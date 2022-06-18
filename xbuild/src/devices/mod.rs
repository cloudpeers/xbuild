use crate::devices::adb::Adb;
use crate::devices::host::Host;
use crate::devices::imd::IMobileDevice;
use crate::{Arch, Platform};
use anyhow::Result;
use std::path::Path;
use std::process::{Child, Command};

mod adb;
mod host;
mod imd;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeviceId {
    Host,
    Adb(String),
    Imd(String),
}

impl DeviceId {
    fn raw(&self) -> &str {
        match self {
            Self::Host => "host",
            Self::Adb(id) => id,
            Self::Imd(id) => id,
        }
    }
}

impl std::str::FromStr for DeviceId {
    type Err = anyhow::Error;

    fn from_str(device: &str) -> Result<Self> {
        if device == "host" {
            return Ok(Self::Host);
        }
        if let Some((backend, id)) = device.split_once(':') {
            Ok(match backend {
                "adb" => Self::Adb(id.to_string()),
                "imd" => Self::Imd(id.to_string()),
                _ => anyhow::bail!("unsupported backend {}", backend),
            })
        } else {
            anyhow::bail!("invalid device identifier {}", device);
        }
    }
}

impl std::fmt::Display for DeviceId {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Adb(id) => write!(f, "adb:{}", &id),
            Self::Host => write!(f, "host"),
            Self::Imd(id) => write!(f, "imd:{}", &id),
        }
    }
}

#[derive(Debug)]
pub struct Device(DeviceInner);

#[derive(Debug)]
enum DeviceInner {
    Host(Host),
    Adb(Adb),
    Imd(IMobileDevice),
}

impl Device {
    pub fn list() -> Result<Vec<DeviceId>> {
        let mut devices = vec![DeviceId::Host];
        Adb::devices(&mut devices)?;
        IMobileDevice::devices(&mut devices).ok();
        Ok(devices)
    }

    pub fn connect(device: DeviceId) -> Result<Self> {
        Ok(Device(match device {
            DeviceId::Host => DeviceInner::Host(Host),
            DeviceId::Adb(id) => DeviceInner::Adb(Adb::connect(id)?),
            DeviceId::Imd(id) => DeviceInner::Imd(IMobileDevice::connect(id)?),
        }))
    }

    pub fn id(&self) -> &DeviceId {
        match &self.0 {
            DeviceInner::Host(host) => host.id(),
            DeviceInner::Adb(adb) => adb.id(),
            DeviceInner::Imd(imd) => imd.id(),
        }
    }

    pub fn name(&mut self) -> Result<String> {
        match &mut self.0 {
            DeviceInner::Adb(adb) => adb.name(),
            DeviceInner::Host(host) => host.name(),
            DeviceInner::Imd(imd) => imd.name(),
        }
    }

    pub fn platform(&mut self) -> Result<Platform> {
        match &mut self.0 {
            DeviceInner::Adb(adb) => adb.platform(),
            DeviceInner::Host(host) => host.platform(),
            DeviceInner::Imd(imd) => imd.platform(),
        }
    }

    pub fn arch(&mut self) -> Result<Arch> {
        match &mut self.0 {
            DeviceInner::Adb(adb) => adb.arch(),
            DeviceInner::Host(host) => host.arch(),
            DeviceInner::Imd(imd) => imd.arch(),
        }
    }

    pub fn details(&mut self) -> Result<String> {
        match &mut self.0 {
            DeviceInner::Adb(adb) => adb.details(),
            DeviceInner::Host(host) => host.details(),
            DeviceInner::Imd(imd) => imd.details(),
        }
    }

    pub fn run(mut self, path: &Path, attach: bool) -> Result<Runner> {
        let runner = match &mut self.0 {
            DeviceInner::Adb(adb) => adb.run(path, attach, false),
            DeviceInner::Host(host) => host.run(path, attach),
            DeviceInner::Imd(imd) => imd.run(path, attach),
        }?;
        Ok(Runner::new(self, runner))
    }

    pub fn lldb(&mut self, executable: &Path, lldb_server: Option<&Path>) -> Result<()> {
        match &mut self.0 {
            DeviceInner::Adb(adb) => {
                if let Some(lldb_server) = lldb_server {
                    adb.lldb(executable, lldb_server)
                } else {
                    anyhow::bail!("lldb-server required on android");
                }
            }
            DeviceInner::Host(host) => host.lldb(executable),
            DeviceInner::Imd(imd) => imd.lldb(executable),
        }
    }

    pub fn attach(&mut self, url: &str, root_dir: &Path, target: &Path) -> Result<()> {
        let port = url
            .strip_prefix("http://127.0.0.1:")
            .unwrap()
            .split_once('/')
            .unwrap()
            .0
            .parse()?;
        let host_vmservice_port = match &mut self.0 {
            DeviceInner::Adb(adb) => Some(adb.forward(port)?),
            _ => None,
        };
        // TODO: finish porting flutter attach to rust
        //crate::command::attach(url, root_dir, target, host_vmservice_port).await?;
        let device = match &mut self.0 {
            DeviceInner::Host(host) => host.platform()?.to_string(),
            DeviceInner::Adb(adb) => adb.id().raw().to_string(),
            DeviceInner::Imd(imd) => imd.id().raw().to_string(),
        };
        let mut attach = Command::new("flutter");
        attach
            .current_dir(root_dir)
            .arg("attach")
            .arg("--device-id")
            .arg(device)
            .arg("--debug-url")
            .arg(url)
            .arg("--target")
            .arg(target);
        if let Some(port) = host_vmservice_port {
            attach.arg("--host-vmservice-port").arg(port.to_string());
        }
        attach.status()?;
        Ok(())
    }
}

pub(crate) struct PartialRunner {
    url: Option<String>,
    logger: Box<dyn FnOnce() + Send>,
    child: Option<Child>,
}

#[must_use]
pub struct Runner {
    device: Device,
    url: Option<String>,
    logger: Box<dyn FnOnce() + Send>,
    child: Option<Child>,
}

impl Runner {
    fn new(device: Device, runner: PartialRunner) -> Self {
        Self {
            device,
            url: runner.url,
            logger: runner.logger,
            child: runner.child,
        }
    }

    pub fn wait(self) {
        (self.logger)();
    }

    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    pub fn attach(mut self, root_dir: &Path, target: &Path) -> Result<()> {
        if let Some(url) = self.url.as_ref() {
            std::thread::spawn(self.logger);
            self.device.attach(url, root_dir, target)?;
        } else {
            self.wait();
        }
        Ok(())
    }

    pub fn kill(self) -> Result<()> {
        if let Some(mut child) = self.child {
            child.kill()?;
        }
        Ok(())
    }
}
