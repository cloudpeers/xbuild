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
        log::debug!("send {:02x?}", &packet);
        self.muxer.send(self.port, packet)
    }

    fn recv(&mut self) -> Result<Vec<u8>> {
        let (port, buf) = self.muxer.recv()?;
        anyhow::ensure!(port == self.port);
        Ok(buf)
        /*anyhow::ensure!(buf.len() >= 4);
        let len = u32::from_be_bytes(buf[..4].try_into().unwrap()) as usize;
        let mut buf = buf[4..].to_vec();
        while buf.len() < len {
            let (port, bytes) = self.muxer.recv()?;
            anyhow::ensure!(port == self.port);
            buf.extend_from_slice(&bytes);
        }
        log::debug!("recv {:02x?}", &buf[..]);
        Ok(buf)*/
    }
}
