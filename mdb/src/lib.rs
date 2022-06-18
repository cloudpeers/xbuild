use anyhow::Result;
use async_io::Timer;
use futures::stream::{Stream, StreamExt};
use rsa::pkcs8::DecodePrivateKey;
use rsa::RsaPrivateKey;
use std::net::SocketAddr;
use std::pin::Pin;
use std::time::Duration;

mod adb;
mod lockdown;
mod usb;
mod usbmux;

pub use crate::adb::Adb;
pub use crate::lockdown::Lockdown;
pub use crate::usbmux::Usbmux;

pub fn adbkey() -> Result<RsaPrivateKey> {
    let home = dirs::home_dir().unwrap();
    Ok(RsaPrivateKey::read_pkcs8_pem_file(
        &home.join(".android/adbkey"),
    )?)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Protocol {
    Adb,
    Usbmux,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Transport {
    Usb,
    Tcp(SocketAddr),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceId {
    serial: String,
    protocol: Protocol,
    transport: Transport,
}

impl DeviceId {
    pub fn new(serial: String, protocol: Protocol, transport: Transport) -> Self {
        Self {
            serial,
            protocol,
            transport,
        }
    }

    pub fn serial(&self) -> &str {
        &self.serial
    }

    pub fn protocol(&self) -> Protocol {
        self.protocol
    }

    pub fn transport(&self) -> Transport {
        self.transport
    }
}

pub fn devices() -> Result<Vec<DeviceId>> {
    let mut devices = vec![];
    for device in usb::usb_devices()?.iter() {
        let device = device?;
        devices.push(DeviceId {
            serial: device.serial().into(),
            protocol: device.protocol(),
            transport: Transport::Usb,
        });
    }
    async_global_executor::block_on(async {
        let adb_stream = mdns::discover::all("_adb._tcp.local", Duration::from_secs(15))?
            .listen()
            .filter_map(|resp| async move {
                if let Ok(resp) = resp {
                    let serial = resp
                        .hostname()?
                        .split_once('.')?
                        .0
                        .split_once('-')?
                        .1
                        .to_string();
                    let transport = Transport::Tcp(resp.socket_address()?);
                    let protocol = Protocol::Adb;
                    Some(DeviceId {
                        serial,
                        protocol,
                        transport,
                    })
                } else {
                    None
                }
            });
        futures::pin_mut!(adb_stream);
        let usbmux_stream =
            mdns::discover::all("_apple-mobdev2._tcp.local", Duration::from_secs(15))?
                .listen()
                .filter_map(|resp| async move {
                    if let Ok(resp) = resp {
                        let any_addr = resp.ip_addr()?;
                        let addr = resp
                            .records()
                            .find_map(|rec| {
                                if let mdns::RecordKind::A(addr) = rec.kind {
                                    Some(addr.into())
                                } else {
                                    None
                                }
                            })
                            .unwrap_or(any_addr);
                        let serial = "unknown".to_string();
                        let transport = Transport::Tcp(SocketAddr::new(addr, 62078));
                        let protocol = Protocol::Usbmux;
                        Some(DeviceId {
                            serial,
                            protocol,
                            transport,
                        })
                    } else {
                        None
                    }
                });
        futures::pin_mut!(usbmux_stream);
        let stream = futures::stream::select_all([
            adb_stream as Pin<&mut dyn Stream<Item = DeviceId>>,
            usbmux_stream,
        ])
        .take_until(Timer::after(Duration::from_secs(1)));
        futures::pin_mut!(stream);
        while let Some(device) = stream.next().await {
            devices.push(device);
        }
        Ok::<_, anyhow::Error>(())
    })?;
    Ok(devices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discovery() -> Result<()> {
        env_logger::try_init().ok();
        println!("{:#?}", devices()?);
        Ok(())
    }
}
