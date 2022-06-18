use anyhow::Result;
use clap::{Parser, Subcommand};
use xbuild::{command, BuildArgs, BuildEnv};

#[derive(Parser)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(subcommand)]
    command: Commands,
}

fn main() -> Result<()> {
    use tracing_subscriber::{fmt::format::FmtSpan, EnvFilter};
    tracing_log::LogTracer::init().ok();
    let env = std::env::var("XBUILD_LOG").unwrap_or_else(|_| "error".into());
    let subscriber = tracing_subscriber::FmtSubscriber::builder()
        .with_span_events(FmtSpan::ACTIVE | FmtSpan::CLOSE)
        .with_env_filter(EnvFilter::new(env))
        .with_writer(std::io::stderr)
        .finish();
    tracing::subscriber::set_global_default(subscriber).ok();
    log_panics::init();
    let args = Args::parse();
    args.command.run()
}

#[derive(Subcommand)]
enum Commands {
    /// Show information about the installed tooling
    Doctor,
    /// List all connected devices
    Devices,
    /// Creates a new flutter/rust project
    New {
        /// Project name
        name: String,
    },
    /// Updates the flutter sdk and cargo/pub dependencies
    Update {
        #[clap(flatten)]
        args: BuildArgs,
    },
    /// Build an executable app or install bundle
    Build {
        #[clap(flatten)]
        args: BuildArgs,
    },
    /// Run app on an attached device
    Run {
        #[clap(flatten)]
        args: BuildArgs,
    },
    /// Launch app in a debugger on an attached device
    Lldb {
        #[clap(flatten)]
        args: BuildArgs,
    },
    /*Attach {
        #[clap(long)]
        url: String,
        #[clap(long)]
        root_dir: PathBuf,
        #[clap(long)]
        target_file: PathBuf,
        #[clap(long)]
        host_vmservice_port: Option<u16>,
    },*/
}

impl Commands {
    pub fn run(self) -> Result<()> {
        match self {
            Self::Doctor => command::doctor(),
            Self::Devices => command::devices()?,
            Self::New { name } => command::new(&name)?,
            Self::Update { args } => {
                let (env, _device) = BuildEnv::new(args)?;
                command::update(&env)?;
            }
            Self::Build { args } => {
                let (env, _device) = BuildEnv::new(args)?;
                command::build(&env)?;
            }
            Self::Run { args } => {
                let (env, device) = BuildEnv::new(args)?;
                command::build(&env)?;
                command::run(&env, device)?;
            }
            Self::Lldb { args } => {
                let (env, device) = BuildEnv::new(args)?;
                command::build(&env)?;
                command::lldb(&env, device)?;
            } /*Self::Attach {
                  url,
                  root_dir,
                  target_file,
                  host_vmservice_port,
              } => command::attach(&url, &root_dir, &target_file, host_vmservice_port)?,*/
        }
        Ok(())
    }
}
