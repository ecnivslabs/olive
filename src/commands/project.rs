use super::utils::load_config;
use super::utils::{Config, Pod};
use crate::fmt::{format_file, walk_and_format};
use crate::tooling;
use crate::tooling::repl::run_shell;
use std::collections::HashMap;
use std::{fs, path::Path, process};

pub fn execute_new(name: &str) {
    let path = Path::new(name);
    if path.exists() {
        eprintln!("error: directory `{}` already exists", name);
        process::exit(1);
    }

    fs::create_dir_all(path.join("src")).unwrap();

    let config = Config {
        pod: Some(Pod {
            name: name.to_string(),
            version: "0.1.0".to_string(),
            entry: "src/main.liv".to_string(),
            olive: None,
        }),
        dependencies: HashMap::new(),
        workspace: None,
        profile: HashMap::new(),
    };

    fs::write(path.join("pit.toml"), toml::to_string(&config).unwrap()).unwrap();
    fs::write(
        path.join("src/main.liv"),
        "fn main():\n    print(\"Hello from Olive!\")\n",
    )
    .unwrap();
    fs::write(path.join(".gitignore"), ".env\n.env.*\n*.secret\ngrove/\n").unwrap();

    println!(
        "\x1b[1;32mCreated\x1b[0m binary (application) `{}` pod",
        name
    );
}

pub fn execute_publish() {
    let config = load_config();
    let pod = config.pod.unwrap_or_else(|| {
        eprintln!("error: no pod defined in pit.toml to publish");
        process::exit(1);
    });
    if let Err(e) = tooling::publish::publish(&pod.name, &pod.version) {
        eprintln!("error: {}", e);
        process::exit(1);
    }
}

pub fn execute_upgrade() {
    if let Err(e) = tooling::upgrade::upgrade() {
        eprintln!("error: {}", e);
        process::exit(1);
    }
}

pub fn execute_fmt(file: Option<&String>) {
    if let Some(f) = file {
        let path = Path::new(f);
        if path.is_dir() {
            walk_and_format(path);
        } else {
            format_file(f);
        }
    } else {
        let _config = load_config();
        walk_and_format(Path::new("."));
    }
}

pub fn execute_shell() {
    run_shell();
}
