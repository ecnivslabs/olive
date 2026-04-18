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
            if let Ok(content) = fs::read_to_string(&config_path) {
                if let Ok(val) = toml::from_str::<toml::Value>(&content) {
                    if val.get("workspace").is_some() {
                        return current_dir;
                    }
                }
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

            if let Some(pod) = &config.pod {
                if let Some(req_str) = &pod.olive {
                    let req = semver::VersionReq::parse(req_str).unwrap_or_else(|e| {
                        eprintln!(
                            "error: invalid olive version requirement '{}': {}",
                            req_str, e
                        );
                        process::exit(1);
                    });
                    let current_version =
                        semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
                    if !req.matches(&current_version) {
                        eprintln!(
                            "error: this project requires olive {}, but you are using {}. Run 'pit upgrade' to update.",
                            req_str, current_version
                        );
                        process::exit(1);
                    }
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
            if let Ok(content) = fs::read_to_string(&member_toml) {
                if let Ok(mc) = toml::from_str::<Config>(&content) {
                    for (k, v) in mc.dependencies {
                        all_deps.insert(k, v);
                    }
                }
            }
        }
    }
    all_deps
}
