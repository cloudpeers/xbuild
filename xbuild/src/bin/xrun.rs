use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use xbuild::devices::{Device, DeviceId};

#[derive(Parser)]
pub struct Args {
    #[clap(long)]
    path: PathBuf,
    #[clap(long)]
    device: Option<DeviceId>,
}

fn main() -> Result<()> {
    use tracing_subscriber::{fmt::format::FmtSpan, EnvFilter};
    tracing_log::LogTracer::init().ok();
    let env = std::env::var(EnvFilter::DEFAULT_ENV).unwrap_or_else(|_| "error".into());
    let subscriber = tracing_subscriber::FmtSubscriber::builder()
        .with_span_events(FmtSpan::ACTIVE | FmtSpan::CLOSE)
        .with_env_filter(EnvFilter::new(env))
        .with_writer(std::io::stderr)
        .finish();
    tracing::subscriber::set_global_default(subscriber).ok();
    log_panics::init();
    let args = Args::parse();
    let device_id = args.device.unwrap_or(DeviceId::Host);
    let device = Device::connect(device_id)?;
    let runner = device.run(&args.path, true)?;
    if let Some(url) = runner.url() {
        println!("found url {}", url);
    }
    runner.kill()?;
    Ok(())
}
