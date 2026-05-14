use super::utils::load_config;
use super::utils::{Config, Pod};
use crate::fmt::{self, DEFAULT_WIDTH};
use crate::tooling;
use crate::tooling::repl::run_shell;
use std::collections::HashMap;
use std::{fs, path::Path, process};

fn git_user_name() -> Option<String> {
    std::process::Command::new("git")
        .args(["config", "--get", "user.name"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

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
            author: git_user_name(),
            entry: "src/main.liv".to_string(),
            olive: None,
        }),
        dependencies: HashMap::new(),
        workspace: None,
        profile: HashMap::new(),
        fmt: None,
    };

    fs::write(path.join("pit.toml"), toml::to_string(&config).unwrap()).unwrap();
    fs::write(
        path.join("src/main.liv"),
        "fn main():\n    print(\"Hello from Olive!\")\n",
    )
    .unwrap();
    fs::write(path.join(".gitignore"), ".env\n.env.*\n*.secret\ngrove/\n").unwrap();

    match std::process::Command::new("git")
        .arg("init")
        .current_dir(path)
        .output()
    {
        Ok(out) if out.status.success() => {}
        _ => eprintln!("warning: could not initialize git repository"),
    }

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

pub fn execute_fmt(file: Option<&String>, check: bool, diff: bool, stdin: bool) {
    let max_width = configured_fmt_width().unwrap_or(DEFAULT_WIDTH);
    let mode = if stdin {
        fmt::Mode::Stdin
    } else if check {
        fmt::Mode::Check
    } else if diff {
        fmt::Mode::Diff
    } else {
        fmt::Mode::Write
    };
    let code = fmt::execute(file, fmt::Options { max_width, mode });
    if code != 0 {
        process::exit(code);
    }
}

/// Read `[fmt] max_width` from the nearest `pit.toml`, if any. Unlike `load_config`
/// this never exits: `pit fmt` must work on a lone file outside a project.
fn configured_fmt_width() -> Option<usize> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join("pit.toml");
        if candidate.exists() {
            let content = fs::read_to_string(&candidate).ok()?;
            let config: Config = toml::from_str(&content).ok()?;
            return config.fmt.and_then(|f| f.max_width);
        }
        dir = dir.parent()?.to_path_buf();
    }
}

pub fn execute_shell() {
    run_shell();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execute_new_creates_project_structure() {
        let _lock = crate::commands::utils::CWD_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join("olive_project_test_new");
        let _ = std::fs::create_dir_all(&dir);
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();

        execute_new("test_proj");

        let proj_dir = dir.join("test_proj");
        assert!(proj_dir.join("pit.toml").exists());
        assert!(proj_dir.join("src/main.liv").exists());
        assert!(proj_dir.join(".gitignore").exists());
        assert!(proj_dir.join("src").is_dir());
        assert!(proj_dir.join(".git").is_dir());

        let config_content = std::fs::read_to_string(proj_dir.join("pit.toml")).unwrap();
        let config: Config = toml::from_str(&config_content).unwrap();
        assert_eq!(config.pod.as_ref().unwrap().name, "test_proj");
        assert_eq!(config.pod.as_ref().unwrap().version, "0.1.0");
        assert_eq!(config.pod.as_ref().unwrap().entry, "src/main.liv");

        let main_content = std::fs::read_to_string(proj_dir.join("src/main.liv")).unwrap();
        assert!(main_content.contains("fn main()"));

        std::env::set_current_dir(&cwd).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn execute_new_creates_valid_pit_toml() {
        let _lock = crate::commands::utils::CWD_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join("olive_project_test_toml");
        let _ = std::fs::create_dir_all(&dir);
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();

        execute_new("toml_check");

        let config_content = std::fs::read_to_string(dir.join("toml_check/pit.toml")).unwrap();
        let config: Config = toml::from_str(&config_content).unwrap();
        let pod = config.pod.unwrap();
        assert_eq!(pod.name, "toml_check");
        assert_eq!(pod.version, "0.1.0");
        assert_eq!(pod.entry, "src/main.liv");
        assert!(pod.olive.is_none());
        assert!(config.dependencies.is_empty());
        assert!(config.workspace.is_none());

        std::env::set_current_dir(&cwd).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn execute_new_creates_gitignore_with_grove() {
        let _lock = crate::commands::utils::CWD_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join("olive_project_test_gitignore");
        let _ = std::fs::create_dir_all(&dir);
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();

        execute_new("gitignore_check");

        let gitignore = std::fs::read_to_string(dir.join("gitignore_check/.gitignore")).unwrap();
        assert!(gitignore.contains("grove/"));

        std::env::set_current_dir(&cwd).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
