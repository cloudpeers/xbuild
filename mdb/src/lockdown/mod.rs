use crate::lockdown::transport::{LockdownTcpTransport, LockdownTransport, LockdownUsbTransport};
use crate::{DeviceId, Protocol, Transport};
use anyhow::Result;
use serde::Serialize;

mod transport;

pub struct Lockdown {
    transport: Box<dyn LockdownTransport>,
}

impl Lockdown {
    pub fn connect(device_id: &DeviceId) -> Result<Self> {
        anyhow::ensure!(device_id.protocol() == Protocol::Usbmux);
        let transport: Box<dyn LockdownTransport> = match device_id.transport() {
            Transport::Usb => Box::new(LockdownUsbTransport::connect(device_id.serial())?),
            Transport::Tcp(addr) => Box::new(LockdownTcpTransport::connect(addr)?),
        };
        Ok(Self { transport })
    }

    fn send<T: Serialize>(&mut self, msg: &T) -> Result<()> {
        let mut buf = vec![];
        plist::to_writer_xml(&mut buf, &msg)?;
        buf.push(b'\n');
        self.transport.send(buf)?;
        Ok(())
    }

    fn recv(&mut self) -> Result<String> {
        let bytes = self.transport.recv()?;
        Ok(String::from_utf8(bytes)?)
    }

    pub fn query_type(&mut self, label: &str) -> Result<String> {
        self.send(&Args {
            request: "QueryType",
            label: Some(label),
            key: None,
        })?;
        self.recv()
    }

    pub fn get_value(&mut self, key: Option<&str>) -> Result<String> {
        self.send(&Args {
            request: "GetValue",
            label: None,
            key,
        })?;
        self.recv()
    }
}

#[derive(Serialize)]
struct Args<'a> {
    #[serde(rename = "Request")]
    pub request: &'a str,
    #[serde(rename = "Label")]
    pub label: Option<&'a str>,
    #[serde(rename = "Key")]
    pub key: Option<&'a str>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usb::usb_devices;

    #[test]
    fn test_lockdown_tcp() -> Result<()> {
        env_logger::try_init().ok();
        let device_id = DeviceId::new(
            "unknown".to_string(),
            Protocol::Usbmux,
            Transport::Tcp("192.168.2.215:62078".parse()?),
        );
        let mut lockdown = Lockdown::connect(&device_id)?;
        let values = lockdown.get_value(None)?;
        println!("{}", values);
        Ok(())
    }

    #[test]
    fn test_lockdown_usb() -> Result<()> {
        env_logger::try_init().ok();
        let device = usb_devices()?.iter().next().unwrap();
        let device = DeviceId::new(device?.serial().into(), Protocol::Usbmux, Transport::Usb);
        let mut lockdown = Lockdown::connect(&device)?;
        let values = lockdown.get_value(None)?;
        println!("{}", values);
        Ok(())
    }
}
