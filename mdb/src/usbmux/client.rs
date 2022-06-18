use crate::usbmux::packet::MuxPacket;
use crate::usbmux::tcp::TcpState;
use crate::usbmux::transport::MuxUsbTransport;
use anyhow::Result;
use futures::channel::{mpsc, oneshot};
use futures::prelude::*;
use std::collections::BTreeMap;

struct UsbmuxHandler {
    transport: MuxUsbTransport,
    streams: BTreeMap<u16, TcpState>,
}

impl UsbmuxHandler {
    pub fn connect(serial: &str) -> Result<Self> {
        let mut transport = MuxUsbTransport::connect(serial)?;
        transport.send(MuxPacket::version(2, 0)?)?;
        let version = transport.recv()?.version_payload()?;
        log::debug!("usbmux negotiated version {}", version);
        transport.set_version(version);
        if version >= 2 {
            transport.send(MuxPacket::setup())?;
        }
        Ok(Self {
            transport,
            streams: Default::default(),
        })
    }

    pub fn open(&mut self, port: u16) -> Result<u16> {
        anyhow::ensure!(self.streams.len() < 0xfff0);
        let sport = (0x0010..0xffff)
            .find(|sport| !self.streams.contains_key(sport))
            .unwrap();
        let mut tcp_state = TcpState::new(sport, port);

        self.transport.send(MuxPacket::tcp(tcp_state.syn()))?;
        tcp_state.recv(self.transport.recv()?.tcp_payload()?)?;
        self.transport.send(MuxPacket::tcp(tcp_state.ack()))?;

        self.streams.insert(sport, tcp_state);
        Ok(sport)
    }

    pub fn send(&mut self, port: u16, bytes: Vec<u8>) -> Result<()> {
        let tcp_state = self
            .streams
            .get_mut(&port)
            .ok_or_else(|| anyhow::anyhow!("port closed"))?;
        self.transport.send(MuxPacket::tcp(tcp_state.data(bytes)))?;
        Ok(())
    }

    pub fn recv(&mut self) -> Result<(u16, Vec<u8>)> {
        let tcp_packet = self.transport.recv()?.tcp_payload()?;
        let port = tcp_packet.dest_port();
        let tcp_state = self
            .streams
            .get_mut(&port)
            .ok_or_else(|| anyhow::anyhow!("port closed"))?;
        let payload = tcp_state.recv(tcp_packet)?;
        Ok((port, payload))
    }

    pub fn close(&mut self, port: u16) -> Result<()> {
        let mut tcp_state = self
            .streams
            .remove(&port)
            .ok_or_else(|| anyhow::anyhow!("port closed"))?;
        self.transport.send(MuxPacket::tcp(tcp_state.rst()))?;
        Ok(())
    }
}

impl Drop for UsbmuxHandler {
    fn drop(&mut self) {
        for tcp_state in self.streams.values_mut() {
            self.transport.send(MuxPacket::tcp(tcp_state.rst())).ok();
        }
    }
}

enum Command {
    Open(
        u16,
        mpsc::Sender<Result<Vec<u8>>>,
        oneshot::Sender<Result<u16>>,
    ),
    Send(u16, Vec<u8>, oneshot::Sender<Result<()>>),
    Close(u16, oneshot::Sender<Result<()>>),
}

pub struct Usbmux {
    tx: mpsc::Sender<Command>,
    task: async_global_executor::Task<()>,
}

impl Usbmux {
    pub fn connect(serial: &str) -> Result<Self> {
        let (tx, mut rx) = mpsc::channel(1);
        let mut muxer = UsbmuxHandler::connect(serial)?;
        let task = async_global_executor::spawn_blocking(move || loop {
            match futures::executor::block_on(rx.next()) {
                Some(Command::Open(port, tcp_tx, tx)) => {
                    tx.send(muxer.open(port)).ok();
                }
                Some(Command::Send(port, packet, tx)) => {
                    tx.send(muxer.send(port, packet)).ok();
                }
                Some(Command::Close(port, tx)) => {
                    tx.send(muxer.close(port)).ok();
                }
                None => break,
            }
        });
        Ok(Self { tx, task })
    }

    pub async fn open(&mut self, port: u16) -> Result<TcpStream> {
        let (tx, rx) = oneshot::channel();
        let (tcp_tx, tcp_rx) = mpsc::channel(1);
        self.tx.send(Command::Open(port, tcp_tx, tx)).await?;
        let port = rx.await??;
        Ok(TcpStream {
            port,
            tx: self.tx.clone(),
            rx: tcp_rx,
        })
    }
}

pub struct TcpStream {
    port: u16,
    tx: mpsc::Sender<Command>,
    rx: mpsc::Receiver<Result<Vec<u8>>>,
}

impl TcpStream {
    pub async fn send(&mut self, packet: Vec<u8>) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Command::Send(self.port, packet, tx)).await?;
        Ok(rx.await??)
    }

    pub async fn recv(&mut self) -> Result<Vec<u8>> {
        self.rx.next().await.unwrap()
    }
}

// TODO: impl Read + Write
