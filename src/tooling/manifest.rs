//! `pit.toml` data model. Lives in `tooling` (not `commands`) so `compile/`
//! can read a pod's manifest (for `[native]` resolution) without depending on
//! the command layer. `commands::utils` re-exports everything here, so
//! existing `commands::utils::Config` style references keep working.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
}
