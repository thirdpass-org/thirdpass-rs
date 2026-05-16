use anyhow::{format_err, Context, Result};
use std::io::Read;

mod cargo;

/// Rust package registry extension backed by crates.io and Cargo metadata.
#[derive(Clone, Debug)]
pub struct RsExtension {
    name_: String,
    registry_host_names_: Vec<String>,
}

impl thirdpass_core::extension::FromLib for RsExtension {
    fn new() -> Self {
        Self {
            name_: "rs".to_string(),
            registry_host_names_: vec![cargo::get_registry_host_name()],
        }
    }
}

impl thirdpass_core::extension::Extension for RsExtension {
    fn name(&self) -> String {
        self.name_.clone()
    }

    fn registries(&self) -> Vec<String> {
        self.registry_host_names_.clone()
    }

    fn review_target_policy(&self) -> thirdpass_core::extension::ReviewTargetPolicy {
        thirdpass_core::extension::ReviewTargetPolicy {
            excluded_exact_paths: vec![
                ".cargo_vcs_info.json".to_string(),
                "Cargo.lock".to_string(),
            ],
        }
    }

    /// Returns resolved dependencies for a crates.io package release.
    fn identify_package_dependencies(
        &self,
        package_name: &str,
        package_version: &Option<&str>,
        _extension_args: &[String],
    ) -> Result<Vec<thirdpass_core::extension::PackageDependencies>> {
        let package_version = match package_version {
            Some(version) => version.to_string(),
            None => get_latest_version(package_name)?
                .ok_or(format_err!("Failed to find latest package version."))?,
        };
        let dependencies = cargo::get_package_dependencies(package_name, &package_version)?;
        Ok(vec![thirdpass_core::extension::PackageDependencies {
            package_version: Ok(package_version),
            registry_host_name: cargo::get_registry_host_name(),
            dependencies,
        }])
    }

    fn identify_file_defined_dependencies(
        &self,
        working_directory: &std::path::Path,
        _extension_args: &[String],
    ) -> Result<Vec<thirdpass_core::extension::FileDefinedDependencies>> {
        let dependency_set = match cargo::get_file_defined_dependencies(working_directory)? {
            Some(dependency_set) => dependency_set,
            None => return Ok(Vec::new()),
        };

        Ok(vec![thirdpass_core::extension::FileDefinedDependencies {
            path: dependency_set.path,
            registry_host_name: cargo::get_registry_host_name(),
            dependencies: dependency_set.dependencies,
        }])
    }

    fn registries_package_metadata(
        &self,
        package_name: &str,
        package_version: &Option<&str>,
    ) -> Result<Vec<thirdpass_core::extension::RegistryPackageMetadata>> {
        let entry_json = get_registry_entry_json(package_name)?;
        let package_version = match package_version {
            Some(version) => {
                if !registry_entry_has_version(&entry_json, version) {
                    return Err(format_err!(
                        "Package version not found in crates.io registry: {} {}",
                        package_name,
                        version
                    ));
                }
                version.to_string()
            }
            None => get_latest_version_from_entry_json(&entry_json)
                .ok_or(format_err!("Failed to find latest package version."))?,
        };

        let registry_host_name = self
            .registries()
            .first()
            .ok_or(format_err!(
                "Code error: vector of registry host names is empty."
            ))?
            .clone();
        let human_url = get_registry_human_url(package_name, &package_version)?;
        let artifact_url = get_archive_url(package_name, &package_version)?;

        Ok(vec![thirdpass_core::extension::RegistryPackageMetadata {
            registry_host_name,
            human_url: human_url.to_string(),
            artifact_url: artifact_url.to_string(),
            is_primary: true,
            package_version,
        }])
    }
}

fn get_latest_version(package_name: &str) -> Result<Option<String>> {
    let entry_json = get_registry_entry_json(package_name)?;
    Ok(get_latest_version_from_entry_json(&entry_json))
}

fn get_latest_version_from_entry_json(registry_entry_json: &serde_json::Value) -> Option<String> {
    let crate_entry = registry_entry_json.get("crate")?;
    for field in &["max_stable_version", "max_version", "newest_version"] {
        if let Some(version) = crate_entry.get(field).and_then(|value| value.as_str()) {
            if !version.is_empty() {
                return Some(version.to_string());
            }
        }
    }
    None
}

fn registry_entry_has_version(registry_entry_json: &serde_json::Value, version: &str) -> bool {
    registry_entry_json["versions"]
        .as_array()
        .map(|versions| {
            versions
                .iter()
                .any(|entry| entry["num"].as_str() == Some(version))
        })
        .unwrap_or_default()
}

fn get_registry_entry_json(package_name: &str) -> Result<serde_json::Value> {
    let url = format!("https://crates.io/api/v1/crates/{}", package_name);
    let client = reqwest::blocking::Client::builder()
        .user_agent(format!("thirdpass-rs/{}", env!("CARGO_PKG_VERSION")))
        .build()?;
    let mut result = client.get(&url).send()?.error_for_status()?;
    let mut body = String::new();
    result.read_to_string(&mut body)?;

    Ok(serde_json::from_str(&body).context(format!("JSON was not well-formatted:\n{}", body))?)
}

fn get_registry_human_url(package_name: &str, package_version: &str) -> Result<url::Url> {
    Ok(url::Url::parse(&format!(
        "https://crates.io/crates/{}/{}",
        package_name, package_version
    ))?)
}

fn get_archive_url(package_name: &str, package_version: &str) -> Result<url::Url> {
    Ok(url::Url::parse(&format!(
        "https://static.crates.io/crates/{}/{}-{}.crate",
        package_name, package_name, package_version
    ))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use thirdpass_core::extension::{Extension, FromLib};

    #[test]
    fn latest_version_prefers_stable_version() {
        let registry_entry_json = serde_json::json!({
            "crate": {
                "max_stable_version": "1.2.3",
                "max_version": "2.0.0-alpha.1",
                "newest_version": "2.0.0-alpha.1"
            }
        });

        assert_eq!(
            get_latest_version_from_entry_json(&registry_entry_json),
            Some("1.2.3".to_string())
        );
    }

    #[test]
    fn registry_entry_version_lookup_matches_version_numbers() {
        let registry_entry_json = serde_json::json!({
            "versions": [
                { "num": "1.0.0" },
                { "num": "1.1.0" }
            ]
        });

        assert!(registry_entry_has_version(&registry_entry_json, "1.1.0"));
        assert!(!registry_entry_has_version(&registry_entry_json, "1.2.0"));
    }

    #[test]
    fn registry_urls_match_crates_io_routes() -> Result<()> {
        assert_eq!(
            get_registry_human_url("serde", "1.0.0")?.as_str(),
            "https://crates.io/crates/serde/1.0.0"
        );
        assert_eq!(
            get_archive_url("serde", "1.0.0")?.as_str(),
            "https://static.crates.io/crates/serde/serde-1.0.0.crate"
        );
        Ok(())
    }

    #[test]
    fn review_target_policy_skips_generated_cargo_metadata() {
        let policy = RsExtension::new().review_target_policy();

        assert!(policy.excludes_exact_path(".cargo_vcs_info.json"));
        assert!(policy.excludes_exact_path("Cargo.lock"));
        assert!(!policy.excludes_exact_path("Cargo.toml"));
    }
}
