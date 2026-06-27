use crate::tooling;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::{fs, path::Path, process};

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
}

pub fn default_entry() -> String {
    "src/main.liv".to_string()
}

pub fn workspace_root() -> std::path::PathBuf {
    let mut current_dir = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    let mut fallback = current_dir.clone();
    loop {
        let config_path = current_dir.join("pit.toml");
        if config_path.exists() {
            fallback = current_dir.clone();
            if let Ok(content) = fs::read_to_string(&config_path)
                && let Ok(val) = toml::from_str::<toml::Value>(&content)
                && val.get("workspace").is_some()
            {
                return current_dir;
            }
        }
        if let Some(parent) = current_dir.parent() {
            current_dir = parent.to_path_buf();
        } else {
            return fallback;
        }
    }
}

pub fn load_config() -> Config {
    let mut current_dir = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    loop {
        let config_path = current_dir.join("pit.toml");
        if config_path.exists() {
            if std::env::set_current_dir(&current_dir).is_err() {
                eprintln!(
                    "error: could not set working directory to {}",
                    current_dir.display()
                );
                process::exit(1);
            }
            let content = fs::read_to_string("pit.toml").unwrap();
            let config: Config = toml::from_str(&content).unwrap_or_else(|e| {
                eprintln!("error: invalid pit.toml: {}", e);
                process::exit(1);
            });

            if let Some(pod) = &config.pod
                && let Some(req_str) = &pod.olive
            {
                let req = semver::VersionReq::parse(req_str).unwrap_or_else(|e| {
                    eprintln!(
                        "error: invalid olive version requirement '{}': {}",
                        req_str, e
                    );
                    process::exit(1);
                });
                let current_version = semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
                if !req.matches(&current_version) {
                    eprintln!(
                        "error: this project requires olive {}, but you are using {}. Run 'pit upgrade' to update.",
                        req_str, current_version
                    );
                    process::exit(1);
                }
            }

            return config;
        }
        if let Some(parent) = current_dir.parent() {
            current_dir = parent.to_path_buf();
        } else {
            eprintln!("error: could not find `pit.toml` in this directory or any parent directory");
            process::exit(1);
        }
    }
}

pub fn save_config(config: &Config) {
    let content = toml::to_string(config).unwrap();
    fs::write("pit.toml", content).unwrap();
}

pub fn maybe_install_deps(deps: &HashMap<String, String>) {
    if deps.is_empty() {
        return;
    }
    let rt = tokio::runtime::Runtime::new().unwrap();
    if let Err(e) = rt.block_on(tooling::pods::ensure_deps_installed(deps, None, false)) {
        eprintln!("error: {}", e);
        process::exit(1);
    }
}

pub fn run_build_script(time: bool, release: bool) {
    if Path::new("build.liv").exists() {
        println!("\x1b[1;34mRunning\x1b[0m build.liv");
        crate::compile::compile_and_run("build.liv", true, time, false, false, release);
    }
}

pub fn aggregate_deps(config: &Config) -> HashMap<String, String> {
    let mut all_deps = config.dependencies.clone();
    if let Some(workspace) = &config.workspace {
        let root_dir = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
        for member in &workspace.members {
            let member_toml = root_dir.join(member).join("pit.toml");
            if let Ok(content) = fs::read_to_string(&member_toml)
                && let Ok(mc) = toml::from_str::<Config>(&content)
            {
                for (k, v) in mc.dependencies {
                    all_deps.insert(k, v);
                }
            }
        }
    }
    all_deps
}

#[cfg(test)]
pub(crate) static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_default_empty() {
        let cfg = Config::default();
        assert!(cfg.pod.is_none());
        assert!(cfg.dependencies.is_empty());
        assert!(cfg.workspace.is_none());
        assert!(cfg.profile.is_empty());
    }

    #[test]
    fn config_with_pod() {
        let cfg = Config {
            pod: Some(Pod {
                name: "my_app".into(),
                version: "0.1.0".into(),
                author: None,
                entry: "src/main.liv".into(),
                olive: Some(">=0.1".into()),
            }),
            dependencies: HashMap::new(),
            workspace: None,
            profile: HashMap::new(),
            fmt: None,
        };
        let toml_str = toml::to_string(&cfg).unwrap();
        assert!(toml_str.contains("my_app"));
        assert!(toml_str.contains("0.1.0"));
        assert!(toml_str.contains("src/main.liv"));

        let deserialized: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(deserialized.pod.unwrap().name, "my_app");
    }

    #[test]
    fn config_with_deps() {
        let mut deps = HashMap::new();
        deps.insert("serde".into(), "1.0".into());
        deps.insert("tokio".into(), "1.36".into());
        let cfg = Config {
            dependencies: deps,
            ..Config::default()
        };
        let toml_str = toml::to_string(&cfg).unwrap();
        assert!(toml_str.contains("serde"));
        assert!(toml_str.contains("tokio"));

        let deserialized: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(deserialized.dependencies.len(), 2);
        assert_eq!(deserialized.dependencies.get("serde").unwrap(), "1.0");
    }

    #[test]
    fn config_with_workspace() {
        let cfg = Config {
            workspace: Some(Workspace {
                members: vec!["lib_a".into(), "lib_b".into()],
            }),
            ..Config::default()
        };
        let toml_str = toml::to_string(&cfg).unwrap();
        assert!(toml_str.contains("lib_a"));

        let deserialized: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(deserialized.workspace.unwrap().members.len(), 2);
    }

    #[test]
    fn config_with_profile() {
        let mut profile = HashMap::new();
        profile.insert(
            "release".into(),
            Profile {
                opt_level: Some("3".into()),
            },
        );
        let cfg = Config {
            profile,
            ..Config::default()
        };
        let toml_str = toml::to_string(&cfg).unwrap();
        assert!(toml_str.contains("release"));

        let deserialized: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(
            deserialized.profile.get("release").unwrap().opt_level,
            Some("3".into())
        );
    }

    #[test]
    fn pod_default_entry() {
        let toml_str = r#"[pod]
name = "x"
version = "1.0"
"#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.pod.unwrap().entry, "src/main.liv");
    }

    #[test]
    fn pod_with_custom_entry() {
        let pod = Pod {
            name: "x".into(),
            version: "1.0".into(),
            author: None,
            entry: "lib.liv".into(),
            olive: None,
        };
        assert_eq!(pod.entry, "lib.liv");
    }

    #[test]
    fn pod_olive_req_roundtrip() {
        let cfg = Config {
            pod: Some(Pod {
                name: "test".into(),
                version: "0.0.1".into(),
                author: None,
                entry: "src/main.liv".into(),
                olive: Some(">=0.1.0".into()),
            }),
            ..Config::default()
        };
        let toml_str = toml::to_string(&cfg).unwrap();
        assert!(toml_str.contains("olive"));
        let deserialized: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(deserialized.pod.unwrap().olive, Some(">=0.1.0".into()));
    }

    #[test]
    fn aggregate_deps_no_workspace() {
        let mut deps = HashMap::new();
        deps.insert("dep_a".into(), "1.0".into());
        let cfg = Config {
            dependencies: deps.clone(),
            ..Config::default()
        };
        let result = aggregate_deps(&cfg);
        assert_eq!(result.len(), 1);
        assert_eq!(result.get("dep_a").unwrap(), "1.0");
    }

    #[test]
    fn aggregate_deps_with_workspace() {
        let _lock = CWD_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join("olive_utils_test_agg");
        let _ = fs::create_dir_all(&dir);
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();

        let member_dir = dir.join("member_a");
        fs::create_dir_all(&member_dir).unwrap();
        let member_config = r#"[dependencies]
member_dep = "0.5"
"#;
        fs::write(member_dir.join("pit.toml"), member_config).unwrap();

        let mut deps = HashMap::new();
        deps.insert("root_dep".into(), "1.0".into());
        let cfg = Config {
            dependencies: deps,
            workspace: Some(Workspace {
                members: vec!["member_a".into()],
            }),
            ..Config::default()
        };

        let result = aggregate_deps(&cfg);
        assert_eq!(result.len(), 2);
        assert_eq!(result.get("root_dep").unwrap(), "1.0");
        assert_eq!(result.get("member_dep").unwrap(), "0.5");

        std::env::set_current_dir(&cwd).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn aggregate_deps_member_missing_file_skipped() {
        let mut deps = HashMap::new();
        deps.insert("root_dep".into(), "1.0".into());
        let cfg = Config {
            dependencies: deps,
            workspace: Some(Workspace {
                members: vec!["nonexistent_member".into()],
            }),
            ..Config::default()
        };
        let result = aggregate_deps(&cfg);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn profile_opt_level_none() {
        let profile = Profile::default();
        assert!(profile.opt_level.is_none());
    }

    #[test]
    fn workspace_members_roundtrip() {
        let ws = Workspace {
            members: vec!["a".into(), "b".into(), "c".into()],
        };
        let toml_str = toml::to_string(&ws).unwrap();
        let deserialized: Workspace = toml::from_str(&toml_str).unwrap();
        assert_eq!(deserialized.members.len(), 3);
    }

    #[test]
    fn config_serde_roundtrip_full() {
        let mut deps = HashMap::new();
        deps.insert("log".into(), "0.4".into());
        let mut profile = HashMap::new();
        profile.insert(
            "dev".into(),
            Profile {
                opt_level: Some("0".into()),
            },
        );

        let cfg = Config {
            pod: Some(Pod {
                name: "full_test".into(),
                version: "2.0.0".into(),
                author: None,
                entry: "src/lib.liv".into(),
                olive: Some(">=0.2".into()),
            }),
            dependencies: deps,
            workspace: Some(Workspace {
                members: vec!["sub_crate".into()],
            }),
            profile,
            fmt: None,
        };

        let toml_str = toml::to_string(&cfg).unwrap();
        let deserialized: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(deserialized.pod.unwrap().name, "full_test");
        assert_eq!(deserialized.dependencies.len(), 1);
        assert!(deserialized.workspace.is_some());
        assert_eq!(deserialized.profile.len(), 1);
    }
}
