use crate::adb::pubkey::AndroidPublicKey;
use anyhow::Result;
use byteorder::{ReadBytesExt, WriteBytesExt, LE};
use rsa::RsaPublicKey;
use std::io::{Read, Write};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Command {
    Sync = 0x434e5953,
    Connect = 0x4e584e43,
    Auth = 0x48545541,
    Open = 0x4e45504f,
    Ready = 0x59414b4f,
    Close = 0x45534c43,
    Write = 0x45545257,
}

impl Command {
    pub fn new(cmd: u32) -> Option<Self> {
        Some(match cmd {
            cmd if cmd == Self::Sync as _ => Self::Sync,
            cmd if cmd == Self::Connect as _ => Self::Connect,
            cmd if cmd == Self::Auth as _ => Self::Auth,
            cmd if cmd == Self::Open as _ => Self::Open,
            cmd if cmd == Self::Ready as _ => Self::Ready,
            cmd if cmd == Self::Close as _ => Self::Close,
            cmd if cmd == Self::Write as _ => Self::Write,
            _ => return None,
        })
    }
}

fn crc(payload: &[u8]) -> u32 {
    payload.iter().map(|&b| b as u32).sum()
}

#[derive(Debug)]
pub struct AdbPacket {
    command: Command,
    arg0: u32,
    arg1: u32,
    payload: Vec<u8>,
}

impl AdbPacket {
    pub fn connect(version: u32, max_data: u32, system_identity: &str) -> Self {
        let mut payload = Vec::with_capacity(system_identity.len() + 1);
        payload.extend_from_slice(system_identity.as_bytes());
        payload.push(0);
        Self {
            command: Command::Connect,
            arg0: version,
            arg1: max_data,
            payload,
        }
    }

    pub fn auth_signature(sig: Vec<u8>) -> Self {
        Self {
            command: Command::Auth,
            arg0: 2,
            arg1: 0,
            payload: sig,
        }
    }

    pub fn auth_rsa_public_key(pubkey: RsaPublicKey) -> Self {
        Self {
            command: Command::Auth,
            arg0: 3,
            arg1: 0,
            payload: AndroidPublicKey::new(pubkey).encode().unwrap().into_bytes(),
        }
    }

    pub fn open(local_id: u32, dest: String) -> Self {
        Self {
            command: Command::Open,
            arg0: local_id,
            arg1: 0,
            payload: dest.into_bytes(),
        }
    }

    pub fn ready(local_id: u32, remote_id: u32) -> Self {
        Self {
            command: Command::Ready,
            arg0: local_id,
            arg1: remote_id,
            payload: vec![],
        }
    }

    pub fn write(local_id: u32, remote_id: u32, data: Vec<u8>) -> Self {
        Self {
            command: Command::Write,
            arg0: local_id,
            arg1: remote_id,
            payload: data,
        }
    }

    pub fn close(local_id: u32, remote_id: u32) -> Self {
        Self {
            command: Command::Close,
            arg0: local_id,
            arg1: remote_id,
            payload: vec![],
        }
    }

    pub fn command(&self) -> Command {
        self.command
    }

    pub fn arg0(&self) -> u32 {
        self.arg0
    }

    pub fn arg1(&self) -> u32 {
        self.arg1
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub fn encode(&self, w: &mut impl Write) -> Result<()> {
        w.write_u32::<LE>(self.command as u32)?;
        w.write_u32::<LE>(self.arg0)?;
        w.write_u32::<LE>(self.arg1)?;
        w.write_u32::<LE>(self.payload.len() as _)?;
        w.write_u32::<LE>(crc(&self.payload))?;
        w.write_u32::<LE>(self.command as u32 ^ 0xffff_ffff)?;
        w.write_all(&self.payload)?;
        Ok(())
    }

    pub fn decode(r: &mut impl Read) -> Result<Self> {
        let command = r.read_u32::<LE>()?;
        let arg0 = r.read_u32::<LE>()?;
        let arg1 = r.read_u32::<LE>()?;
        let data_len = r.read_u32::<LE>()?;
        let data_crc = r.read_u32::<LE>()?;
        let magic = r.read_u32::<LE>()?;
        anyhow::ensure!(command ^ 0xffff_ffff == magic);
        let mut payload = Vec::with_capacity(data_len as _);
        r.take(data_len as _).read_to_end(&mut payload)?;
        anyhow::ensure!(crc(&payload) == data_crc);
        let command = Command::new(command)
            .ok_or_else(|| anyhow::anyhow!("unknown command 0x{:x}", command))?;
        Ok(Self {
            command,
            arg0,
            arg1,
            payload,
        })
    }
}
