use anyhow::Result;
use byteorder::{ReadBytesExt, WriteBytesExt, BE};
use std::io::{Read, Write};

pub struct TcpState {
    connected: bool,
    src_port: u16,
    dest_port: u16,

    tx_seq: u32,
    tx_ack: u32,
    tx_win: u32,

    rx_seq: u32,
    rx_ack: u32,
    rx_win: u32,
}

impl TcpState {
    pub fn new(src_port: u16, dest_port: u16) -> Self {
        Self {
            connected: false,
            src_port,
            dest_port,

            tx_seq: 0,
            tx_ack: 0,
            tx_win: 0x8_0000,

            rx_seq: 0,
            rx_ack: 0,
            rx_win: 0,
        }
    }

    pub fn syn(&mut self) -> TcpPacket {
        self.send(TcpFlag::Syn, vec![])
    }

    pub fn ack(&mut self) -> TcpPacket {
        self.send(TcpFlag::Ack, vec![])
    }

    pub fn rst(&mut self) -> TcpPacket {
        self.send(TcpFlag::Rst, vec![])
    }

    pub fn data(&mut self, payload: Vec<u8>) -> TcpPacket {
        self.send(TcpFlag::Ack, payload)
    }

    fn send(&mut self, flags: TcpFlag, payload: Vec<u8>) -> TcpPacket {
        let packet = TcpPacket {
            src_port: self.src_port,
            dest_port: self.dest_port,
            seq_num: self.tx_seq,
            ack_num: self.tx_ack,
            data_offset: 5,
            flags: flags as _,
            window_size: (self.tx_win >> 8) as u16,
            checksum: 0,
            urgent_ptr: 0,
            payload,
        };
        log::info!(
            "send seq = {} ack = {} len = {} flags = {}",
            packet.seq_num,
            packet.ack_num,
            packet.payload.len(),
            packet.flags
        );
        self.tx_seq += packet.payload().len() as u32;
        packet
    }

    pub fn recv(&mut self, packet: TcpPacket) -> Result<Vec<u8>> {
        log::info!(
            "recv seq = {} ack = {} len = {} flags = {}",
            packet.seq_num,
            packet.ack_num,
            packet.payload.len(),
            packet.flags
        );
        self.rx_seq = packet.seq_num;
        self.rx_ack = packet.ack_num;
        self.rx_win = (packet.window_size as u32) << 8;
        self.tx_ack += packet.payload().len() as u32;

        if packet.flags & TcpFlag::Rst as u8 > 0 {
            let msg = std::str::from_utf8(&packet.payload)?;
            anyhow::bail!("connection reset: {}", msg);
        }

        if !self.connected {
            anyhow::ensure!(packet.flags == (TcpFlag::Syn as u8 | TcpFlag::Ack as u8));
            anyhow::ensure!(packet.payload.is_empty());
            self.connected = true;
            self.tx_seq += 1;
            self.tx_ack += 1;
        } else {
            anyhow::ensure!(packet.flags == TcpFlag::Ack as _);
        }

        Ok(packet.payload)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum TcpFlag {
    Syn = 2,
    Rst = 4,
    Ack = 16,
}

#[derive(Debug)]
pub struct TcpPacket {
    src_port: u16,
    dest_port: u16,
    seq_num: u32,
    ack_num: u32,
    data_offset: u8,
    flags: u8,
    window_size: u16,
    checksum: u16,
    urgent_ptr: u16,
    payload: Vec<u8>,
}

impl TcpPacket {
    pub fn dest_port(&self) -> u16 {
        self.dest_port
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub fn encode(&self, w: &mut impl Write) -> Result<()> {
        w.write_u16::<BE>(self.src_port)?;
        w.write_u16::<BE>(self.dest_port)?;
        w.write_u32::<BE>(self.seq_num)?;
        w.write_u32::<BE>(self.ack_num)?;
        w.write_u8(self.data_offset << 4)?;
        w.write_u8(self.flags)?;
        w.write_u16::<BE>(self.window_size)?;
        w.write_u16::<BE>(self.checksum)?;
        w.write_u16::<BE>(self.urgent_ptr)?;
        w.write_all(&self.payload)?;
        Ok(())
    }

    pub fn decode(r: &mut impl Read) -> Result<Self> {
        let src_port = r.read_u16::<BE>()?;
        let dest_port = r.read_u16::<BE>()?;
        let seq_num = r.read_u32::<BE>()?;
        let ack_num = r.read_u32::<BE>()?;
        let data_offset = r.read_u8()? >> 4;
        let flags = r.read_u8()?;
        let window_size = r.read_u16::<BE>()?;
        let checksum = r.read_u16::<BE>()?;
        let urgent_ptr = r.read_u16::<BE>()?;
        let mut payload = vec![];
        r.read_to_end(&mut payload)?;
        Ok(Self {
            src_port,
            dest_port,
            seq_num,
            ack_num,
            data_offset,
            flags,
            window_size,
            checksum,
            urgent_ptr,
            payload,
        })
    }
}
