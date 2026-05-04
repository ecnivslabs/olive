use super::utils::{load_config, save_config};
use crate::tooling;
use std::process;

fn parse_versioned_pod(pod: &str) -> (String, String) {
    if let Some((n, v)) = pod.split_once('@') {
        (n.to_string(), v.to_string())
    } else {
        (pod.to_string(), "latest".to_string())
    }
}

pub fn execute_add(pod: &str) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (name, version_req) = parse_versioned_pod(pod);

    let versions = rt
        .block_on(tooling::registry::fetch_versions(&name, false))
        .unwrap_or_else(|e| {
            eprintln!("error: {}", e);
            process::exit(1);
        });

    let pkg = tooling::registry::resolve_version(&versions, &version_req).unwrap_or_else(|| {
        eprintln!("error: no matching version for '{}@{}'", name, version_req);
        process::exit(1);
    });

    let resolved_version = pkg.vers.clone();

    let mut config = load_config();
    config
        .dependencies
        .insert(name.clone(), resolved_version.clone());
    save_config(&config);

    let all_deps = super::utils::aggregate_deps(&config);
    if let Err(e) = rt.block_on(tooling::pods::install_all_deps(&all_deps, None, false)) {
        config.dependencies.remove(&name);
        save_config(&config);
        eprintln!("error: {}", e);
        process::exit(1);
    }

    println!("\x1b[1;32m    Added\x1b[0m {}@{}", name, resolved_version);
}

pub fn execute_remove(pod: &str) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut config = load_config();
    if config.dependencies.remove(pod).is_none() {
        eprintln!("error: '{}' is not a dependency", pod);
        process::exit(1);
    }
    save_config(&config);

    let all_deps = super::utils::aggregate_deps(&config);
    if let Err(e) = rt.block_on(tooling::pods::install_all_deps(&all_deps, None, false)) {
        eprintln!("error: failed to update lockfile: {}", e);
    }

    println!("\x1b[1;32m  Removed\x1b[0m {}", pod);
}

pub fn execute_install() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let config = load_config();
    let all_deps = super::utils::aggregate_deps(&config);
    if all_deps.is_empty() {
        println!("No dependencies to install.");
        return;
    }
    if let Err(e) = rt.block_on(tooling::pods::install_all_deps(&all_deps, None, false)) {
        eprintln!("error: {}", e);
        process::exit(1);
    }
    println!("\x1b[1;32m   Installed\x1b[0m all dependencies");
}

pub fn execute_update(pod: Option<&String>) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let config = load_config();
    let all_deps = super::utils::aggregate_deps(&config);
    if all_deps.is_empty() {
        println!("No dependencies to update.");
        return;
    }

    let unlocked = if let Some(name) = pod {
        Some(vec![name.clone()])
    } else {
        Some(vec![])
    };

    if let Err(e) = rt.block_on(tooling::pods::install_all_deps(&all_deps, unlocked, false)) {
        eprintln!("error: {}", e);
        process::exit(1);
    }

    println!("\x1b[1;32m   Updated\x1b[0m lockfile and dependencies");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::utils::Config;

    #[test]
    fn parse_versioned_pod_with_version() {
        let (name, version) = parse_versioned_pod("foo@1.2.3");
        assert_eq!(name, "foo");
        assert_eq!(version, "1.2.3");
    }

    #[test]
    fn parse_versioned_pod_without_version() {
        let (name, version) = parse_versioned_pod("foo");
        assert_eq!(name, "foo");
        assert_eq!(version, "latest");
    }

    #[test]
    fn parse_versioned_pod_multiple_at_signs() {
        let (name, version) = parse_versioned_pod("foo@1.0@beta");
        assert_eq!(name, "foo");
        assert_eq!(version, "1.0@beta");
    }

    #[test]
    fn config_save_and_load_with_deps() {
        let _lock = crate::commands::utils::CWD_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join("olive_deps_test_config");
        let _ = std::fs::create_dir_all(&dir);
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();

        let mut deps = std::collections::HashMap::new();
        deps.insert("serde".into(), "1.0".into());
        let config = Config {
            dependencies: deps,
            ..Config::default()
        };
        save_config(&config);

        let loaded = load_config();
        assert_eq!(loaded.dependencies.len(), 1);
        assert_eq!(loaded.dependencies.get("serde").unwrap(), "1.0");

        std::env::set_current_dir(&cwd).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_remove_dep_roundtrip() {
        let _lock = crate::commands::utils::CWD_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join("olive_deps_test_remove");
        let _ = std::fs::create_dir_all(&dir);
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();

        let mut deps = std::collections::HashMap::new();
        deps.insert("tokio".into(), "1.0".into());
        deps.insert("serde".into(), "2.0".into());
        let config = Config {
            dependencies: deps,
            ..Config::default()
        };
        save_config(&config);

        let mut loaded = load_config();
        loaded.dependencies.remove("serde");
        save_config(&loaded);

        let reloaded = load_config();
        assert_eq!(reloaded.dependencies.len(), 1);
        assert!(reloaded.dependencies.contains_key("tokio"));

        std::env::set_current_dir(&cwd).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
