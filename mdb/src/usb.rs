use crate::{DeviceId, Protocol, Transport};
use anyhow::Result;
use rusb::{
    ConfigDescriptor, Device, DeviceDescriptor, DeviceHandle, DeviceList, Devices, Direction,
    GlobalContext, InterfaceDescriptor, TransferType, UsbContext,
};
use std::sync::Arc;
use std::time::Duration;

fn error(err: rusb::Error) -> anyhow::Error {
    if err == rusb::Error::Busy {
        anyhow::anyhow!("device busy, is adb server running? try running `adb kill-server`")
    } else {
        err.into()
    }
}

fn protocol(desc: &InterfaceDescriptor) -> Option<Protocol> {
    match (
        desc.class_code(),
        desc.sub_class_code(),
        desc.protocol_code(),
    ) {
        (0xff, 0x42, 0x1) => Some(Protocol::Adb),
        (0xff, 0xfe, 0x2) => Some(Protocol::Usbmux),
        _ => None,
    }
}

pub fn usb_devices() -> Result<UsbDeviceList> {
    let context = GlobalContext::default();
    //context.set_log_level(LogLevel::Debug);
    Ok(UsbDeviceList(context.devices()?))
}

pub struct UsbDeviceList(DeviceList<GlobalContext>);

impl UsbDeviceList {
    pub fn iter(&self) -> UsbDevices {
        UsbDevices(self.0.iter())
    }
}

pub struct UsbDevices<'a>(Devices<'a, GlobalContext>);

impl<'a> Iterator for UsbDevices<'a> {
    type Item = Result<UsbDevice>;

    fn next(&mut self) -> Option<Self::Item> {
        for device in self.0.by_ref() {
            if let Some(res) = UsbDevice::new(device).transpose() {
                return Some(res);
            }
        }
        None
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct UsbDevice {
    handle: DeviceHandle<GlobalContext>,
    serial: String,
    protocol: Protocol,
    config: u8,
    iface: u8,
    setting: u8,
    ep_read: u8,
    ep_write: u8,
}

impl UsbDevice {
    fn new(device: Device<GlobalContext>) -> Result<Option<Self>> {
        let device_desc = device.device_descriptor()?;
        let config_desc = device.active_config_descriptor()?;
        if let Some(device) = Self::new_with_config(&device, &device_desc, &config_desc)? {
            return Ok(Some(device));
        }
        for i in 0..device_desc.num_configurations() {
            let config_desc = device.config_descriptor(i)?;
            if let Some(device) = Self::new_with_config(&device, &device_desc, &config_desc)? {
                return Ok(Some(device));
            }
        }
        Ok(None)
    }

    fn new_with_config(
        device: &Device<GlobalContext>,
        device_desc: &DeviceDescriptor,
        config_desc: &ConfigDescriptor,
    ) -> Result<Option<UsbDevice>> {
        for iface in config_desc.interfaces() {
            for iface_desc in iface.descriptors() {
                if let Some(protocol) = protocol(&iface_desc) {
                    let ep_read = iface_desc
                        .endpoint_descriptors()
                        .filter(|ep| ep.transfer_type() == TransferType::Bulk)
                        .filter(|ep| ep.direction() == Direction::In)
                        .map(|ep| ep.address())
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("invalid endpoint"))?;
                    let ep_write = iface_desc
                        .endpoint_descriptors()
                        .filter(|ep| ep.transfer_type() == TransferType::Bulk)
                        .filter(|ep| ep.direction() == Direction::Out)
                        .map(|ep| ep.address())
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("invalid endpoint"))?;
                    let handle = device.open().map_err(error)?;
                    let serial = handle.read_serial_number_string_ascii(&device_desc)?;
                    return Ok(Some(Self {
                        handle,
                        serial,
                        protocol,
                        config: config_desc.number(),
                        iface: iface_desc.interface_number(),
                        setting: iface_desc.setting_number(),
                        ep_read,
                        ep_write,
                    }));
                }
            }
        }
        Ok(None)
    }

    fn open(serial: &str, protocol: Protocol) -> Result<Self> {
        let mut device = usb_devices()?
            .iter()
            .filter_map(|res| res.ok())
            .find(|dev| dev.serial == serial && dev.protocol == protocol)
            .ok_or_else(|| anyhow::anyhow!("device with serial {} not found", serial))?;
        device.handle.reset()?;
        device.handle.detach_kernel_driver(device.iface).ok();
        if device.handle.active_configuration()? != device.config {
            device
                .handle
                .set_active_configuration(device.config)
                .map_err(error)?;
        }
        device.handle.claim_interface(device.iface).map_err(error)?;
        device
            .handle
            .set_alternate_setting(device.iface, device.setting)
            .map_err(error)?;
        log::debug!("opened device {}", serial);
        Ok(device)
    }
}

impl From<UsbDevice> for DeviceId {
    fn from(device: UsbDevice) -> Self {
        DeviceId::new(device.serial, device.protocol, Transport::Usb)
    }
}

#[derive(Clone)]
pub struct UsbDeviceHandle(Arc<UsbDevice>);

impl UsbDeviceHandle {
    pub fn new(device: UsbDevice) -> Self {
        Self(Arc::new(device))
    }

    pub fn open(serial: &str, protocol: Protocol) -> Result<Self> {
        Ok(Self::new(UsbDevice::open(serial, protocol)?))
    }

    pub fn serial(&self) -> &str {
        self.0.serial.as_str()
    }

    pub fn protocol(&self) -> Protocol {
        self.0.protocol
    }

    pub fn send(&self, buf: &[u8]) -> Result<usize> {
        Ok(self
            .0
            .handle
            .write_bulk(self.0.ep_write, buf, Duration::from_millis(0))?)
    }

    pub fn recv(&self, buf: &mut [u8]) -> Result<usize> {
        Ok(self
            .0
            .handle
            .read_bulk(self.0.ep_read, buf, Duration::from_millis(0))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_usb_connect() -> Result<()> {
        let devices = usb_devices()?.iter().collect::<Result<Vec<_>>>()?;
        assert_eq!(devices.len(), 1);
        let device = UsbDeviceHandle::open(&devices[0].serial, Protocol::Adb)?;
        println!("{:?}", device);
        Ok(())
    }
}
