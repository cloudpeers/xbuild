use crate::{Arch, BuildEnv, CompileTarget, Opt, Platform};
use anyhow::Result;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use mvn::Download;
use reqwest::blocking::Client;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use tar::{Archive, EntryType};
use zstd::Decoder;

pub struct DownloadManager<'a> {
    env: &'a BuildEnv,
    client: Client,
}

impl<'a> Download for DownloadManager<'a> {
    fn download(&self, url: &str, dest: &Path) -> Result<()> {
        let pb = ProgressBar::with_draw_target(0, ProgressDrawTarget::stdout())
        .with_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} {prefix:.bold} [{elapsed}] {wide_bar:.green} {bytes}/{total_bytes} {msg}")
                .progress_chars("█▇▆▅▄▃▂▁  ")
        );
        let file_name = dest.file_name().unwrap().to_str().unwrap().to_string();
        pb.set_prefix(file_name);
        pb.set_message("📥 downloading");

        let mut resp = self.client.get(url).send()?;
        anyhow::ensure!(
            resp.status().is_success(),
            "GET {} returned status code {}",
            url,
            resp.status()
        );
        let len = resp.content_length().unwrap_or_default();
        pb.set_length(len);

        let dest = BufWriter::new(File::create(&dest)?);
        std::io::copy(&mut resp, &mut pb.wrap_write(dest))?;
        pb.finish_with_message("📥 downloaded");

        Ok(())
    }
}

impl<'a> Download for &'a DownloadManager<'a> {
    fn download(&self, url: &str, dest: &Path) -> Result<()> {
        (*self).download(url, dest)
    }
}

impl<'a> DownloadManager<'a> {
    pub fn new(env: &'a BuildEnv) -> Result<Self> {
        let client = Client::new();
        let download_dir = env.cache_dir().join("download");
        std::fs::create_dir_all(&download_dir)?;
        Ok(Self { env, client })
    }

    pub(crate) fn env(&self) -> &BuildEnv {
        self.env
    }

    pub(crate) fn fetch(&self, item: WorkItem) -> Result<()> {
        if item.output.exists() {
            return Ok(());
        }
        let name = item.url.rsplit_once('/').unwrap().1;
        let result: Result<()> = (|| {
            if name.ends_with(".tar.zst") {
                let archive = self.env().cache_dir().join("download").join(name);
                self.download(&item.url, &archive)?;
                let archive = BufReader::new(File::open(&archive)?);
                let mut archive = Archive::new(Decoder::new(archive)?);
                let dest = item.output.parent().unwrap();
                std::fs::create_dir_all(&dest)?;
                for entry in archive.entries()? {
                    let mut entry = entry?;
                    if item.no_symlinks && entry.header().entry_type() == EntryType::Symlink {
                        continue;
                    }
                    if item.no_colons && entry.header().path()?.to_str().unwrap().contains(':') {
                        continue;
                    }
                    entry.unpack_in(&dest)?;
                }
            } else if name.ends_with(".framework.zip") {
                let download_dir = self.env().cache_dir().join("download");
                let archive = download_dir.join(name);
                self.download(&item.url, &archive)?;
                let framework_dir = download_dir.join("framework");
                xcommon::extract_zip(&archive, &framework_dir)?;
                let archive = framework_dir.join(name);
                std::fs::create_dir_all(&item.output)?;
                xcommon::extract_zip(&archive, &item.output)?;
            } else if name.ends_with(".zip") {
                let archive = self.env().cache_dir().join("download").join(name);
                self.download(&item.url, &archive)?;
                xcommon::extract_zip(&archive, item.output.parent().unwrap())?;
            } else {
                self.download(&item.url, &item.output)?;
            }
            Ok(())
        })();
        if result.is_err() {
            if item.output.is_dir() {
                std::fs::remove_dir_all(&item.output).ok();
            } else {
                std::fs::remove_file(&item.output).ok();
            }
        }
        result
    }

    pub fn prefetch(&self, build_classes_dex: bool) -> Result<()> {
        match self.env().target().platform() {
            Platform::Linux if Platform::host()? != Platform::Linux => {
                anyhow::bail!("cross compiling to linux is not yet supported");
            }
            Platform::Windows if Platform::host()? != Platform::Windows => {
                self.windows_sdk()?;
            }
            Platform::Macos if Platform::host()? != Platform::Macos => {
                self.macos_sdk()?;
            }
            Platform::Android => {
                self.android_ndk()?;
                self.android_jar()?;
            }
            Platform::Ios => {
                self.ios_sdk()?;
            }
            _ => {}
        }
        if self.env.flutter().is_some() {
            let host = CompileTarget::new(Platform::host()?, Arch::host()?, Opt::Debug);
            for target in self
                .env
                .target()
                .compile_targets()
                .chain(std::iter::once(host))
            {
                self.flutter_engine(target)?;
            }
            self.material_fonts()?;
            if build_classes_dex && self.env().target().platform() == Platform::Android {
                self.r8()?;
                self.flutter_embedding()?;
            }
        }
        Ok(())
    }
}

pub struct WorkItem {
    url: String,
    output: PathBuf,
    no_symlinks: bool,
    no_colons: bool,
}

impl WorkItem {
    pub fn new(output: PathBuf, url: String) -> Self {
        Self {
            url,
            output,
            no_symlinks: false,
            no_colons: false,
        }
    }

    /// The windows sdk contains symlinks for case sensitive
    /// filesystems. on case sensitive file systems skip the
    /// symlinks
    pub fn no_symlinks(&mut self) -> &mut Self {
        self.no_symlinks = true;
        self
    }

    /// the macos sdk contains man pages. man pages contain
    /// colons in the file names. on windows it's an invalid
    /// file name character, so we skip file names with colons.
    pub fn no_colons(&mut self) -> &mut Self {
        self.no_colons = true;
        self
    }
}

impl WorkItem {
    pub fn github_release(
        output: PathBuf,
        org: &str,
        name: &str,
        version: &str,
        artifact: &str,
    ) -> Self {
        Self::new(
            output,
            format!(
                "https://github.com/{}/{}/releases/download/{}/{}",
                org, name, version, artifact
            ),
        )
    }
}

impl<'a> DownloadManager<'a> {
    pub fn android_jar(&self) -> Result<()> {
        let dir = self.env.android_sdk();
        let sdk = self.env.target_sdk_version();
        let path = dir
            .join("platforms")
            .join(format!("android-{}", sdk))
            .join("android.jar");
        if !path.exists() {
            let package = format!("platforms;android-{}", sdk);
            android_sdkmanager::download_and_extract_packages(
                dir.to_str().unwrap(),
                android_sdkmanager::HostOs::Linux,
                &[&package],
                Some(&[android_sdkmanager::MatchType::EntireName("android.jar")]),
            )
        }
        Ok(())
    }

    pub fn windows_sdk(&self) -> Result<()> {
        let output = self.env.windows_sdk();
        let mut item =
            WorkItem::github_release(output, "cloudpeers", "x", "v0.1.0+2", "Windows.sdk.tar.zst");
        if !cfg!(target_os = "linux") {
            item.no_symlinks();
        }
        self.fetch(item)
    }

    pub fn macos_sdk(&self) -> Result<()> {
        let output = self.env.macos_sdk();
        let mut item =
            WorkItem::github_release(output, "cloudpeers", "x", "v0.1.0+2", "MacOSX.sdk.tar.zst");
        if cfg!(target_os = "windows") {
            item.no_colons();
        }
        self.fetch(item)
    }

    pub fn android_ndk(&self) -> Result<()> {
        let output = self.env.android_ndk();
        let item =
            WorkItem::github_release(output, "cloudpeers", "x", "v0.1.0+2", "Android.ndk.tar.zst");
        self.fetch(item)
    }

    pub fn ios_sdk(&self) -> Result<()> {
        let output = self.env.ios_sdk();
        let mut item = WorkItem::github_release(
            output,
            "cloudpeers",
            "x",
            "v0.1.0+2",
            "iPhoneOS.sdk.tar.zst",
        );
        if cfg!(target_os = "windows") {
            item.no_colons();
        }
        self.fetch(item)
    }
}
