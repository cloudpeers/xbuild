use crate::usbmux::tcp::TcpPacket;
use anyhow::Result;
use byteorder::{ReadBytesExt, WriteBytesExt, BE};
use std::io::{Read, Write};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Command {
    Version = 0,
    Control = 1,
    Setup = 2,
    Tcp = 6,
}

impl Command {
    pub fn new(cmd: u32) -> Option<Self> {
        Some(match cmd {
            cmd if cmd == Self::Version as _ => Self::Version,
            cmd if cmd == Self::Control as _ => Self::Control,
            cmd if cmd == Self::Setup as _ => Self::Setup,
            cmd if cmd == Self::Tcp as _ => Self::Tcp,
            _ => return None,
        })
    }
}

pub struct MuxPacket {
    command: Command,
    tx_seq: u16,
    rx_seq: u16,
    payload: Vec<u8>,
}

impl MuxPacket {
    pub fn version(major: u32, minor: u32) -> Result<Self> {
        let mut payload = Vec::with_capacity(12);
        payload.write_u32::<BE>(major)?;
        payload.write_u32::<BE>(minor)?;
        payload.write_u32::<BE>(0)?;
        Ok(Self {
            command: Command::Version,
            tx_seq: 0,
            rx_seq: 0xffff,
            payload,
        })
    }

    pub fn setup() -> Self {
        Self {
            command: Command::Setup,
            tx_seq: 0,
            rx_seq: 0xffff,
            payload: vec![0x7],
        }
    }

    pub fn tcp(packet: TcpPacket) -> Self {
        let mut payload = Vec::with_capacity(20 + packet.payload().len());
        packet.encode(&mut payload).unwrap();
        Self {
            command: Command::Tcp,
            tx_seq: 0,
            rx_seq: 0xffff,
            payload,
        }
    }

    pub fn command(&self) -> Command {
        self.command
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub fn version_payload(&self) -> Result<u32> {
        anyhow::ensure!(self.command() == Command::Version);
        Ok(u32::from_be_bytes(self.payload()[..4].try_into().unwrap()))
    }

    pub fn control_payload(&self) -> Result<(u8, String)> {
        anyhow::ensure!(self.command() == Command::Control);
        let ty = self.payload[0];
        let msg = std::str::from_utf8(&self.payload[1..])?;
        Ok((ty, msg.to_string()))
    }

    pub fn tcp_payload(&self) -> Result<TcpPacket> {
        anyhow::ensure!(self.command == Command::Tcp);
        TcpPacket::decode(&mut &self.payload[..])
    }

    pub fn rx_seq(&self) -> u16 {
        self.rx_seq
    }

    pub fn set_seq(&mut self, tx_seq: u16, rx_seq: u16) {
        self.tx_seq = tx_seq;
        self.rx_seq = rx_seq;
    }

    pub fn encode(&self, w: &mut impl Write, version: u32) -> Result<()> {
        let mut length = 8 + self.payload.len();
        if version >= 2 {
            length += 8;
        }
        //anyhow::ensure!(length < USB_MTU);
        w.write_u32::<BE>(self.command as u32)?;
        w.write_u32::<BE>(length as u32)?;
        if version >= 2 {
            w.write_u32::<BE>(0xfeedface)?;
            w.write_u16::<BE>(self.tx_seq)?;
            w.write_u16::<BE>(self.rx_seq)?;
        }
        w.write_all(&self.payload)?;
        Ok(())
    }

    pub fn decode(r: &mut impl Read, version: u32) -> Result<Self> {
        let command = r.read_u32::<BE>()?;
        let length = r.read_u32::<BE>()?;
        anyhow::ensure!(length >= 8);
        let mut payload_len = length - 8;
        let mut tx_seq = 0;
        let mut rx_seq = 0xffff;
        if version >= 2 {
            anyhow::ensure!(length >= 16);
            let _magic = r.read_u32::<BE>()?;
            //anyhow::ensure!(magic == 0xfeedface);
            tx_seq = r.read_u16::<BE>()?;
            rx_seq = r.read_u16::<BE>()?;
            payload_len -= 8;
        }
        let mut payload = Vec::with_capacity(payload_len as _);
        r.take(payload_len as _).read_to_end(&mut payload)?;
        let command = Command::new(command)
            .ok_or_else(|| anyhow::anyhow!("unknown command 0x{:x}", command))?;
        Ok(Self {
            command,
            tx_seq,
            rx_seq,
            payload,
        })
    }
}

impl std::fmt::Debug for MuxPacket {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let payload = if let Ok(payload) = self.control_payload() {
            format!("{:?}", payload)
        } else if let Ok(payload) = self.tcp_payload() {
            format!("{:?}", payload)
        } else {
            format!("{:x?}", self.payload)
        };
        f.debug_struct("MuxPacket")
            .field("command", &self.command)
            .field("payload", &payload)
            .field("tx_seq", &self.tx_seq)
            .field("rx_seq", &self.rx_seq)
            .finish()
    }
}
