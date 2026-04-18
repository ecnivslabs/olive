use super::utils::{load_config, save_config};
use crate::tooling;
use std::process;

pub fn execute_add(pod: &str) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (name, version_req) = if let Some((n, v)) = pod.split_once('@') {
        (n.to_string(), v.to_string())
    } else {
        (pod.to_string(), "latest".to_string())
    };

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
