use anyhow::{format_err, Context, Result};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::io::Write;

static HOST_NAME: &str = "crates.io";
static CARGO_MANIFEST_FILE_NAME: &str = "Cargo.toml";
static CARGO_LOCK_FILE_NAME: &str = "Cargo.lock";
static CRATES_IO_GIT_INDEX_SOURCE: &str = "registry+https://github.com/rust-lang/crates.io-index";
static CRATES_IO_SPARSE_INDEX_SOURCE: &str = "sparse+https://index.crates.io/";
static CRATES_IO_REGISTRY_INDEX_SOURCE: &str = "registry+https://index.crates.io/";

/// Dependencies resolved from a Rust project.
#[derive(Debug, Clone)]
pub struct FileDefinedDependencySet {
    /// Source file used to resolve dependency versions.
    pub path: std::path::PathBuf,

    /// Resolved crates.io dependencies.
    pub dependencies: Vec<thirdpass_core::extension::Dependency>,
}

#[derive(Debug, Clone)]
struct TargetPackage<'a> {
    name: &'a str,
    version: &'a str,
}

#[derive(Debug, Deserialize)]
struct CargoMetadata {
    packages: Vec<CargoPackage>,
    workspace_root: std::path::PathBuf,
}

#[derive(Debug, Deserialize)]
struct CargoPackage {
    name: String,
    version: String,
    source: Option<String>,
}

/// Return the host name for the default Rust package registry.
pub fn get_registry_host_name() -> String {
    HOST_NAME.to_string()
}

/// Resolve dependencies defined by the nearest Cargo project.
pub fn get_file_defined_dependencies(
    working_directory: &std::path::PathBuf,
) -> Result<Option<FileDefinedDependencySet>> {
    let manifest_path = match find_manifest_path(working_directory) {
        Some(path) => path,
        None => return Ok(None),
    };
    let metadata = match run_cargo_metadata(&manifest_path, true) {
        Ok(metadata) => metadata,
        Err(error) => {
            return Err(format_err!(
                "Failed to resolve Rust dependencies for {}. Rust dependency discovery requires a current Cargo.lock.\n{:#}",
                manifest_path.display(),
                error
            ));
        }
    };
    let path = get_source_path(&metadata, &manifest_path);
    let dependencies = crates_io_dependencies_from_metadata(&metadata, None);
    Ok(Some(FileDefinedDependencySet { path, dependencies }))
}

/// Resolve dependencies for a specific crates.io package release.
pub fn get_package_dependencies(
    package_name: &str,
    package_version: &str,
) -> Result<Vec<thirdpass_core::extension::Dependency>> {
    validate_crate_name(package_name)?;

    let tmp_dir = tempdir::TempDir::new("thirdpass_rs_identify_package_dependencies")?;
    let manifest_path = write_probe_project(tmp_dir.path(), package_name, package_version)?;
    let metadata = match run_cargo_metadata(&manifest_path, false) {
        Ok(metadata) => metadata,
        Err(error) => {
            return Err(format_err!(
                "Failed to resolve Rust package dependencies for {} {}.\n{:#}",
                package_name,
                package_version,
                error
            ));
        }
    };
    let target = TargetPackage {
        name: package_name,
        version: package_version,
    };
    Ok(crates_io_dependencies_from_metadata(
        &metadata,
        Some(&target),
    ))
}

fn find_manifest_path(working_directory: &std::path::PathBuf) -> Option<std::path::PathBuf> {
    let mut directory = working_directory.clone();
    loop {
        let manifest_path = directory.join(CARGO_MANIFEST_FILE_NAME);
        if manifest_path.is_file() {
            return Some(manifest_path);
        }
        if !directory.pop() {
            return None;
        }
    }
}

fn get_source_path(
    metadata: &CargoMetadata,
    manifest_path: &std::path::PathBuf,
) -> std::path::PathBuf {
    let lock_path = metadata.workspace_root.join(CARGO_LOCK_FILE_NAME);
    if lock_path.is_file() {
        lock_path
    } else {
        manifest_path.clone()
    }
}

fn run_cargo_metadata(manifest_path: &std::path::PathBuf, locked: bool) -> Result<CargoMetadata> {
    let mut command = std::process::Command::new("cargo");
    command
        .arg("metadata")
        .arg("--format-version")
        .arg("1")
        .arg("--manifest-path")
        .arg(manifest_path)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped());
    if locked {
        command.arg("--locked");
    }

    let output = command.output().context("Failed to run cargo metadata.")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format_err!(
            "cargo metadata failed for {}:\n{}",
            manifest_path.display(),
            stderr.trim()
        ));
    }

    serde_json::from_slice(&output.stdout).context("cargo metadata returned invalid JSON.")
}

fn write_probe_project(
    directory: &std::path::Path,
    package_name: &str,
    package_version: &str,
) -> Result<std::path::PathBuf> {
    let source_directory = directory.join("src");
    std::fs::create_dir_all(&source_directory)?;
    let mut lib_file = std::fs::File::create(source_directory.join("lib.rs"))?;
    lib_file.write_all(b"pub fn probe() {}\n")?;

    let manifest_path = directory.join(CARGO_MANIFEST_FILE_NAME);
    let manifest = format!(
        "[package]\n\
         name = \"thirdpass-rs-dependency-probe\"\n\
         version = \"0.0.0\"\n\
         edition = \"2018\"\n\
         publish = false\n\
         \n\
         [dependencies]\n\
         target-package = {{ package = \"{}\", version = \"={}\" }}\n",
        toml_basic_string(package_name),
        toml_basic_string(package_version),
    );
    let mut manifest_file = std::fs::File::create(&manifest_path)?;
    manifest_file.write_all(manifest.as_bytes())?;
    Ok(manifest_path)
}

fn crates_io_dependencies_from_metadata(
    metadata: &CargoMetadata,
    target: Option<&TargetPackage>,
) -> Vec<thirdpass_core::extension::Dependency> {
    let mut dependencies = BTreeSet::new();
    for package in &metadata.packages {
        if !package
            .source
            .as_deref()
            .map(is_crates_io_source)
            .unwrap_or_default()
        {
            continue;
        }
        if target
            .map(|target| package.name == target.name && package.version == target.version)
            .unwrap_or_default()
        {
            continue;
        }

        dependencies.insert(thirdpass_core::extension::Dependency {
            name: package.name.clone(),
            version: Ok(package.version.clone()),
        });
    }
    dependencies.into_iter().collect()
}

fn is_crates_io_source(source: &str) -> bool {
    source == CRATES_IO_GIT_INDEX_SOURCE
        || source.starts_with(CRATES_IO_SPARSE_INDEX_SOURCE)
        || source.starts_with(CRATES_IO_REGISTRY_INDEX_SOURCE)
}

fn validate_crate_name(package_name: &str) -> Result<()> {
    if package_name.is_empty() {
        return Err(format_err!("Crate name is empty."));
    }
    if package_name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        Ok(())
    } else {
        Err(format_err!("Invalid crate name: {}", package_name))
    }
}

fn toml_basic_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_selection_filters_to_crates_io() -> Result<()> {
        let metadata: CargoMetadata = serde_json::from_str(
            r#"{
                "workspace_root": "/tmp/project",
                "packages": [
                    {
                        "name": "workspace-crate",
                        "version": "0.1.0",
                        "source": null
                    },
                    {
                        "name": "serde",
                        "version": "1.0.0",
                        "source": "registry+https://github.com/rust-lang/crates.io-index"
                    },
                    {
                        "name": "internal",
                        "version": "1.0.0",
                        "source": "registry+https://example.com/index"
                    }
                ]
            }"#,
        )?;

        let dependencies = crates_io_dependencies_from_metadata(&metadata, None);

        assert_eq!(dependencies.len(), 1);
        assert_eq!(dependencies[0].name, "serde");
        assert_eq!(dependencies[0].version, Ok("1.0.0".to_string()));
        Ok(())
    }

    #[test]
    fn dependency_selection_excludes_target_package() -> Result<()> {
        let metadata: CargoMetadata = serde_json::from_str(
            r#"{
                "workspace_root": "/tmp/project",
                "packages": [
                    {
                        "name": "serde",
                        "version": "1.0.0",
                        "source": "registry+https://github.com/rust-lang/crates.io-index"
                    },
                    {
                        "name": "serde_derive",
                        "version": "1.0.0",
                        "source": "registry+https://github.com/rust-lang/crates.io-index"
                    }
                ]
            }"#,
        )?;
        let target = TargetPackage {
            name: "serde",
            version: "1.0.0",
        };

        let dependencies = crates_io_dependencies_from_metadata(&metadata, Some(&target));

        assert_eq!(dependencies.len(), 1);
        assert_eq!(dependencies[0].name, "serde_derive");
        Ok(())
    }

    #[test]
    fn find_manifest_walks_up_from_child_directory() -> Result<()> {
        let tmp_dir = tempdir::TempDir::new("thirdpass_rs_manifest_test")?;
        let child = tmp_dir.path().join("src").join("nested");
        std::fs::create_dir_all(&child)?;
        std::fs::write(tmp_dir.path().join(CARGO_MANIFEST_FILE_NAME), "[package]\n")?;

        let manifest_path = find_manifest_path(&child).expect("manifest path");

        assert_eq!(manifest_path, tmp_dir.path().join(CARGO_MANIFEST_FILE_NAME));
        Ok(())
    }

    #[test]
    fn file_defined_dependencies_resolve_current_project_from_child_directory() -> Result<()> {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let child = manifest_dir.join("src");

        let dependency_set =
            get_file_defined_dependencies(&child)?.expect("expected current Cargo project");

        assert_eq!(dependency_set.path, manifest_dir.join(CARGO_LOCK_FILE_NAME));
        assert!(
            dependency_set
                .dependencies
                .iter()
                .any(|dependency| dependency.name == "anyhow"
                    && matches!(&dependency.version, Ok(version) if !version.is_empty())),
            "expected resolved anyhow dependency in {:?}",
            dependency_set.dependencies
        );
        assert!(
            dependency_set
                .dependencies
                .iter()
                .any(|dependency| dependency.name == "serde"
                    && matches!(&dependency.version, Ok(version) if !version.is_empty())),
            "expected resolved serde dependency in {:?}",
            dependency_set.dependencies
        );
        assert!(
            !dependency_set
                .dependencies
                .iter()
                .any(|dependency| dependency.name == "thirdpass-core"),
            "path dependencies should not be reported as crates.io dependencies"
        );
        Ok(())
    }
}
