use crate::lockdown::transport::{LockdownTcpTransport, LockdownTransport, LockdownUsbTransport};
use crate::{DeviceId, Protocol, Transport};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[macro_use]
mod plist_macro;
mod transport;

fn config_path(uuid: &str) -> PathBuf {
    Path::new("/var/lib/lockdown").join(format!("{}.plist", uuid))
}

fn tls_priv_key(bytes: &[u8]) -> Result<rustls::PrivateKey> {
    let pem = pem::parse(&bytes)?;
    Ok(rustls::PrivateKey(pem.contents))
}

fn tls_cert(bytes: &[u8]) -> Result<rustls::Certificate> {
    let pem = pem::parse(&bytes)?;
    Ok(rustls::Certificate(pem.contents))
}

#[derive(Deserialize)]
pub struct Config {
    #[serde(rename = "DeviceCertificate")]
    #[serde(with = "serde_bytes")]
    device_certificate: Vec<u8>,
    #[serde(rename = "HostPrivateKey")]
    #[serde(with = "serde_bytes")]
    host_private_key: Vec<u8>,
    #[serde(rename = "HostCertificate")]
    #[serde(with = "serde_bytes")]
    host_certificate: Vec<u8>,
    #[serde(rename = "RootPrivateKey")]
    #[serde(with = "serde_bytes")]
    root_private_key: Vec<u8>,
    #[serde(rename = "RootCertificate")]
    #[serde(with = "serde_bytes")]
    root_certificate: Vec<u8>,
    #[serde(rename = "SystemBUID")]
    system_buid: String,
    #[serde(rename = "HostID")]
    host_id: String,
    #[serde(rename = "EscrowBag")]
    #[serde(with = "serde_bytes")]
    escrow_bag: Vec<u8>,
    #[serde(rename = "WiFiMACAddress")]
    mac_address: String,
}

impl Config {
    pub fn from_uuid(uuid: &str) -> Result<Self> {
        let buf = std::fs::read(config_path(uuid))?;
        Ok(plist::from_reader_xml(&mut &buf[..])?)
    }

    pub fn device_certificate(&self) -> Result<rustls::Certificate> {
        tls_cert(&self.device_certificate)
    }

    pub fn host_private_key(&self) -> Result<rustls::PrivateKey> {
        tls_priv_key(&self.host_private_key)
    }

    pub fn host_certificate(&self) -> Result<rustls::Certificate> {
        tls_cert(&self.host_certificate)
    }

    pub fn root_private_key(&self) -> Result<rustls::PrivateKey> {
        tls_priv_key(&self.root_private_key)
    }

    pub fn root_certificate(&self) -> Result<rustls::Certificate> {
        tls_cert(&self.root_certificate)
    }
}

pub struct Lockdown {
    transport: Box<dyn LockdownTransport>,
    session: Option<Session>,
}

impl Lockdown {
    pub fn connect(device_id: &DeviceId) -> Result<Self> {
        anyhow::ensure!(device_id.protocol() == Protocol::Usbmux);
        let transport: Box<dyn LockdownTransport> = match device_id.transport() {
            Transport::Usb => Box::new(LockdownUsbTransport::connect(device_id.serial())?),
            Transport::Tcp(addr) => Box::new(LockdownTcpTransport::connect(addr)?),
        };
        Ok(Self {
            transport,
            session: None,
        })
    }

    fn send_plain<T: Serialize>(&mut self, msg: &T) -> Result<()> {
        anyhow::ensure!(self.session.is_none());
        let mut buf = vec![0; 4];
        plist::to_writer_xml(&mut buf, &msg)?;
        buf.push(b'\n');
        let len = buf.len() as u32 - 4;
        buf[..4].copy_from_slice(&len.to_be_bytes());
        self.transport.send(buf)
    }

    fn recv_plain<T>(&mut self) -> Result<T>
    where
        T: for<'a> Deserialize<'a>,
    {
        anyhow::ensure!(self.session.is_none());
        let bytes = self.transport.recv()?;
        let value = plist::from_reader_xml(&mut &bytes[..])?;
        Ok(value)
    }

    fn send<T: Serialize>(&mut self, msg: &T) -> Result<()> {
        anyhow::ensure!(self.session.is_some());
        let session = self.session.as_mut().unwrap();
        let mut buf = vec![0; 4];
        plist::to_writer_xml(&mut buf, &msg)?;
        buf.push(b'\n');
        let len = buf.len() as u32 - 4;
        buf[..4].copy_from_slice(&len.to_be_bytes());
        let mut w = session.tls.writer();
        w.write_all(&buf)?;
        while session.tls.wants_write() {
            let mut buf = vec![];
            session.tls.write_tls(&mut buf)?;
            self.transport.send(buf)?;
        }
        Ok(())
    }

    fn recv<T>(&mut self) -> Result<T>
    where
        T: for<'a> Deserialize<'a>,
    {
        anyhow::ensure!(self.session.is_some());
        let bytes = self.transport.recv()?;
        let session = self.session.as_mut().unwrap();
        session.tls.read_tls(&mut &bytes[..])?;
        let mut buf = vec![];
        while session.tls.wants_read() {
            let mut r = session.tls.reader();
            r.read_to_end(&mut buf)?;
        }
        Ok(plist::from_reader_xml(&mut &buf[..])?)
    }

    /*pub fn query_type(&mut self, label: &str) -> Result<String> {
        self.send(&plist!({
            "Request": "QueryType",
            "Label": label,
        }))?;
        self.recv()
    }*/

    pub fn get_value(&mut self) -> Result<plist::Value> {
        self.send(&plist!({
            "Request": "GetValue",
        }))?;
        self.recv()
    }

    /// Returns the session id.
    pub fn start_session(&mut self, config: Config) -> Result<()> {
        #[derive(Deserialize)]
        struct Resp {
            #[serde(rename = "SessionID")]
            session_id: String,
            #[serde(rename = "EnableSessionSSL")]
            enable_ssl: bool,
        }
        self.send_plain(&plist!({
            "Request": "StartSession",
            "HostID": config.host_id.as_str(),
            "SystemBUID": config.system_buid.as_str(),
        }))?;
        let resp = self.recv_plain::<Resp>()?;
        anyhow::ensure!(resp.enable_ssl);

        let mut session = Session::new(resp.session_id, config)?;
        while session.tls.is_handshaking() {
            if session.tls.wants_write() {
                let mut buf = vec![];
                session.tls.write_tls(&mut buf)?;
                self.transport.send(buf)?;
                continue;
            }
            if session.tls.wants_read() {
                let buf = self.transport.recv()?;
                session.tls.read_tls(&mut &buf[..])?;
            }
        }
        self.session = Some(session);
        Ok(())
    }
}

struct Session {
    id: String,
    tls: rustls::ClientConnection,
}

impl Session {
    pub fn new(id: String, config: Config) -> Result<Self> {
        let mut root_store = rustls::RootCertStore::empty();
        root_store.add(&config.root_certificate()?)?;
        let mut config = rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_root_certificates(root_store)
            .with_single_cert(vec![config.host_certificate()?], config.host_private_key()?)?;
        config.enable_tickets = false;
        config.enable_sni = false;
        let tls = rustls::ClientConnection::new(Arc::new(config), id.as_str().try_into()?)?;
        Ok(Self { id, tls })
    }
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
            Transport::Tcp("192.168.229.215:62078".parse()?),
        );
        let config = Config::from_uuid(device_id.serial())?;
        let mut lockdown = Lockdown::connect(&device_id)?;
        lockdown.start_session(config)?;
        let values = lockdown.get_value()?;
        println!("{:?}", values);
        Ok(())
    }

    #[test]
    fn test_lockdown_usb() -> Result<()> {
        env_logger::try_init().ok();
        let device = usb_devices()?.iter().next().unwrap();
        let device_id = DeviceId::new(device?.serial().into(), Protocol::Usbmux, Transport::Usb);
        let config = Config::from_uuid(device_id.serial())?;
        let mut lockdown = Lockdown::connect(&device_id)?;
        lockdown.start_session(config)?;
        let values = lockdown.get_value()?;
        println!("{:?}", values);
        Ok(())
    }
}
