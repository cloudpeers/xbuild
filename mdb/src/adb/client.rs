use crate::adb::packet::{AdbPacket, Command};
use crate::adb::transport::{AdbTcpTransport, AdbTransport, AdbUsbTransport};
use crate::{DeviceId, Protocol, Transport};
use anyhow::Result;
use rsa::{Hash, PaddingScheme, RsaPrivateKey, RsaPublicKey};

const VERSION: u32 = 0x0100_0000;
const MAX_DATA: u32 = 0x10_0000;

pub struct Adb {
    transport: Box<dyn AdbTransport>,
}

impl Adb {
    pub fn connect(private_key: &RsaPrivateKey, device_id: &DeviceId) -> Result<Self> {
        anyhow::ensure!(device_id.protocol() == Protocol::Adb);
        let mut transport: Box<dyn AdbTransport> = match device_id.transport() {
            Transport::Usb => Box::new(AdbUsbTransport::connect(device_id.serial())?),
            Transport::Tcp(addr) => Box::new(AdbTcpTransport::connect(addr)?),
        };

        transport.send(AdbPacket::connect(VERSION, MAX_DATA, "host::"))?;
        let mut auth = 0;
        loop {
            let packet = transport.recv()?;
            match packet.command() {
                Command::Connect => {
                    let device_id = String::from_utf8_lossy(packet.payload());
                    log::debug!(
                        "handshake ok: device_id = {}, version = 0x{:x}, max_data = 0x{:x}",
                        device_id,
                        packet.arg0(),
                        packet.arg1(),
                    );
                    break;
                }
                Command::Auth => match auth {
                    0 => {
                        let padding = PaddingScheme::new_pkcs1v15_sign(Some(Hash::SHA1));
                        let signature = private_key.sign(padding, packet.payload())?;
                        transport.send(AdbPacket::auth_signature(signature))?;
                        auth += 1;
                    }
                    1 => {
                        let public_key = RsaPublicKey::from(private_key);
                        transport.send(AdbPacket::auth_rsa_public_key(public_key))?;
                        auth += 2;
                    }
                    _ => {
                        anyhow::bail!("authentication failed");
                    }
                },
                cmd => {
                    anyhow::bail!("unexpected command {:?}", cmd);
                }
            }
        }

        Ok(Self { transport })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adbkey;
    use crate::usb::usb_devices;

    #[test]
    fn test_client_tcp() -> Result<()> {
        env_logger::try_init().ok();
        let private_key = adbkey()?;
        let device = DeviceId::new(
            "16ee50bc".into(),
            Protocol::Adb,
            Transport::Tcp("192.168.2.43:5555".parse()?),
        );
        let _conn = Adb::connect(&private_key, &device)?;
        Ok(())
    }

    #[test]
    fn test_client_usb() -> Result<()> {
        env_logger::try_init().ok();
        let private_key = adbkey()?;
        for device in usb_devices()?.iter() {
            let device = DeviceId::new(device?.serial().into(), Protocol::Adb, Transport::Usb);
            let _conn = Adb::connect(&private_key, &device)?;
        }
        Ok(())
    }
}
