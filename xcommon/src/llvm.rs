//! LLVM utilities

use anyhow::{bail, ensure, Context, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Returns the set of additional libraries that need to be bundled with
/// the given library, scanned recursively.
///
/// Any libraries in `provided_libs_paths` will be treated as available, without
/// being emitted. Any other library not in `search_paths` or `provided_libs_paths`
/// will result in an error.
pub fn list_needed_libs_recursively(
    lib: &Path,
    search_paths: &[&Path],
    provided_libs_paths: &[&Path],
) -> Result<HashSet<PathBuf>> {
    // Create a view of all libraries that are available on Android
    let mut provided = HashSet::new();
    for path in provided_libs_paths {
        for lib in find_libs_in_dir(path).with_context(|| {
            format!("Unable to list available libraries in `{}`", path.display())
        })? {
            // libc++_shared is bundled with the NDK but not available on-device
            if lib != "libc++_shared.so" {
                provided.insert(lib);
            }
        }
    }

    let mut to_copy = HashSet::new();

    let mut artifacts = vec![lib.to_path_buf()];
    while let Some(artifact) = artifacts.pop() {
        for need in list_needed_libs(&artifact).with_context(|| {
            format!(
                "Unable to read needed libraries from `{}`",
                artifact.display()
            )
        })? {
            // c++_shared is available in the NDK but not on-device.
            // Must be bundled with the apk if used:
            // https://developer.android.com/ndk/guides/cpp-support#libc
            let search_paths = if need == "libc++_shared.so" {
                provided_libs_paths
            } else {
                search_paths
            };

            if provided.insert(need.clone()) {
                if let Some(path) = find_library_path(search_paths, &need).with_context(|| {
                    format!(
                        "Could not iterate one or more search directories in `{:?}` while searching for library `{}`",
                        search_paths, need
                    )
                })? {
                    to_copy.insert(path.clone());
                    artifacts.push(path);
                } else {
                    bail!("Shared library `{}` not found", need);
                }
            }
        }
    }

    Ok(to_copy)
}

/// List all required shared libraries as per the dynamic section
fn list_needed_libs(library_path: &Path) -> Result<HashSet<String>> {
    let mut readelf = Command::new("llvm-readelf");
    let readelf = readelf.arg("--needed-libs").arg(library_path);
    let output = readelf
        .output()
        .with_context(|| format!("Failed to run `{:?}`", readelf))?;
    ensure!(
        output.status.success(),
        "Failed to run `{:?}`: {}",
        readelf,
        output.status
    );
    let output = std::str::from_utf8(&output.stdout).unwrap();
    let output = output.strip_prefix("NeededLibraries [\n").unwrap();
    let output = output.strip_suffix("]\n").unwrap();
    let mut needed = HashSet::new();

    for line in output.lines() {
        let lib = line.trim_start();
        needed.insert(lib.to_string());
    }
    Ok(needed)
}

/// List names of shared libraries inside directory
fn find_libs_in_dir(path: &Path) -> Result<HashSet<String>> {
    let mut libs = HashSet::new();
    let entries = std::fs::read_dir(path)?;
    for entry in entries {
        let entry = entry?;
        if !entry.path().is_dir() {
            if let Some(file_name) = entry.file_name().to_str() {
                if file_name.ends_with(".so") {
                    libs.insert(file_name.to_string());
                }
            }
        }
    }
    Ok(libs)
}

/// Resolves native library using search paths
fn find_library_path(paths: &[&Path], library: &str) -> Result<Option<PathBuf>> {
    for path in paths {
        let lib_path = path.join(library);
        if lib_path.exists() {
            return Ok(Some(dunce::canonicalize(lib_path)?));
        }
    }
    Ok(None)
}
