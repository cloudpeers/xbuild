use crate::cargo::CrateType;
use crate::download::DownloadManager;
use crate::flutter::depfile::depfile_is_dirty;
use crate::task::TaskRunner;
use crate::{BuildEnv, Format, Opt, Platform};
use anyhow::{Context, Result};
use apk::Apk;
use appbundle::AppBundle;
use appimage::AppImage;
use msix::Msix;
use std::path::Path;
use xcommon::ZipFileOptions;

pub fn build(env: &BuildEnv) -> Result<()> {
    let platform_dir = env.platform_dir();
    std::fs::create_dir_all(&platform_dir)?;

    let mut num_tasks = 8;
    let mut build_classes_dex = false;
    if env.target().platform() == Platform::Android {
        num_tasks += 1;
        if !platform_dir.join("classes.dex").exists() {
            build_classes_dex = true;
        }
    }

    let mut runner = TaskRunner::new(num_tasks, env.verbose());

    runner.start_task("Fetch flutter repo");
    if let Some(flutter) = env.flutter() {
        if !flutter.root().exists() {
            if !env.offline() {
                flutter.git_clone()?;
                runner.end_task();
            } else {
                anyhow::bail!("flutter repo missing. Run `x` without `--offline`");
            }
        }
    }

    // if engine version changed clean

    runner.start_task("Fetch precompiled artefacts");
    let manager = DownloadManager::new(env)?;
    if !env.offline() {
        manager.prefetch(build_classes_dex)?;
        runner.end_verbose_task();
    }

    runner.start_task("Run pub get");
    if let Some(flutter) = env.flutter() {
        if !env.offline() {
            let package_config = env
                .root_dir()
                .join(".dart_tool")
                .join("package_config.json");
            let pubspec_stamp = env.build_dir().join("pubspec.stamp");
            if !package_config.exists() || xcommon::is_stamp_dirty(env.pubspec(), &pubspec_stamp)? {
                flutter.pub_get(env.root_dir())?;
                xcommon::create_stamp(&pubspec_stamp)?;
                runner.end_task();
            }
        }
    }

    runner.start_task("Build rust");
    let bin_target = env.target().platform() != Platform::Android
        && (env.flutter().is_none() || env.target().platform() != Platform::Ios);
    let has_lib = env.root_dir().join("src").join("lib.rs").exists();
    if bin_target || has_lib {
        for target in env.target().compile_targets() {
            let arch_dir = platform_dir.join(target.arch().to_string());
            let mut cargo = env.cargo_build(target, &arch_dir.join("cargo"))?;
            if !bin_target {
                cargo.arg("--lib");
            }
            cargo.exec()?;
        }
        runner.end_verbose_task();
    }

    if env.target().platform() == Platform::Android {
        runner.start_task("Build classes.dex");
        if let Some(flutter) = env.flutter() {
            if !platform_dir.join("classes.dex").exists() {
                let r8 = manager.r8()?;
                let deps = manager.flutter_embedding()?;
                flutter.build_classes_dex(env, &r8, deps)?;
                runner.end_task();
            }
        }
    }

    runner.start_task("Build flutter assets");
    let flutter_assets = env.build_dir().join("flutter_assets");
    if let Some(flutter) = env.flutter() {
        flutter.build_flutter_assets(env.root_dir(), &flutter_assets)?;
        runner.end_task();
    }

    runner.start_task("Build kernel_blob.bin");
    let kernel_blob = platform_dir.join("kernel_blob.bin");
    let kernel_blob_d = platform_dir.join("kernel_blob.bin.d");
    let mut aot_snapshot = false;
    if let Some(flutter) = env.flutter() {
        if !kernel_blob_d.exists() || depfile_is_dirty(&kernel_blob_d)? {
            aot_snapshot = true;
            flutter.kernel_blob_bin(
                env.root_dir(),
                env.target_file(),
                &kernel_blob,
                &kernel_blob_d,
                env.target().opt(),
            )?;
            runner.end_task();
        }
    }

    runner.start_task("Build aot snapshot");
    if let Some(flutter) = env.flutter() {
        if env.target().opt() == Opt::Release {
            for target in env.target().compile_targets() {
                let arch_dir = platform_dir.join(target.arch().to_string());
                let output =
                    if target.platform() == Platform::Macos || target.platform() == Platform::Ios {
                        arch_dir.join("App")
                    } else {
                        arch_dir.join("libapp.so")
                    };
                if aot_snapshot || !output.exists() {
                    std::fs::create_dir_all(&arch_dir)?;
                    let sdkroot = if target.platform() == Platform::Ios {
                        Some(env.ios_sdk())
                    } else {
                        None
                    };
                    flutter.aot_snapshot(
                        env.root_dir(),
                        &arch_dir,
                        &kernel_blob,
                        &output,
                        target,
                        sdkroot.as_deref(),
                    )?;
                }
            }
            runner.end_task();
        }
    }

    runner.start_task(format!("Create {}", env.target().format()));
    match env.target().platform() {
        Platform::Linux => {
            let target = env.target().compile_targets().next().unwrap();
            let arch_dir = platform_dir.join(target.arch().to_string());

            let appimage = AppImage::new(&arch_dir, env.name().to_string())?;
            appimage.add_apprun()?;
            appimage.add_desktop()?;
            if let Some(icon) = env.icon() {
                appimage.add_icon(icon)?;
            }

            if let Some(flutter) = env.flutter() {
                let engine_dir = flutter.engine_dir(target)?;
                appimage.add_file(
                    &flutter.icudtl_dat()?,
                    &Path::new("data").join("icudtl.dat"),
                )?;
                appimage.add_file(
                    &engine_dir.join("libflutter_linux_gtk.so"),
                    &Path::new("lib").join("libflutter_linux_gtk.so"),
                )?;
                appimage
                    .add_directory(&flutter_assets, &Path::new("data").join("flutter_assets"))?;
                match target.opt() {
                    Opt::Debug => {
                        appimage.add_file(
                            &platform_dir.join("kernel_blob.bin"),
                            &Path::new("data")
                                .join("flutter_assets")
                                .join("kernel_blob.bin"),
                        )?;
                    }
                    Opt::Release => {
                        appimage.add_file(
                            &arch_dir.join("libapp.so"),
                            &Path::new("lib").join("libapp.so"),
                        )?;
                    }
                }
            }

            let main = env.cargo_artefact(&arch_dir.join("cargo"), target, CrateType::Bin)?;
            appimage.add_file(&main, Path::new(env.name()))?;

            if has_lib {
                let lib = env.cargo_artefact(&arch_dir.join("cargo"), target, CrateType::Cdylib)?;
                appimage.add_file(&lib, &Path::new("lib").join(lib.file_name().unwrap()))?;
            }

            if env.target().format() == Format::Appimage {
                let out = arch_dir.join(format!("{}.AppImage", env.name()));
                appimage.build(&out, env.target().signer().cloned())?;
            }
        }
        Platform::Android => {
            let out = platform_dir.join(format!("{}.{}", env.name(), env.target().format()));

            let mut apk = Apk::new(
                out,
                env.manifest().android().clone(),
                env.target().opt() != Opt::Debug,
            )?;
            apk.add_res(env.icon(), &env.android_jar())?;
            if let Some(flutter) = env.flutter() {
                for target in env.target().compile_targets() {
                    apk.add_lib(
                        target.android_abi(),
                        &flutter.engine_dir(target)?.join("libflutter.so"),
                    )?;
                }
                apk.add_dex(&platform_dir.join("classes.dex"))?;
                apk.add_directory(&flutter_assets, &Path::new("assets").join("flutter_assets"))?;
                apk.add_file(
                    &flutter.vm_snapshot_data()?,
                    &Path::new("assets")
                        .join("flutter_assets")
                        .join("vm_snapshot_data"),
                    ZipFileOptions::Compressed,
                )?;
                apk.add_file(
                    &flutter.isolate_snapshot_data()?,
                    &Path::new("assets")
                        .join("flutter_assets")
                        .join("isolate_snapshot_data"),
                    ZipFileOptions::Compressed,
                )?;
                match env.target().opt() {
                    Opt::Debug => {
                        apk.add_file(
                            &platform_dir.join("kernel_blob.bin"),
                            &Path::new("assets")
                                .join("flutter_assets")
                                .join("kernel_blob.bin"),
                            ZipFileOptions::Compressed,
                        )?;
                    }
                    Opt::Release => {
                        for target in env.target().compile_targets() {
                            apk.add_lib(
                                target.android_abi(),
                                &platform_dir
                                    .join(target.arch().to_string())
                                    .join("libapp.so"),
                            )?;
                        }
                    }
                }
            }

            if has_lib {
                for target in env.target().compile_targets() {
                    let arch_dir = platform_dir.join(target.arch().to_string());
                    let cargo_dir = arch_dir.join("cargo");
                    let lib = env.cargo_artefact(&cargo_dir, target, CrateType::Cdylib)?;
                    apk.add_lib(target.android_abi(), &lib)?;

                    let ndk = env.android_ndk();

                    let deps_dir = {
                        let arch_dir = if target.is_host()? {
                            cargo_dir.to_path_buf()
                        } else {
                            cargo_dir.join(target.rust_triple()?)
                        };
                        let opt_dir = arch_dir.join(target.opt().to_string());
                        opt_dir.join("deps")
                    };

                    let mut search_paths = env
                        .cargo()
                        .lib_search_paths(&cargo_dir, target)
                        .with_context(|| {
                            format!(
                                "Finding libraries in `{}` for {:?}",
                                cargo_dir.display(),
                                target
                            )
                        })?;
                    search_paths.push(deps_dir);
                    let search_paths = search_paths.iter().map(AsRef::as_ref).collect::<Vec<_>>();

                    let ndk_sysroot_libs = ndk.join("usr/lib").join(target.ndk_triple());
                    let provided_libs_paths = [
                        ndk_sysroot_libs.as_path(),
                        &*ndk_sysroot_libs.join(
                            // Use libraries (symbols) from the lowest NDK that is supported by the application,
                            // to prevent inadvertently making newer APIs available:
                            // https://developer.android.com/ndk/guides/sdk-versions
                            env.manifest()
                                .android()
                                .sdk
                                .min_sdk_version
                                .unwrap()
                                .to_string(),
                        ),
                    ];

                    let extra_libs = xcommon::llvm::list_needed_libs_recursively(
                        &lib,
                        &search_paths,
                        &provided_libs_paths,
                    )
                    .with_context(|| {
                        format!(
                            "Failed to collect all required libraries for `{}` with `{:?}` available libraries and `{:?}` shippable libraries",
                            lib.display(),
                            provided_libs_paths,
                            search_paths
                        )
                    })?;
                    for lib in &extra_libs {
                        apk.add_lib(target.android_abi(), lib)?;
                    }
                }
            }

            apk.finish(env.target().signer().cloned())?;
        }
        Platform::Macos => {
            let target = env.target().compile_targets().next().unwrap();
            let arch_dir = platform_dir.join(target.arch().to_string());

            let mut app = AppBundle::new(&arch_dir, env.manifest().macos().clone())?;
            if let Some(icon) = env.icon() {
                app.add_icon(icon)?;
            }

            if let Some(flutter) = env.flutter() {
                app.add_framework(&flutter.engine_dir(target)?.join("FlutterMacOS.framework"))?;
                app.add_directory(&flutter_assets, Path::new("flutter_assets"))?;
                match target.opt() {
                    Opt::Debug => {
                        app.add_file(
                            &platform_dir.join("kernel_blob.bin"),
                            &Path::new("flutter_assets").join("kernel_blob.bin"),
                        )?;
                    }
                    Opt::Release => {
                        let framework_dir = arch_dir.join("App.framework");
                        let snapshot_dir = framework_dir.join("Versions").join("A");
                        let assets_dir = snapshot_dir.join("Resources").join("flutter_assets");
                        let current = framework_dir.join("Versions").join("Current");
                        if framework_dir.exists() {
                            std::fs::remove_dir_all(&framework_dir)?;
                        }
                        std::fs::create_dir_all(&snapshot_dir)?;
                        std::fs::create_dir_all(&assets_dir)?;
                        xcommon::copy_dir_all(&flutter_assets, &assets_dir)?;
                        std::fs::copy(arch_dir.join("App"), snapshot_dir.join("App"))?;
                        symlink::symlink_dir(Path::new("A"), current)?;
                        symlink::symlink_file(
                            Path::new("Versions").join("Current").join("App"),
                            framework_dir.join("App"),
                        )?;
                        symlink::symlink_dir(
                            Path::new("Versions").join("Current").join("Resources"),
                            framework_dir.join("Resources"),
                        )?;
                        std::fs::write(
                            snapshot_dir.join("Resources").join("Info.plist"),
                            include_bytes!("../../assets/Info.plist"),
                        )?;
                        app.add_framework(&framework_dir)?;
                    }
                }
            }

            let main = env.cargo_artefact(&arch_dir.join("cargo"), target, CrateType::Bin)?;
            app.add_executable(&main)?;

            if has_lib {
                let lib = env.cargo_artefact(&arch_dir.join("cargo"), target, CrateType::Cdylib)?;
                app.add_lib(&lib)?;
            }

            app.finish(env.target().signer().cloned())?;
            if let Some((key, issuer)) = env.target().notarization_key_and_issuer() {
                appbundle::notarize(app.appdir(), issuer, key)?;
            }
            if env.target().format() == Format::Dmg {
                let out = arch_dir.join(format!("{}.dmg", env.name()));
                dmg_oxide::create_dmg(app.appdir(), &out, env.name(), 0x40000)?;
                if let Some(signer) = env.target().signer() {
                    app.sign_dmg(&out, signer)?;
                    if let Some((key, issuer)) = env.target().notarization_key_and_issuer() {
                        appbundle::notarize(&out, issuer, key)?;
                    }
                }
            }
        }
        Platform::Ios => {
            let target = env.target().compile_targets().next().unwrap();
            let arch_dir = platform_dir.join(target.arch().to_string());
            std::fs::create_dir_all(&arch_dir)?;
            let mut app = AppBundle::new(&arch_dir, env.manifest().ios().clone())?;
            if let Some(icon) = env.icon() {
                app.add_icon(icon)?;
            }
            if let Some(flutter) = env.flutter() {
                let framework = flutter
                    .engine_dir(target)?
                    .join("Flutter.xcframework")
                    .join("ios-arm64_armv7")
                    .join("Flutter.framework");
                app.add_framework(&framework)?;

                let framework_dir = arch_dir.join("App.framework");
                let assets_dir = framework_dir.join("flutter_assets");
                std::fs::create_dir_all(&assets_dir)?;
                xcommon::copy_dir_all(&flutter_assets, &assets_dir)?;
                std::fs::write(
                    framework_dir.join("Info.plist"),
                    include_bytes!("../../assets/Info.plist"),
                )?;
                if target.opt() == Opt::Debug {
                    std::fs::copy(&kernel_blob, assets_dir.join("kernel_blob.bin"))?;
                    std::fs::copy(
                        flutter.vm_snapshot_data()?,
                        assets_dir.join("vm_snapshot_data"),
                    )?;
                    std::fs::copy(
                        flutter.isolate_snapshot_data()?,
                        assets_dir.join("isolate_snapshot_data"),
                    )?;
                    flutter.build_ios_empty_dylib(env, target)?;
                }
                std::fs::copy(arch_dir.join("App"), framework_dir.join("App"))?;
                app.add_framework(&arch_dir.join("App.framework"))?;

                let lib = if has_lib {
                    Some(env.cargo_artefact(
                        &arch_dir.join("cargo"),
                        target,
                        CrateType::Staticlib,
                    )?)
                } else {
                    None
                };

                flutter.build_ios_main(env, target, lib.as_deref())?;
                app.add_executable(&arch_dir.join("main"))?;
            } else {
                let main = env.cargo_artefact(&arch_dir.join("cargo"), target, CrateType::Bin)?;
                app.add_executable(&main)?;
            }

            app.add_provisioning_profile(env.target().provisioning_profile().unwrap())?;
            app.finish(env.target().signer().cloned())?;
        }
        Platform::Windows => {
            let target = env.target().compile_targets().next().unwrap();
            let arch_dir = platform_dir.join(target.arch().to_string());
            std::fs::create_dir_all(&arch_dir)?;
            let out = arch_dir.join(format!("{}.msix", env.name()));
            let mut msix = Msix::new(
                out,
                env.manifest().windows().clone(),
                target.opt() != Opt::Debug,
            )?;
            if let Some(icon) = env.icon() {
                msix.add_icon(icon)?;
            }
            // TODO: *.pri

            if let Some(flutter) = env.flutter() {
                let engine_dir = flutter.engine_dir(target)?;
                msix.add_file(
                    &flutter.icudtl_dat()?,
                    &Path::new("data").join("icudtl.dat"),
                    ZipFileOptions::Compressed,
                )?;
                msix.add_file(
                    &engine_dir.join("flutter_windows.dll"),
                    Path::new("flutter_windows.dll"),
                    ZipFileOptions::Compressed,
                )?;
                msix.add_directory(&flutter_assets, &Path::new("data").join("flutter_assets"))?;
                match target.opt() {
                    Opt::Debug => {
                        msix.add_file(
                            &platform_dir.join("kernel_blob.bin"),
                            &Path::new("data")
                                .join("flutter_assets")
                                .join("kernel_blob.bin"),
                            ZipFileOptions::Compressed,
                        )?;
                    }
                    Opt::Release => {
                        msix.add_file(
                            &arch_dir.join("libapp.so"),
                            &Path::new("data").join("app.so"),
                            ZipFileOptions::Compressed,
                        )?;
                    }
                }
            }
            let main = env.cargo_artefact(&arch_dir.join("cargo"), target, CrateType::Bin)?;
            msix.add_file(
                &main,
                format!("{}.exe", env.name()).as_ref(),
                ZipFileOptions::Compressed,
            )?;

            if has_lib {
                let lib = env.cargo_artefact(&arch_dir.join("cargo"), target, CrateType::Cdylib)?;
                msix.add_file(
                    &lib,
                    Path::new(lib.file_name().unwrap()),
                    ZipFileOptions::Compressed,
                )?;
            }

            msix.finish(env.target().signer().cloned())?;
        }
    }
    runner.end_task();

    Ok(())
}
