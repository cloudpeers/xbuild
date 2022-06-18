use crate::devices::{DeviceId, PartialRunner};
use crate::{Arch, Platform};
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug)]
pub(crate) struct IMobileDevice {
    id: DeviceId,
    ideviceinfo: PathBuf,
    ideviceinstaller: PathBuf,
    idevicedebug: PathBuf,
}

impl IMobileDevice {
    pub fn devices(devices: &mut Vec<DeviceId>) -> Result<()> {
        let output = Command::new(exe!("idevice_id"))
            .arg("-l")
            .arg("-d")
            .output()?;
        anyhow::ensure!(output.status.success(), "failed to run idevice_id");
        let lines = std::str::from_utf8(&output.stdout)?.lines();
        for uuid in lines {
            devices.push(DeviceId::Imd(uuid.trim().to_string()));
        }
        Ok(())
    }

    pub fn connect(device: String) -> Result<Self> {
        Ok(Self {
            id: DeviceId::Imd(device),
            ideviceinfo: which::which(exe!("ideviceinfo"))?,
            ideviceinstaller: which::which(exe!("ideviceinstaller"))?,
            idevicedebug: which::which(exe!("idevicedebug"))?,
        })
    }

    pub fn id(&self) -> &DeviceId {
        &self.id
    }

    fn device(&self) -> &str {
        if let DeviceId::Imd(id) = &self.id {
            id
        } else {
            unreachable!()
        }
    }

    fn getkey(&self, key: &str) -> Result<String> {
        let output = Command::new(&self.ideviceinfo)
            .arg("--udid")
            .arg(self.device())
            .arg("--key")
            .arg(key)
            .output()?;
        anyhow::ensure!(output.status.success(), "failed to run ideviceinfo");
        Ok(std::str::from_utf8(&output.stdout)?.trim().to_string())
    }

    fn install(&self, path: &Path) -> Result<()> {
        let status = Command::new(&self.ideviceinstaller)
            .arg("--udid")
            .arg(self.device())
            .arg("--install")
            .arg(path)
            .status()?;
        anyhow::ensure!(status.success(), "failed to run ideviceinstaller");
        Ok(())
    }

    fn start(&self, bundle_identifier: &str) -> Result<()> {
        let status = Command::new(&self.idevicedebug)
            .arg("--udid")
            .arg(self.device())
            .arg("run")
            .arg(bundle_identifier)
            .status()?;
        anyhow::ensure!(status.success(), "failed to run idevicedebug");
        Ok(())
    }

    pub fn run(&self, path: &Path, _flutter_attach: bool) -> Result<PartialRunner> {
        let bundle_identifier = appbundle::app_bundle_identifier(path)?;
        self.install(path)?;
        self.start(&bundle_identifier)?;
        // TODO: log, attach
        Ok(PartialRunner {
            url: None,
            logger: Box::new(|| unimplemented!()),
            child: None,
        })
    }

    pub fn name(&self) -> Result<String> {
        self.getkey("DeviceName")
    }

    pub fn platform(&self) -> Result<Platform> {
        Ok(Platform::Ios)
    }

    pub fn arch(&self) -> Result<Arch> {
        match self.getkey("CPUArchitecture")?.as_str() {
            "arm64" => Ok(Arch::Arm64),
            arch => anyhow::bail!("unsupported arch {}", arch),
        }
    }

    pub fn details(&self) -> Result<String> {
        let name = self.getkey("ProductName")?;
        let version = self.getkey("ProductVersion")?;
        Ok(format!("{} {}", name, version))
    }

    pub fn lldb(&self, _executable: &Path) -> Result<()> {
        anyhow::bail!("unimplemented");
    }
}
