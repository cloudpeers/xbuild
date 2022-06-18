use crate::client::{AdbConnection, AdbStreamPacket};
use crate::message::Command;
use crate::sync::{SyncCommand, SyncStreamExt};
use anyhow::Result;
use log::debug;
use std::fs::File;
use std::io::prelude::*;
use std::io::BufReader;
use std::io::SeekFrom;
use std::path::Path;
use std::time::SystemTime;

impl AdbConnection {
    pub fn push_reader<R: Read + Seek>(&mut self, r: R, remote_path: &str) -> Result<()> {
        let mut r = r;

        let size = r.seek(SeekFrom::End(0))?;
        r.seek(SeekFrom::Start(0))?;

        debug!("size = 0x{:x}", size);

        let stream = self.open_stream("sync:")?;

        debug!("STAT");
        let stat = SyncCommand::new_stat(remote_path);
        let packet = AdbStreamPacket::new_write(stat);
        stream.send(packet)?;
        stream.sync_recv_ok()?;
        debug!("STAT ok");

        let reply = stream.sync_recv_command(Command::A_WRTE)?;
        debug!(
            "STAT data received: payload len = 0x{:x}",
            reply.payload.len()
        );

        stream.send_ok()?;

        debug!("SEND");
        let send = SyncCommand::new_send(remote_path, 0o100644);
        let packet = AdbStreamPacket::new_write(send);
        stream.send(packet)?;
        stream.sync_recv_ok()?;
        debug!("SEND ok");

        let mut data = SyncCommand::new_data(self.max_data_len());
        let mut bytes_sent = 0;

        loop {
            let n = data.read_payload_from(&mut r)?;
            if n == 0 {
                break;
            }

            let next_pos: u64 = bytes_sent + n as u64;

            assert!(next_pos <= size);

            let is_last_chunk = next_pos == size;
            debug!("DATA [0x{:x}:0x{:x}]", bytes_sent, next_pos);
            bytes_sent = bytes_sent + n as u64;

            let packet = AdbStreamPacket::new_write(&data);

            if is_last_chunk {
                let now = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as u32;
                let done = SyncCommand::new_done(now);
                let space = self.max_data_len() - data.len();
                if space >= done.len() {
                    data.extend(done);
                    let packet = AdbStreamPacket::new_write(&data);
                    stream.send(packet)?;
                    stream.sync_recv_ok()?;
                    debug!("DATA last chunk ok");
                } else {
                    let append_len = space;
                    if append_len > 0 {
                        data.extend(&done[0..append_len]);
                    }
                    let packet = AdbStreamPacket::new_write(&data);
                    stream.send(packet)?;
                    stream.sync_recv_ok()?;
                    stream.send(AdbStreamPacket::new_write(&done[append_len..]))?;
                    stream.sync_recv_ok()?;
                }
                break;
            } else {
                stream.send(packet)?;

                stream.sync_recv_ok()?;
                debug!("DATA chunk ok");
            }
        }

        assert_eq!(bytes_sent as u64, size);

        let reply = stream.sync_recv_command(Command::A_WRTE)?;
        debug!("result = {}", String::from_utf8_lossy(&reply.payload));

        stream.send_ok()?;

        debug!("QUIT");
        let quit = SyncCommand::new_quit();
        let packet = AdbStreamPacket::new_write(quit);
        stream.send(packet)?;

        stream.sync_recv_ok()?;

        stream.send_close()?;

        stream.sync_recv_command(Command::A_CLSE)?;

        Ok(())
    }

    pub fn push<P: AsRef<Path>>(&mut self, local_path: P, remote_path: &str) -> Result<()> {
        let file = File::open(local_path)?;
        let r = BufReader::new(file);
        self.push_reader(r, remote_path)
    }
}
