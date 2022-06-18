use crate::cargo::CrateType;
use crate::devices::Device;
use crate::task::TaskRunner;
use crate::{BuildEnv, CompileTarget, Platform};
use anyhow::Result;
use std::process::Command;

//mod attach;
mod build;
mod doctor;
mod new;

//pub use attach::attach;
pub use build::build;
pub use doctor::doctor;
pub use new::new;

pub fn devices() -> Result<()> {
    for device_id in Device::list()? {
        let mut device = Device::connect(device_id)?;
        println!(
            "{:50}{:20}{:20}{}",
            device.id().to_string(),
            device.name()?,
            format_args!("{} {}", device.platform()?, device.arch()?),
            device.details()?,
        );
    }
    Ok(())
}

pub fn update(env: &BuildEnv) -> Result<()> {
    let mut runner = TaskRunner::new(3, env.verbose());

    runner.start_task("Update flutter");
    if let Some(flutter) = env.flutter() {
        flutter.git_pull()?;
        runner.end_verbose_task();
    }

    runner.start_task("Run pub upgrade");
    if let Some(flutter) = env.flutter() {
        flutter.pub_upgrade(env.root_dir())?;
        runner.end_verbose_task();
    }

    runner.start_task("Run cargo update");
    Command::new("cargo")
        .current_dir(env.root_dir())
        .arg("update")
        .status()?;
    runner.end_verbose_task();

    Ok(())
}

pub fn run(env: &BuildEnv, device: Option<Device>) -> Result<()> {
    let out = env.executable();
    if let Some(device) = device {
        device
            .run(&out, env.has_dart_code())?
            .attach(env.root_dir(), env.target_file())?;
    } else {
        anyhow::bail!("no device specified");
    }
    Ok(())
}

pub fn lldb(env: &BuildEnv, mut device: Option<Device>) -> Result<()> {
    if let Some(device) = device.as_mut() {
        let target = CompileTarget::new(device.platform()?, device.arch()?, env.target().opt());
        let cargo_dir = env
            .build_dir()
            .join(target.opt().to_string())
            .join(target.platform().to_string())
            .join(target.arch().to_string())
            .join("cargo");
        let executable = match target.platform() {
            Platform::Android => env.cargo_artefact(&cargo_dir, target, CrateType::Cdylib)?,
            Platform::Ios => env.output().join("main"),
            Platform::Linux => env.output().join(env.name()),
            Platform::Macos => env.executable(),
            Platform::Windows => todo!(),
        };
        let lldb_server = match target.platform() {
            Platform::Android => Some(env.lldb_server(target)?),
            _ => None,
        };
        device.lldb(&executable, lldb_server.as_deref())?;
    } else {
        anyhow::bail!("no device specified");
    }
    Ok(())
}
