use super::utils::{load_config, maybe_install_deps, run_build_script};
use crate::compile::{compile_and_emit, compile_and_test};
use std::path::Path;

fn derive_output_name(path: &str, output: Option<&String>) -> String {
    output.cloned().unwrap_or_else(|| {
        Path::new(path)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
    })
}

pub fn execute_build(path: Option<&String>, output: Option<&String>, time: bool, release: bool) {
    let original_dir = std::env::current_dir().unwrap();
    if let Some(p) = path {
        let path_obj = Path::new(p);
        if path_obj.is_file() || p.ends_with(".liv") {
            let out = derive_output_name(p, output);
            compile_and_emit(p, &out, time, release);
            return;
        }

        if std::env::set_current_dir(p).is_err() {
            eprintln!("error: could not switch to directory {}", p);
            std::process::exit(1);
        }
    }
    let config = load_config();
    let all_deps = super::utils::aggregate_deps(&config);
    maybe_install_deps(&all_deps);

    if let Some(workspace) = config.workspace {
        println!("\x1b[1;32m   Compiling\x1b[0m workspace...");
        for member in workspace.members {
            if std::env::set_current_dir(original_dir.join(&member)).is_err() {
                eprintln!("error: could not switch to workspace member {}", member);
                continue;
            }
            let member_config = load_config();
            if let Some(pod) = member_config.pod {
                crate::compile::loader::set_pod_meta(crate::compile::loader::PodMeta {
                    name: pod.name.clone(),
                    version: pod.version.clone(),
                    author: pod.author.clone().unwrap_or_default(),
                });
                run_build_script(time, release);
                let out = format!("grove/{}", pod.name);
                println!("\x1b[1;32m   Compiling\x1b[0m {}", pod.name);
                compile_and_emit(&pod.entry, &out, time, release);
            }
        }
    } else if let Some(pod) = config.pod {
        crate::compile::loader::set_pod_meta(crate::compile::loader::PodMeta {
            name: pod.name.clone(),
            version: pod.version.clone(),
            author: pod.author.clone().unwrap_or_default(),
        });
        run_build_script(time, release);
        let out = format!("grove/{}", pod.name);
        compile_and_emit(&pod.entry, &out, time, release);
    } else {
        eprintln!("error: no pod or workspace defined in pit.toml");
        std::process::exit(1);
    }
    let _ = std::env::set_current_dir(original_dir);
}

pub fn execute_test(time: bool, release: bool) {
    let original_dir = std::env::current_dir().unwrap();
    let config = load_config();
    let all_deps = super::utils::aggregate_deps(&config);
    maybe_install_deps(&all_deps);

    if let Some(workspace) = config.workspace {
        println!("\x1b[1;34mRunning tests for workspace...\x1b[0m");
        for member in workspace.members {
            if std::env::set_current_dir(original_dir.join(&member)).is_err() {
                continue;
            }
            let member_config = load_config();
            if let Some(pod) = member_config.pod {
                crate::compile::loader::set_pod_meta(crate::compile::loader::PodMeta {
                    name: pod.name.clone(),
                    version: pod.version.clone(),
                    author: pod.author.clone().unwrap_or_default(),
                });
                run_build_script(time, release);
                println!("\x1b[1;34mTesting\x1b[0m {}", pod.name);
                compile_and_test(&pod.entry, time, release);
            }
        }
    } else if let Some(pod) = config.pod {
        crate::compile::loader::set_pod_meta(crate::compile::loader::PodMeta {
            name: pod.name.clone(),
            version: pod.version.clone(),
            author: pod.author.clone().unwrap_or_default(),
        });
        run_build_script(time, release);
        compile_and_test(&pod.entry, time, release);
    } else {
        eprintln!("error: no pod or workspace defined in pit.toml");
        std::process::exit(1);
    }
    let _ = std::env::set_current_dir(original_dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_output_name_uses_file_stem_when_no_output_given() {
        let result = derive_output_name("src/main.liv", None);
        assert_eq!(result, "main");
    }

    #[test]
    fn derive_output_name_uses_given_output() {
        let result = derive_output_name("src/main.liv", Some(&"my_prog".into()));
        assert_eq!(result, "my_prog");
    }

    #[test]
    fn derive_output_name_handles_path_without_extension() {
        let result = derive_output_name("build", None);
        assert_eq!(result, "build");
    }

    #[test]
    fn derive_output_name_prefers_explicit_output() {
        let result = derive_output_name("foo.liv", Some(&"bar".into()));
        assert_eq!(result, "bar");
    }
}
