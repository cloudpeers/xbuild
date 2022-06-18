use crate::usbmux::packet::MuxPacket;
use crate::usbmux::tcp::TcpState;
use crate::usbmux::transport::MuxUsbTransport;
use anyhow::Result;
use std::collections::BTreeMap;

pub struct Usbmux {
    transport: MuxUsbTransport,
    streams: BTreeMap<u16, TcpState>,
}

impl Usbmux {
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

impl Drop for Usbmux {
    fn drop(&mut self) {
        for tcp_state in self.streams.values_mut() {
            self.transport.send(MuxPacket::tcp(tcp_state.rst())).ok();
        }
    }
}
