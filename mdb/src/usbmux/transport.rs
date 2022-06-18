use crate::usb::UsbDeviceHandle;
use crate::usbmux::packet::{Command, MuxPacket};
use crate::Protocol;
use anyhow::Result;

const USB_MRU: usize = 16384;

pub struct MuxUsbTransport {
    device: UsbDeviceHandle,
    version: u32,
    tx_seq: u16,
    rx_seq: u16,
    send_buffer: Vec<u8>,
    recv_buffer: Vec<u8>,
}

impl MuxUsbTransport {
    pub fn connect(serial: &str) -> Result<Self> {
        Ok(Self {
            device: UsbDeviceHandle::open(serial, Protocol::Usbmux)?,
            version: 0,
            tx_seq: 0,
            rx_seq: 0xffff,
            send_buffer: vec![],
            recv_buffer: vec![0; USB_MRU],
        })
    }
}

impl MuxUsbTransport {
    pub fn set_version(&mut self, version: u32) {
        self.version = version;
    }

    pub fn send(&mut self, mut packet: MuxPacket) -> Result<()> {
        if packet.command() == Command::Setup {
            self.tx_seq = 0;
            self.rx_seq = 0xffff;
        }
        packet.set_seq(self.tx_seq, self.rx_seq);
        self.tx_seq += 1;
        //log::debug!("send {:x?}", packet);
        self.send_buffer.clear();
        packet.encode(&mut self.send_buffer, self.version)?;
        let n = self.device.send(&self.send_buffer)?;
        anyhow::ensure!(n == self.send_buffer.len());
        //log::debug!("sent packet");
        Ok(())
    }

    pub fn recv(&mut self) -> Result<MuxPacket> {
        loop {
            let n = self.device.recv(&mut self.recv_buffer)?;
            log::debug!("recv {:x?}", &self.recv_buffer[..n]);
            let packet = MuxPacket::decode(&mut &self.recv_buffer[..n], self.version)?;
            self.rx_seq = packet.rx_seq();
            //log::debug!("recv {:?}", packet);
            if packet.command() == Command::Control {
                let (_, msg) = packet.control_payload()?;
                log::error!("control: {}", msg);
                continue;
            }
            return Ok(packet);
        }
    }
}
