use crate::adb::packet::AdbPacket;
use crate::usb::UsbDevice;
use crate::Protocol;
use anyhow::Result;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

pub trait AdbTransport {
    fn send(&mut self, packet: AdbPacket) -> Result<()>;
    fn recv(&mut self) -> Result<AdbPacket>;
}

pub struct AdbTcpTransport(TcpStream);

impl AdbTcpTransport {
    pub fn connect(addrs: impl ToSocketAddrs) -> Result<Self> {
        Ok(Self(TcpStream::connect(addrs)?))
    }
}

impl AdbTransport for AdbTcpTransport {
    fn send(&mut self, packet: AdbPacket) -> Result<()> {
        log::debug!("send {:?}", packet);
        packet.encode(&mut self.0)
    }

    fn recv(&mut self) -> Result<AdbPacket> {
        let packet = AdbPacket::decode(&mut self.0)?;
        log::debug!("recv {:?}", packet);
        Ok(packet)
    }
}

pub struct AdbUsbTransport {
    device: UsbDevice,
    send_buffer: Vec<u8>,
    recv_buffer: Vec<u8>,
}

impl AdbUsbTransport {
    pub fn connect(serial: &str) -> Result<Self> {
        Ok(Self {
            device: UsbDevice::open(serial, Protocol::Adb)?,
            send_buffer: vec![],
            recv_buffer: vec![],
        })
    }
}

impl AdbTransport for AdbUsbTransport {
    fn send(&mut self, packet: AdbPacket) -> Result<()> {
        log::debug!("send {:?}", packet);
        self.send_buffer.clear();
        packet.encode(&mut self.send_buffer)?;
        let n = self
            .device
            .send(&self.send_buffer[..24], Duration::from_secs(1))?;
        anyhow::ensure!(n == 24);
        log::debug!("sent header");
        if packet.payload().len() > 0 {
            let n = self
                .device
                .send(&self.send_buffer[24..], Duration::from_secs(1))?;
            anyhow::ensure!(n == self.send_buffer.len() - 24);
            log::debug!("sent payload");
        }
        Ok(())
    }

    fn recv(&mut self) -> Result<AdbPacket> {
        self.recv_buffer.resize(24, 0);
        let n = self
            .device
            .recv(&mut self.recv_buffer, Duration::from_secs(1))?;
        anyhow::ensure!(n == 24);
        log::debug!("recv header");
        let data_len = u32::from_le_bytes(self.recv_buffer[12..16].try_into().unwrap()) as usize;
        if data_len != 0 {
            self.recv_buffer.resize(24 + data_len, 0);
            let n = self
                .device
                .recv(&mut self.recv_buffer[24..], Duration::from_secs(1))?;
            anyhow::ensure!(n == data_len);
            log::debug!("recv payload");
        }
        let packet = AdbPacket::decode(&mut &self.recv_buffer[..])?;
        log::debug!("recv {:?}", packet);
        Ok(packet)
    }
}
