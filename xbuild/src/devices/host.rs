use crate::devices::{DeviceId, PartialRunner};
use crate::{Arch, Platform};
use anyhow::Result;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Clone, Debug)]
pub(crate) struct Host;

impl Host {
    pub fn id(&self) -> &DeviceId {
        &DeviceId::Host
    }

    pub fn name(&self) -> Result<String> {
        if cfg!(target_os = "linux") {
            let output = Command::new("uname").output()?;
            anyhow::ensure!(output.status.success(), "uname failed");
            let name = std::str::from_utf8(&output.stdout)?.trim();
            Ok(name.to_string())
        } else {
            Ok("host".to_string())
        }
    }

    pub fn platform(&self) -> Result<Platform> {
        Platform::host()
    }

    pub fn arch(&self) -> Result<Arch> {
        Arch::host()
    }

    pub fn details(&self) -> Result<String> {
        if cfg!(target_os = "linux") {
            let os_release = std::fs::read_to_string("/etc/os-release")?;
            let mut distro = os_release
                .lines()
                .filter_map(|line| line.split_once('='))
                .filter(|(k, _)| *k == "NAME")
                .map(|(_, v)| v.trim_matches('"').to_string())
                .next()
                .unwrap_or_default();
            let output = Command::new("uname").arg("-r").output()?;
            anyhow::ensure!(output.status.success(), "uname failed");
            distro.push(' ');
            distro.push_str(std::str::from_utf8(&output.stdout)?.trim());
            Ok(distro)
        } else {
            Ok("".to_string())
        }
    }

    pub fn run(&self, path: &Path, flutter_attach: bool) -> Result<PartialRunner> {
        let mut child = Command::new(path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()?;
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let url = if flutter_attach {
            let url = loop {
                if let Some(line) = lines.next() {
                    let line = line?;
                    let line = line.trim();
                    if let Some((_, url)) = line.rsplit_once(' ') {
                        if url.starts_with("http://127.0.0.1") {
                            break url.trim().to_string();
                        }
                    }
                    println!("{}", line);
                }
            };
            Some(url)
        } else {
            None
        };
        Ok(PartialRunner {
            url,
            logger: Box::new(move || {
                for line in lines.flatten() {
                    println!("{}", line.trim());
                }
            }),
            child: Some(child),
        })
    }

    pub fn lldb(&self, executable: &Path) -> Result<()> {
        Command::new("lldb").arg(executable).status()?;
        Ok(())
    }
}
