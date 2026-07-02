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

fn entry_path(lib: bool) -> &'static str {
    if lib { "src/lib.liv" } else { "src/main.liv" }
}

fn default_source(lib: bool) -> &'static str {
    if lib {
        "fn greet(name: str) -> str:\n    return \"Hello, \" + name + \"!\"\n"
    } else {
        "fn main():\n    print(\"Hello from Olive!\")\n"
    }
}

fn pod_kind(lib: bool) -> &'static str {
    if lib {
        "library"
    } else {
        "binary (application)"
    }
}

fn build_config(name: &str, entry: &str) -> Config {
    Config {
        pod: Some(Pod {
            name: name.to_string(),
            version: "0.1.0".to_string(),
            author: git_user_name(),
            entry: entry.to_string(),
            olive: None,
        }),
        dependencies: HashMap::new(),
        workspace: None,
        profile: HashMap::new(),
        fmt: None,
    }
}

pub fn execute_new(name: &str, lib: bool) {
    let path = Path::new(name);
    if path.exists() {
        eprintln!("error: directory `{}` already exists", name);
        process::exit(1);
    }

    fs::create_dir_all(path.join("src")).unwrap();

    let entry = entry_path(lib);
    let config = build_config(name, entry);

    fs::write(path.join("pit.toml"), toml::to_string(&config).unwrap()).unwrap();
    fs::write(path.join(entry), default_source(lib)).unwrap();
    fs::write(path.join(".gitignore"), ".env\n.env.*\n*.secret\ngrove/\n").unwrap();

    match std::process::Command::new("git")
        .arg("init")
        .current_dir(path)
        .output()
    {
        Ok(out) if out.status.success() => {}
        _ => eprintln!("warning: could not initialize git repository"),
    }

    println!("\x1b[1;32mCreated\x1b[0m {} `{}` pod", pod_kind(lib), name);
}

pub fn execute_init(name: Option<String>, lib: bool) {
    let cwd = std::env::current_dir().unwrap();
    let pit_toml = cwd.join("pit.toml");
    if pit_toml.exists() {
        eprintln!("error: pit.toml already exists in this directory");
        process::exit(1);
    }

    let name = name.unwrap_or_else(|| {
        cwd.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "project".to_string())
    });

    fs::create_dir_all(cwd.join("src")).unwrap();

    let entry = entry_path(lib);
    let config = build_config(&name, entry);
    fs::write(&pit_toml, toml::to_string(&config).unwrap()).unwrap();

    let entry_file = cwd.join(entry);
    if !entry_file.exists() {
        fs::write(&entry_file, default_source(lib)).unwrap();
    }

    let gitignore = cwd.join(".gitignore");
    if !gitignore.exists() {
        fs::write(&gitignore, ".env\n.env.*\n*.secret\ngrove/\n").unwrap();
    } else {
        let content = fs::read_to_string(&gitignore).unwrap();
        if !content.lines().any(|line| line.trim() == "grove/") {
            let mut updated = content;
            if !updated.is_empty() && !updated.ends_with('\n') {
                updated.push('\n');
            }
            updated.push_str("grove/\n");
            fs::write(&gitignore, updated).unwrap();
        }
    }

    println!(
        "\x1b[1;32mInitialized\x1b[0m {} `{}` pod",
        pod_kind(lib),
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

        execute_new("test_proj", false);

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

        execute_new("toml_check", false);

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

        execute_new("gitignore_check", false);

        let gitignore = std::fs::read_to_string(dir.join("gitignore_check/.gitignore")).unwrap();
        assert!(gitignore.contains("grove/"));

        std::env::set_current_dir(&cwd).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn execute_new_lib_creates_lib_liv() {
        let _lock = crate::commands::utils::CWD_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join("olive_project_test_lib");
        let _ = std::fs::create_dir_all(&dir);
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();

        execute_new("test_lib", true);

        let proj_dir = dir.join("test_lib");
        assert!(proj_dir.join("pit.toml").exists());
        assert!(proj_dir.join("src/lib.liv").exists());
        assert!(!proj_dir.join("src/main.liv").exists());

        let config_content = std::fs::read_to_string(proj_dir.join("pit.toml")).unwrap();
        let config: Config = toml::from_str(&config_content).unwrap();
        assert_eq!(config.pod.as_ref().unwrap().entry, "src/lib.liv");

        let lib_content = std::fs::read_to_string(proj_dir.join("src/lib.liv")).unwrap();
        assert!(!lib_content.contains("fn main()"));

        std::env::set_current_dir(&cwd).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn execute_init_creates_project_structure_in_cwd() {
        let _lock = crate::commands::utils::CWD_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join("olive_project_test_init");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();

        execute_init(Some("init_proj".to_string()), false);

        assert!(dir.join("pit.toml").exists());
        assert!(dir.join("src/main.liv").exists());
        assert!(dir.join(".gitignore").exists());
        assert!(!dir.join(".git").exists());

        let config_content = std::fs::read_to_string(dir.join("pit.toml")).unwrap();
        let config: Config = toml::from_str(&config_content).unwrap();
        assert_eq!(config.pod.as_ref().unwrap().name, "init_proj");
        assert_eq!(config.pod.as_ref().unwrap().entry, "src/main.liv");

        std::env::set_current_dir(&cwd).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn execute_init_defaults_name_to_dir_name() {
        let _lock = crate::commands::utils::CWD_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join("olive_project_test_init_default_name");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();

        execute_init(None, false);

        let config_content = std::fs::read_to_string(dir.join("pit.toml")).unwrap();
        let config: Config = toml::from_str(&config_content).unwrap();
        assert_eq!(
            config.pod.as_ref().unwrap().name,
            dir.file_name().unwrap().to_string_lossy()
        );

        std::env::set_current_dir(&cwd).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn execute_init_does_not_clobber_existing_entry_file() {
        let _lock = crate::commands::utils::CWD_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join("olive_project_test_init_no_clobber");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("src/main.liv"),
            "fn main():\n    print(\"existing\")\n",
        )
        .unwrap();
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();

        execute_init(Some("no_clobber".to_string()), false);

        let main_content = std::fs::read_to_string(dir.join("src/main.liv")).unwrap();
        assert!(main_content.contains("existing"));

        std::env::set_current_dir(&cwd).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn execute_init_appends_grove_to_existing_gitignore() {
        let _lock = crate::commands::utils::CWD_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join("olive_project_test_init_gitignore_append");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".gitignore"), "node_modules/\n").unwrap();
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();

        execute_init(Some("gitignore_append".to_string()), false);

        let gitignore = std::fs::read_to_string(dir.join(".gitignore")).unwrap();
        assert!(gitignore.contains("node_modules/"));
        assert!(gitignore.contains("grove/"));

        std::env::set_current_dir(&cwd).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn execute_init_refuses_when_pit_toml_exists() {
        use std::process::{Command, Stdio};
        let dir = std::env::temp_dir().join("olive_project_test_init_refuse");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("pit.toml"), "").unwrap();

        let bin = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("pit");
        let status = Command::new(bin)
            .arg("init")
            .current_dir(&dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();

        assert!(!status.success());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
