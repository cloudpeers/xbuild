use self::assets::AssetBundle;
use crate::{task, Arch, BuildEnv, CompileTarget, Opt, Platform};
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::Command;

mod android;
pub mod artifacts;
pub mod assets;
pub mod attach;
pub mod depfile;
mod ios;

pub struct Flutter {
    git: PathBuf,
    repo: PathBuf,
    cache: PathBuf,
    verbose: bool,
}

impl Flutter {
    pub fn new(repo: PathBuf, cache: PathBuf, verbose: bool) -> Result<Self> {
        let git = which::which("git")?;
        Ok(Self {
            git,
            repo,
            cache,
            verbose,
        })
    }

    pub fn root(&self) -> &Path {
        &self.repo
    }

    pub fn version(&self) -> Result<String> {
        let output = Command::new(&self.git)
            .current_dir(self.root())
            .arg("tag")
            .arg("--points-at")
            .arg("HEAD")
            .output()?;
        anyhow::ensure!(output.status.success(), "failed to get flutter version");
        let version = std::str::from_utf8(&output.stdout)?;
        Ok(version.to_string())
    }

    pub fn git_clone(&self) -> Result<()> {
        let clone_dir = self.repo.parent().unwrap();
        std::fs::create_dir_all(&clone_dir)?;
        let mut cmd = Command::new(&self.git);
        cmd.current_dir(&clone_dir)
            .arg("clone")
            .arg("https://github.com/flutter/flutter")
            .arg("--depth")
            .arg("1")
            .arg("--branch")
            .arg("stable");
        task::run(cmd, self.verbose)?;
        Ok(())
    }

    pub fn git_pull(&self) -> Result<()> {
        let flutter = self.root();
        if !flutter.exists() {
            self.git_clone()?
        } else {
            let mut cmd = Command::new(&self.git);
            cmd.current_dir(&flutter)
                .arg("pull")
                .arg("origin")
                .arg("stable");
            task::run(cmd, true)?;
        }
        Ok(())
    }

    fn artifact_version(&self, artifact: &str) -> Result<String> {
        let path = self
            .root()
            .join("bin")
            .join("internal")
            .join(format!("{}.version", artifact));
        anyhow::ensure!(
            path.exists(),
            "failed to locate engine.version at {}",
            path.display()
        );
        Ok(std::fs::read_to_string(path)?.trim().into())
    }

    pub fn engine_version(&self) -> Result<String> {
        self.artifact_version("engine")
    }

    pub fn material_fonts_version(&self) -> Result<String> {
        Ok(self
            .artifact_version("material_fonts")?
            .split('/')
            .nth(3)
            .unwrap()
            .to_string())
    }

    pub fn engine_dir(&self, target: CompileTarget) -> Result<PathBuf> {
        let path = self
            .cache
            .join("engine")
            .join(self.engine_version()?)
            .join(target.opt().to_string())
            .join(target.platform().to_string())
            .join(target.arch().to_string());
        Ok(path)
    }

    fn host_file(&self, path: &Path) -> Result<PathBuf> {
        let host = CompileTarget::new(Platform::host()?, Arch::host()?, Opt::Debug);
        let path = self.engine_dir(host)?.join(path);
        anyhow::ensure!(path.exists(), "failed to locate {}", path.display());
        Ok(path)
    }

    pub fn material_fonts(&self) -> Result<PathBuf> {
        let dir = self.cache.join("material_fonts");
        let version = self.material_fonts_version()?;
        Ok(dir.join(version))
    }

    pub fn icudtl_dat(&self) -> Result<PathBuf> {
        self.host_file(Path::new("icudtl.dat"))
    }

    pub fn isolate_snapshot_data(&self) -> Result<PathBuf> {
        self.host_file(Path::new("isolate_snapshot.bin"))
    }

    pub fn vm_snapshot_data(&self) -> Result<PathBuf> {
        self.host_file(Path::new("vm_isolate_snapshot.bin"))
    }

    pub fn dart(&self) -> Result<Command> {
        let path = Path::new("dart-sdk").join("bin").join(exe!("dart"));
        Ok(Command::new(self.host_file(&path)?))
    }

    pub fn dart_pub(&self, root_dir: &Path) -> Result<Command> {
        let flutter_root = self.root();
        let version = self.version()?;
        std::fs::write(flutter_root.join("version"), version)?;
        let pkg_dir = flutter_root.join("bin").join("cache").join("pkg");
        std::fs::create_dir_all(&pkg_dir)?;
        let src_dir = self.host_file(Path::new("sky_engine"))?;
        let dest_dir = pkg_dir.join("sky_engine");
        if dest_dir.exists() {
            symlink::remove_symlink_dir(&dest_dir)?;
        }
        symlink::symlink_dir(&src_dir, &dest_dir)?;
        let mut cmd = self.dart()?;
        cmd.current_dir(root_dir)
            .env("FLUTTER_ROOT", flutter_root)
            .arg("pub");
        Ok(cmd)
    }

    pub fn pub_get(&self, root_dir: &Path) -> Result<()> {
        let mut cmd = self.dart_pub(root_dir)?;
        cmd.arg("get").arg("--no-precompile");
        task::run(cmd, self.verbose)?;
        Ok(())
    }

    pub fn pub_upgrade(&self, root_dir: &Path) -> Result<()> {
        let mut cmd = self.dart_pub(root_dir)?;
        cmd.arg("upgrade").arg("--no-precompile");
        task::run(cmd, true)?;
        Ok(())
    }

    pub fn build_flutter_assets(&self, root_dir: &Path, flutter_assets: &Path) -> Result<()> {
        let bundle = AssetBundle::new(root_dir, &self.material_fonts()?)?;
        bundle.assemble(flutter_assets)?;
        Ok(())
    }

    pub fn kernel_blob_bin(
        &self,
        root_dir: &Path,
        target_file: &Path,
        output: &Path,
        depfile: &Path,
        opt: Opt,
    ) -> Result<()> {
        let mut cmd = self.dart()?;
        cmd.current_dir(root_dir)
            .arg(self.host_file(Path::new("frontend_server.dart.snapshot"))?)
            .arg("--target=flutter")
            .arg("--no-print-incremental-dependencies")
            .arg("--packages")
            .arg(".packages")
            .arg("--output-dill")
            .arg(output)
            .arg("--depfile")
            .arg(depfile);
        match opt {
            Opt::Release => {
                cmd.arg("--sdk-root")
                    .arg(self.host_file(Path::new("flutter_patched_sdk_product"))?)
                    .arg("-Ddart.vm.profile=false")
                    .arg("-Ddart.vm.product=true")
                    .arg("--aot")
                    .arg("--tfa");
            }
            Opt::Debug => {
                cmd.arg("--sdk-root")
                    .arg(self.host_file(Path::new("flutter_patched_sdk"))?)
                    .arg("-Ddart.vm.profile=false")
                    .arg("-Ddart.vm.product=false")
                    .arg("--track-widget-creation");
            }
        }
        cmd.arg(target_file);
        task::run(cmd, self.verbose)?;
        Ok(())
    }

    pub fn aot_snapshot(
        &self,
        root_dir: &Path,
        build_dir: &Path,
        kernel_blob_bin: &Path,
        snapshot: &Path,
        target: CompileTarget,
        sdkroot: Option<&Path>,
    ) -> Result<()> {
        let engine_dir = self.engine_dir(target)?;
        let gen_snapshot = if target.platform() == Platform::Ios {
            engine_dir.join(exe!("gen_snapshot_arm64"))
        } else {
            engine_dir.join(exe!("gen_snapshot"))
        };
        let mut cmd = Command::new(gen_snapshot);
        cmd.current_dir(root_dir)
            .arg("--deterministic")
            .arg("--strip");
        if target.platform() == Platform::Ios || target.platform() == Platform::Macos {
            let file_name = snapshot.file_name().unwrap().to_str().unwrap();
            let assembly = build_dir.join("snapshot.S");
            cmd.arg("--snapshot_kind=app-aot-assembly")
                .arg(format!("--assembly={}", assembly.display()))
                .arg(kernel_blob_bin);
            task::run(cmd, self.verbose)?;
            let object = build_dir.join("snapshot.o");
            let arch = match target.arch() {
                Arch::X64 => "x86_64",
                Arch::Arm64 => "arm64",
            };
            let mut cmd = Command::new("clang");
            cmd.arg("-c")
                .arg(assembly)
                .arg("-o")
                .arg(&object)
                .arg("-arch")
                .arg(arch);
            if target.platform() == Platform::Ios {
                cmd.arg("-miphoneos-version-min=9.0");
                if let Some(sdkroot) = sdkroot {
                    cmd.env("SDKROOT", &sdkroot)
                        .arg(format!("--sysroot={}", sdkroot.display()));
                }
            }
            task::run(cmd, self.verbose)?;
            let mut cmd = Command::new("clang");
            cmd.arg("-arch")
                .arg(arch)
                .arg("-dynamiclib")
                .arg("-Xlinker")
                .arg("-rpath")
                .arg("-Xlinker")
                .arg("@executable_path/Frameworks")
                .arg("-Xlinker")
                .arg("-rpath")
                .arg("-Xlinker")
                .arg("@loader_path/Frameworks")
                .arg("-install_name")
                .arg(format!("@rpath/{name}.framework/{name}", name = file_name))
                .arg("-o")
                .arg(snapshot)
                .arg(object);
            if target.platform() == Platform::Ios {
                cmd.arg("-miphoneos-version-min=9.0");
                if let Some(sdkroot) = sdkroot {
                    cmd.env("SDKROOT", &sdkroot)
                        .arg(format!("--sysroot={}", sdkroot.display()));
                }
            }
            task::run(cmd, self.verbose)?;
        } else {
            cmd.arg("--snapshot_kind=app-aot-elf")
                .arg(format!("--elf={}", snapshot.display()))
                .arg(kernel_blob_bin);
            task::run(cmd, self.verbose)?;
        }
        Ok(())
    }

    pub fn build_classes_dex(&self, env: &BuildEnv, r8: &Path, deps: Vec<PathBuf>) -> Result<()> {
        android::build_classes_dex(env, r8, deps)
    }

    pub fn build_ios_main(
        &self,
        env: &BuildEnv,
        target: CompileTarget,
        lib: Option<&Path>,
    ) -> Result<()> {
        ios::build_ios_main(env, self, target, lib)
    }

    pub fn build_ios_empty_dylib(&self, env: &BuildEnv, target: CompileTarget) -> Result<()> {
        ios::build_empty_dylib(env, target)
    }
}
