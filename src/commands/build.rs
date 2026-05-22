use super::utils::{load_config, maybe_install_deps, run_build_script};
use crate::compile::{compile_and_emit, compile_and_test};
use std::path::Path;

pub fn execute_build(time: bool, release: bool) {
    let original_dir = std::env::current_dir().unwrap();
    let config = load_config();
    if let Some(workspace) = config.workspace {
        println!("\x1b[1;32m   Compiling\x1b[0m workspace...");
        for member in workspace.members {
            if std::env::set_current_dir(original_dir.join(&member)).is_err() {
                eprintln!("error: could not switch to workspace member {}", member);
                continue;
            }
            let member_config = load_config();
            if let Some(pod) = member_config.pod {
                maybe_install_deps(&member_config.dependencies);
                run_build_script(time, release);
                let out = format!("grove/{}", pod.name);
                println!("\x1b[1;32m   Compiling\x1b[0m {}", pod.name);
                compile_and_emit(&pod.entry, &out, time, release);
            }
        }
    } else if let Some(pod) = config.pod {
        maybe_install_deps(&config.dependencies);
        run_build_script(time, release);
        let out = format!("grove/{}", pod.name);
        compile_and_emit(&pod.entry, &out, time, release);
    } else {
        eprintln!("error: no pod or workspace defined in pit.toml");
        std::process::exit(1);
    }
    let _ = std::env::set_current_dir(original_dir);
}

pub fn execute_compile(file: &str, output: Option<&String>, time: bool, release: bool) {
    let out = output.map(|s| s.clone()).unwrap_or_else(|| {
        Path::new(file)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
    });
    compile_and_emit(file, &out, time, release);
}

pub fn execute_test(time: bool, release: bool) {
    let original_dir = std::env::current_dir().unwrap();
    let config = load_config();
    if let Some(workspace) = config.workspace {
        println!("\x1b[1;34mRunning tests for workspace...\x1b[0m");
        for member in workspace.members {
            if std::env::set_current_dir(original_dir.join(&member)).is_err() {
                continue;
            }
            let member_config = load_config();
            if let Some(pod) = member_config.pod {
                maybe_install_deps(&member_config.dependencies);
                run_build_script(time, release);
                println!("\x1b[1;34mTesting\x1b[0m {}", pod.name);
                compile_and_test(&pod.entry, time, release);
            }
        }
    } else if let Some(pod) = config.pod {
        maybe_install_deps(&config.dependencies);
        run_build_script(time, release);
        compile_and_test(&pod.entry, time, release);
    } else {
        eprintln!("error: no pod or workspace defined in pit.toml");
        std::process::exit(1);
    }
    let _ = std::env::set_current_dir(original_dir);
}
