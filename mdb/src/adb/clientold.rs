use bytes::{Bytes, BytesMut};
use crossbeam_channel::{bounded, select, unbounded, Receiver, Sender};
use log::{debug, error, warn};
use rsa::{Hash, PaddingScheme, RsaPrivateKey, RsaPublicKey};
use std::collections::HashMap;
use std::io::prelude::*;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{Arc, RwLock};
use std::thread::{self, JoinHandle};

pub use crate::message::Command;

use crate::message::{Connect, Header};
use crate::pubkey::AndroidPublicKey;
use crate::result::*;

#[derive(Debug)]
pub struct AdbClient {
    private_key: RsaPrivateKey,
    system_identity: String,
}

impl AdbClient {
    pub fn new(private_key: RsaPrivateKey, system_identity: &str) -> Self {
        AdbClient {
            private_key,
            system_identity: system_identity.to_string(),
        }
    }

    pub fn connect<T>(self, addr: T) -> AdbResult<AdbConnection>
    where
        T: ToSocketAddrs,
    {
        let addrs: Vec<_> = addr.to_socket_addrs()?.collect();

        debug!("connecting to {:?}...", addrs);

        let mut stream = TcpStream::connect(&addrs as &[SocketAddr])?;

        debug!("connected. sending CNXN...");

        Connect::new(&self.system_identity).encode(&mut stream)?;

        let mut auth = 0;
        let (resp, data) = loop {
            let resp = Header::decode(&mut stream)?;
            match resp.get_command() {
                Some(Command::A_CNXN) => {
                    println!("connected");
                    let data = resp.decode_data(&mut stream)?;
                    break (resp, data);
                }
                Some(Command::A_AUTH) => match auth {
                    0 => {
                        let token = resp.decode_data(&mut stream)?;
                        let padding = PaddingScheme::new_pkcs1v15_sign(Some(Hash::SHA1));
                        let signature = self.private_key.sign(padding, &token).unwrap();
                        assert_eq!(signature.len(), 256);
                        let header = Header::new(Command::A_AUTH)
                            .arg0(2u32)
                            .data(&signature)
                            .finalize();
                        header.encode(&mut stream)?;
                        stream.write_all(&signature)?;
                        auth += 1;
                    }
                    1 => {
                        let public_key = RsaPublicKey::from(&self.private_key);
                        let public_key = AndroidPublicKey::new(public_key);
                        let public_key = public_key.encode().unwrap();
                        assert_eq!(public_key.len(), 701);
                        let header = Header::new(Command::A_AUTH)
                            .arg0(3u32)
                            .data(&public_key)
                            .finalize();
                        header.encode(&mut stream)?;
                        stream.write_all(public_key.as_bytes())?;
                        auth += 1;
                    }
                    _ => {
                        return Err(AdbError::AuthNotSupported);
                    }
                },
                Some(cmd) => {
                    return Err(AdbError::UnexpectedCommand(cmd));
                }
                None => return Err(AdbError::UnknownCommand(resp.command)),
            }
        };

        let device_id = String::from_utf8_lossy(&data);

        debug!(
            "handshake ok: device_id = {}, version = 0x{:x}, max_data = 0x{:x}",
            device_id, resp.arg0, resp.arg1
        );

        let streams = Arc::new(RwLock::new(HashMap::<u32, StreamContext>::new()));

        let (conn_reader_s, conn_reader_r) = bounded::<ConnectionPacket>(0);
        let (conn_writer_s, conn_writer_r) = bounded::<ConnectionPacket>(0);
        let (conn_error_s, conn_error_r) = unbounded();

        let reader_worker = thread::spawn({
            let mut stream = stream.try_clone()?;
            let error_s = conn_error_s.clone();
            move || loop {
                let res = Header::decode(&mut stream)
                    .and_then(|header| {
                        let mut payload = BytesMut::new();
                        if header.data_length > 0 {
                            payload.resize(header.data_length as usize, 0);
                            stream
                                .read_exact(&mut payload)
                                .map(move |_| ConnectionPacket {
                                    header,
                                    payload: payload.freeze(),
                                })
                                .map_err(Into::into)
                        } else {
                            Ok(ConnectionPacket {
                                header,
                                payload: payload.freeze(),
                            })
                        }
                    })
                    .and_then(|packet| {
                        conn_reader_s
                            .send(packet)
                            .map_err(|_| AdbError::Disconnected)
                    });

                if let Err(err) = res {
                    debug!("AdbConnection: reader_worker exited: {}", err);
                    error_s.send(err).ok();
                    break;
                }
            }
        });

        let writer_worker = thread::spawn({
            let mut stream = stream.try_clone()?;
            let streams = streams.clone();
            let error_s = conn_error_s.clone();
            move || {
                let mut closed_local_ids = vec![];
                let mut conn_dead = false;
                loop {
                    let packet = conn_writer_r.recv();
                    match packet {
                        Ok(packet) => {
                            let local_id = packet.header.arg0;
                            let locked = streams.read().unwrap();
                            if let Some(ctx) = locked.get(&local_id) {
                                let write = packet.header.encode(&mut stream).and_then(|_| {
                                    stream.write_all(&packet.payload).map_err(Into::into)
                                });
                                match write {
                                    Ok(_) => {
                                        if let Err(_) = ctx.write_result_s.send(Ok(())) {
                                            closed_local_ids.push(local_id);
                                        }
                                    }
                                    Err(err) => {
                                        if let Err(_) =
                                            ctx.write_result_s.send(Err(AdbError::Disconnected))
                                        {
                                            closed_local_ids.push(local_id);
                                        }
                                        conn_dead = true;
                                        error_s.send(err).ok();
                                    }
                                }
                            } else {
                                warn!(
                                    "write packet discarded: cmd = {}, local_id = {}",
                                    packet.header.command, packet.header.arg0
                                );
                            }
                        }
                        Err(_) => {
                            break;
                        }
                    }

                    if !closed_local_ids.is_empty() {
                        let mut locked = streams.write().unwrap();
                        for id in &closed_local_ids {
                            debug!("remove stream: local_id = {}", id);
                            locked.remove(&id);
                        }
                        closed_local_ids.clear();
                    }

                    if conn_dead {
                        break;
                    }
                }
            }
        });

        let dispatch_worker = thread::spawn({
            let streams = streams.clone();
            move || {
                let mut closed_local_ids = vec![];
                loop {
                    select! {
                      recv(conn_reader_r) -> packet => {
                        match packet {
                          Ok(packet) => {
                            let local_id = packet.header.arg1;
                            let locked = streams.read().unwrap();
                            match locked.get(&packet.header.arg1) {
                              Some(ctx) => {
                                if packet.header.get_command().is_some() {
                                  if let Err(_) = ctx.stream_reader_s.send(packet) {
                                    closed_local_ids.push(local_id);
                                  }
                                } else {
                                  error!(
                                    "read packet discarded: unknown_cmd = 0x{:x}, local_id = {}",
                                    packet.header.command,
                                    packet.header.arg1
                                  );
                                }
                              },
                              None => {
                                warn!("read packet discarded: cmd = 0x{:x}, local_id = {}",
                                  packet.header.command,
                                  packet.header.arg1
                                );
                              },
                            }
                          },
                          Err(err) => {
                            error!("recv conn_reader_r: {}", err);
                            break
                          },
                        }
                      },
                      recv(conn_error_r) -> err => {
                        match err {
                          Ok(_) => {
                            break;
                          },
                          Err(recv_err) => {
                            error!("recv conn_error_r: {}", recv_err);
                            break
                          },
                        }
                      },
                    }

                    if !closed_local_ids.is_empty() {
                        let mut locked = streams.write().unwrap();
                        for id in &closed_local_ids {
                            debug!("remove stream: local_id = {}", id);
                            locked.remove(&id);
                        }
                        closed_local_ids.clear();
                    }
                }
                debug!("dispatch worker exited.");
            }
        });

        Ok(AdbConnection {
            system_identity: self.system_identity,
            device_system_identity: device_id.to_string(),
            device_version: resp.arg0,
            device_max_data: resp.arg1,
            tcp_stream: stream,
            local_id_counter: 0,
            workers: vec![reader_worker, writer_worker, dispatch_worker],
            streams,
            conn_writer_s,
        })
    }
}

#[derive(Debug, Clone)]
struct StreamContext {
    local_id: u32,
    remote_id: u32,
    stream_reader_s: Sender<ConnectionPacket>,
    write_result_s: Sender<AdbResult<()>>,
}

#[derive(Debug)]
pub struct AdbConnection {
    system_identity: String,
    device_system_identity: String,
    device_version: u32,
    device_max_data: u32,
    tcp_stream: TcpStream,
    local_id_counter: u32,
    workers: Vec<JoinHandle<()>>,
    streams: Arc<RwLock<HashMap<u32, StreamContext>>>,
    conn_writer_s: Sender<ConnectionPacket>,
}

impl Drop for AdbConnection {
    fn drop(&mut self) {
        use std::net::Shutdown;
        self.tcp_stream.shutdown(Shutdown::Both).ok();
        let (conn_writer_s, _) = bounded::<ConnectionPacket>(0);
        self.conn_writer_s = conn_writer_s;
        for w in std::mem::replace(&mut self.workers, vec![]) {
            w.join().ok();
        }
    }
}

impl AdbConnection {
    pub fn max_data_len(&self) -> usize {
        self.device_max_data as usize
    }

    pub fn open_stream(&mut self, destination: &str) -> AdbResult<AdbStream> {
        use bytes::BufMut;

        self.local_id_counter = self.local_id_counter + 1;
        let local_id = self.local_id_counter;
        debug!(
            "opening stream: local_id = {}, destination = {}...",
            local_id, destination
        );

        let (write_result_s, write_result_r) = bounded::<AdbResult<()>>(1);
        let (stream_reader_s, stream_reader_r) = bounded::<ConnectionPacket>(1);

        let ctx = StreamContext {
            local_id,
            remote_id: 0,
            stream_reader_s,
            write_result_s,
        };

        self.streams.write().unwrap().insert(local_id, ctx);
        debug!("register stream: local_id = {}", local_id);

        let mut dst_bytes = BytesMut::with_capacity(destination.as_bytes().len() + 1);
        dst_bytes.extend(destination.as_bytes());
        dst_bytes.put_u8(0);
        let dst_bytes = dst_bytes.freeze();

        let open_packet = ConnectionPacket {
            header: Header::new(Command::A_OPEN)
                .arg0(local_id)
                .data(&dst_bytes)
                .finalize(),
            payload: dst_bytes,
        };

        self.conn_writer_s
            .send(open_packet)
            .map_err(|_| AdbError::Disconnected)?;

        let open_packet = stream_reader_r.recv().map_err(|_| AdbError::Disconnected)?;
        if open_packet.header.command != Command::A_OKAY as u32 {
            if let Some(cmd) = open_packet.header.get_command() {
                return Err(AdbError::UnexpectedCommand(cmd));
            } else {
                return Err(AdbError::UnknownCommand(open_packet.header.command));
            }
        }

        let local_id = open_packet.header.arg1;
        let remote_id = open_packet.header.arg0;
        debug!("stream opened: {} -> {}", local_id, remote_id);

        Ok(AdbStream {
            local_id,
            remote_id,
            stream_reader: stream_reader_r,
            writer: self.conn_writer_s.clone(),
            write_result_r,
        })
    }
}

#[derive(Debug)]
pub struct AdbStream {
    local_id: u32,
    remote_id: u32,
    stream_reader: Receiver<ConnectionPacket>,
    writer: Sender<ConnectionPacket>,
    write_result_r: Receiver<AdbResult<()>>,
}

impl AdbStream {
    pub fn send(&self, packet: AdbStreamPacket) -> AdbResult<()> {
        self.writer
            .send(ConnectionPacket {
                header: Header::new(packet.command)
                    .arg0(self.local_id)
                    .arg1(self.remote_id)
                    .data(&packet.payload)
                    .finalize(),
                payload: packet.payload,
            })
            .map_err(|_| AdbError::Disconnected)
            .and_then(|_| {
                self.write_result_r
                    .recv()
                    .map_err(|_| AdbError::Disconnected)
                    .and_then(|res| res.map(|_| ()))
            })
    }

    pub fn recv(&self) -> AdbResult<AdbStreamPacket> {
        let packet = self
            .stream_reader
            .recv()
            .map_err(|_| AdbError::Disconnected)?;

        Ok(AdbStreamPacket {
            command: packet
                .header
                .get_command()
                .ok_or_else(|| AdbError::UnknownCommand(packet.header.command))?,
            payload: packet.payload,
        })
    }

    pub fn try_recv(&self) -> AdbResult<Option<AdbStreamPacket>> {
        use crossbeam_channel::TryRecvError;
        match self.stream_reader.try_recv() {
            Ok(packet) => Ok(Some(AdbStreamPacket {
                command: packet
                    .header
                    .get_command()
                    .ok_or_else(|| AdbError::UnknownCommand(packet.header.command))?,
                payload: packet.payload,
            })),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(AdbError::Disconnected),
        }
    }

    pub fn send_ok(&self) -> AdbResult<()> {
        self.send(AdbStreamPacket {
            command: Command::A_OKAY,
            payload: Bytes::new(),
        })
    }

    pub fn recv_command(&self, cmd: Command) -> AdbResult<AdbStreamPacket> {
        let packet = self.recv()?;
        if packet.command != cmd {
            return Err(AdbError::UnexpectedCommand(packet.command));
        }
        Ok(packet)
    }

    pub fn send_close(&self) -> AdbResult<()> {
        self.send(AdbStreamPacket {
            command: Command::A_CLSE,
            payload: Bytes::new(),
        })
    }
}

impl Iterator for AdbStream {
    type Item = Vec<u8>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Ok(packet) = self.recv_command(Command::A_WRTE) {
            Some(packet.payload.to_vec())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::pkcs8::DecodePrivateKey;

    #[test]
    fn test_connect() {
        env_logger::try_init().ok();
        let private_key = RsaPrivateKey::read_pkcs8_pem_file("/home/dvc/.android/adbkey").unwrap();
        AdbClient::new(private_key, "host::")
            .connect("192.168.2.43:5555")
            .unwrap();
    }
}
