//! `pit.toml` data model. Lives in `tooling` (not `commands`) so `compile/`
//! can read a pod's manifest (for `[native]` resolution) without depending on
//! the command layer. `commands::utils` re-exports everything here, so
//! existing `commands::utils::Config` style references keep working.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pod: Option<Pod>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub dependencies: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<Workspace>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub profile: HashMap<String, Profile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fmt: Option<FmtConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<Native>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct FmtConfig {
    #[serde(default)]
    pub max_width: Option<usize>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct Workspace {
    pub members: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct Profile {
    #[serde(default)]
    pub opt_level: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct Pod {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default = "default_entry")]
    pub entry: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub olive: Option<String>,
    /// Extra files or directories, relative to the pod root, to add to the
    /// published archive beyond `pit.toml` and `src/**/*.liv`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
}

pub fn default_entry() -> String {
    "src/main.liv".to_string()
}

pub fn safe_relative_path(value: &str, field: &str) -> Result<PathBuf, String> {
    let path = Path::new(value);
    if path.as_os_str().is_empty() {
        return Err(format!("{field} must not be empty"));
    }
    let mut relative = PathBuf::new();
    for component in path.components() {
        let Component::Normal(name) = component else {
            return Err(format!(
                "{field} '{value}' must be a relative path without '.' or '..' components"
            ));
        };
        if !crate::tooling::safe_archive_component(name) {
            return Err(format!(
                "{field} '{value}' contains an unsafe path component"
            ));
        }
        relative.push(name);
    }
    Ok(relative)
}

pub fn resolve_existing_within(root: &Path, value: &str, field: &str) -> Result<PathBuf, String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", root.display()))?;
    let relative = safe_relative_path(value, field)?;
    let resolved = root
        .join(relative)
        .canonicalize()
        .map_err(|error| format!("cannot resolve {field} '{value}': {error}"))?;
    if !resolved.starts_with(&root) {
        return Err(format!(
            "{field} '{value}' escapes project root {}",
            root.display()
        ));
    }
    Ok(resolved)
}

pub fn resolve_file_within(root: &Path, value: &str, field: &str) -> Result<PathBuf, String> {
    let resolved = resolve_existing_within(root, value, field)?;
    if !resolved.is_file() {
        return Err(format!("{field} '{value}' must name a regular file"));
    }
    Ok(resolved)
}

pub fn validate_native_layout(native: &Native) -> Result<(), String> {
    let lib = safe_relative_path(&native.lib, "native library name")?;
    if lib
        .parent()
        .is_some_and(|parent| !parent.as_os_str().is_empty())
    {
        return Err("native library name must be a bare filename stem".to_string());
    }
    safe_relative_path(native.artifact_dir(), "native artifact directory")?;
    if native
        .build
        .as_ref()
        .is_some_and(|argv| argv.first().is_none_or(|program| program.is_empty()))
    {
        return Err("native build command must name a program".to_string());
    }
    Ok(())
}

pub fn validate_pod_layout(config: &Config, root: &Path) -> Result<(), String> {
    if let Some(pod) = &config.pod {
        crate::tooling::registry::validate_pod_name(&pod.name)?;
        resolve_file_within(root, &pod.entry, "pod entry")?;
        for include in &pod.include {
            resolve_existing_within(root, include, "pod include")?;
        }
    }
    if let Some(native) = &config.native {
        validate_native_layout(native)?;
    }
    Ok(())
}

/// A pod's native (non-Olive) shared library, built out of band from the
/// pod's own sources and published as target-specific release assets rather
/// than packed into the `.pit.zst`.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
pub struct Native {
    /// Library stem: "tokenizer" names `libtokenizer.so` / `.dylib` / `.dll`.
    pub lib: String,
    /// Argv used to build the library locally, run directly with no shell.
    /// Defaults to `["cargo", "build", "--release"]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<Vec<String>>,
    /// Directory the build command leaves the artifact in, relative to the
    /// pod root. Defaults to `"target/release"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    /// Target keys (see `tooling::target::SUPPORTED`) this pod publishes a
    /// native artifact for. Defaults to every target pit supports; `pit
    /// publish` fails if any listed target's artifact is missing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<String>,
}

impl Native {
    pub fn build_argv(&self) -> Vec<String> {
        self.build
            .clone()
            .unwrap_or_else(|| vec!["cargo".into(), "build".into(), "--release".into()])
    }

    pub fn artifact_dir(&self) -> &str {
        self.dir.as_deref().unwrap_or("target/release")
    }

    pub fn target_keys(&self) -> Vec<String> {
        if self.targets.is_empty() {
            crate::tooling::target::SUPPORTED
                .iter()
                .map(|s| s.to_string())
                .collect()
        } else {
            self.targets.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_default_has_no_native_table() {
        let cfg = Config::default();
        assert!(cfg.native.is_none());
    }

    #[test]
    fn native_defaults_build_argv_to_cargo_release() {
        let native = Native {
            lib: "tokenizer".into(),
            ..Native::default()
        };
        assert_eq!(
            native.build_argv(),
            vec![
                "cargo".to_string(),
                "build".to_string(),
                "--release".to_string()
            ]
        );
    }

    #[test]
    fn native_defaults_artifact_dir() {
        let native = Native {
            lib: "tokenizer".into(),
            ..Native::default()
        };
        assert_eq!(native.artifact_dir(), "target/release");
    }

    #[test]
    fn native_defaults_target_keys_to_all_supported() {
        let native = Native {
            lib: "tokenizer".into(),
            ..Native::default()
        };
        assert_eq!(native.target_keys(), crate::tooling::target::SUPPORTED);
    }

    #[test]
    fn native_explicit_targets_are_not_widened() {
        let native = Native {
            lib: "tokenizer".into(),
            targets: vec!["linux-x86_64".into()],
            ..Native::default()
        };
        assert_eq!(native.target_keys(), vec!["linux-x86_64".to_string()]);
    }

    #[test]
    fn native_custom_build_and_dir_round_trip_through_toml() {
        let cfg = Config {
            native: Some(Native {
                lib: "tokenizer".into(),
                build: Some(vec!["make".into(), "release".into()]),
                dir: Some("out".into()),
                targets: vec![],
            }),
            ..Config::default()
        };
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        let native = back.native.unwrap();
        assert_eq!(
            native.build_argv(),
            vec!["make".to_string(), "release".to_string()]
        );
        assert_eq!(native.artifact_dir(), "out");
    }

    #[test]
    fn pod_include_defaults_empty_and_round_trips() {
        let cfg = Config {
            pod: Some(Pod {
                name: "x".into(),
                version: "1.0".into(),
                author: None,
                entry: default_entry(),
                olive: None,
                include: vec!["README.md".into()],
            }),
            ..Config::default()
        };
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back.pod.unwrap().include, vec!["README.md".to_string()]);
    }

    #[test]
    fn old_pit_toml_without_native_or_include_still_parses() {
        let s = r#"[pod]
name = "x"
version = "1.0"

[dependencies]
foo = "1.0"
"#;
        let cfg: Config = toml::from_str(s).unwrap();
        assert!(cfg.native.is_none());
        assert!(cfg.pod.unwrap().include.is_empty());
    }

    #[test]
    fn safe_relative_paths_reject_escape_and_special_components() {
        for path in ["", "/tmp/lib.liv", "../lib.liv", "src/../../lib.liv", "CON"] {
            assert!(
                safe_relative_path(path, "pod entry").is_err(),
                "accepted unsafe path {path:?}"
            );
        }
        assert_eq!(
            safe_relative_path("src/lib.liv", "pod entry").unwrap(),
            PathBuf::from("src").join("lib.liv")
        );
    }

    #[test]
    fn native_layout_rejects_escaping_names_and_directories() {
        let native = Native {
            lib: "../outside".into(),
            ..Native::default()
        };
        assert!(validate_native_layout(&native).is_err());

        let native = Native {
            lib: "tokenizer".into(),
            dir: Some("../outside".into()),
            ..Native::default()
        };
        assert!(validate_native_layout(&native).is_err());
    }
}
