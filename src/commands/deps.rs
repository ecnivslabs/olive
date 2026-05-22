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
        .block_on(tooling::registry::fetch_versions(&name))
        .unwrap_or_else(|e| {
            eprintln!("error: {}", e);
            process::exit(1);
        });

    let pkg = tooling::registry::resolve_version(&versions, &version_req).unwrap_or_else(|| {
        eprintln!("error: no matching version for '{}@{}'", name, version_req);
        process::exit(1);
    });

    let resolved_version = pkg.vers.clone();
    let pkg = pkg.clone();

    if let Err(e) = rt.block_on(tooling::pods::download_and_install(&pkg)) {
        eprintln!("error: {}", e);
        process::exit(1);
    }

    let mut config = load_config();
    config
        .dependencies
        .insert(name.clone(), resolved_version.clone());
    save_config(&config);

    println!("\x1b[1;32m    Added\x1b[0m {}@{}", name, resolved_version);
}

pub fn execute_remove(pod: &str) {
    let mut config = load_config();
    if config.dependencies.remove(pod).is_none() {
        eprintln!("error: '{}' is not a dependency", pod);
        process::exit(1);
    }
    save_config(&config);
    println!("\x1b[1;32m  Removed\x1b[0m {}", pod);
}

pub fn execute_install() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let config = load_config();
    if config.dependencies.is_empty() {
        println!("No dependencies to install.");
        return;
    }
    if let Err(e) = rt.block_on(tooling::pods::install_all_deps(&config.dependencies)) {
        eprintln!("error: {}", e);
        process::exit(1);
    }
    println!("\x1b[1;32m   Installed\x1b[0m all dependencies");
}

pub fn execute_update(pod: Option<&String>) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut config = load_config();
    if config.dependencies.is_empty() {
        println!("No dependencies to update.");
        return;
    }

    let targets: Vec<String> = if let Some(name) = pod {
        if !config.dependencies.contains_key(name) {
            eprintln!("error: '{}' is not a dependency", name);
            process::exit(1);
        }
        vec![name.clone()]
    } else {
        config.dependencies.keys().cloned().collect()
    };

    let mut updated = 0;
    for name in &targets {
        let current = config.dependencies[name].clone();
        let versions = rt
            .block_on(tooling::registry::fetch_versions(name))
            .unwrap_or_else(|e| {
                eprintln!("error: {}", e);
                process::exit(1);
            });
        let latest = match tooling::registry::resolve_version(&versions, "latest") {
            Some(v) => v.clone(),
            None => {
                eprintln!("warning: no available version for '{}'", name);
                continue;
            }
        };
        if latest.vers == current {
            println!("  {} already at {}", name, current);
            continue;
        }
        if let Err(e) = rt.block_on(tooling::pods::download_and_install(&latest)) {
            eprintln!("error: {}", e);
            process::exit(1);
        }
        println!(
            "\x1b[1;32m  Updated\x1b[0m {} {} → {}",
            name, current, latest.vers
        );
        config.dependencies.insert(name.clone(), latest.vers);
        updated += 1;
    }

    if updated > 0 {
        save_config(&config);
    }
}
