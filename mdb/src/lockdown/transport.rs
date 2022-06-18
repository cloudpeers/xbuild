use crate::usbmux::Usbmux;
use anyhow::Result;
use byteorder::{ReadBytesExt, WriteBytesExt, BE};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};

pub trait LockdownTransport {
    fn send(&mut self, packet: Vec<u8>) -> Result<()>;
    fn recv(&mut self) -> Result<Vec<u8>>;
}

pub struct LockdownTcpTransport(TcpStream);

impl LockdownTcpTransport {
    pub fn connect(addrs: impl ToSocketAddrs) -> Result<Self> {
        Ok(Self(TcpStream::connect(addrs)?))
    }
}

impl LockdownTransport for LockdownTcpTransport {
    fn send(&mut self, packet: Vec<u8>) -> Result<()> {
        self.0.write_u32::<BE>(packet.len() as _)?;
        self.0.write_all(&packet)?;
        Ok(())
    }

    fn recv(&mut self) -> Result<Vec<u8>> {
        let len = self.0.read_u32::<BE>()? as usize;
        let mut packet = Vec::with_capacity(len);
        (&mut self.0).take(len as u64).read_to_end(&mut packet)?;
        Ok(packet)
    }
}

pub struct LockdownUsbTransport {
    muxer: Usbmux,
    port: u16,
}

impl LockdownUsbTransport {
    pub fn connect(serial: &str) -> Result<Self> {
        let mut muxer = Usbmux::connect(serial)?;
        let port = muxer.open(62078)?;
        Ok(Self { muxer, port })
    }
}

impl LockdownTransport for LockdownUsbTransport {
    fn send(&mut self, packet: Vec<u8>) -> Result<()> {
        let mut buf = Vec::with_capacity(packet.len() + 4);
        buf.write_u32::<BE>(packet.len() as u32)?;
        buf.write_all(&packet)?;
        self.muxer.send(self.port, buf)
    }

    fn recv(&mut self) -> Result<Vec<u8>> {
        let (port, buf) = self.muxer.recv()?;
        anyhow::ensure!(buf.len() >= 4);
        anyhow::ensure!(port == self.port);
        let len = u32::from_be_bytes(buf[..4].try_into().unwrap()) as usize;
        let mut buf = buf[4..].to_vec();
        while buf.len() < len {
            let (port, bytes) = self.muxer.recv()?;
            anyhow::ensure!(port == self.port);
            buf.extend_from_slice(&bytes);
        }
        Ok(buf)
    }
}
