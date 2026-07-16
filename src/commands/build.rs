use super::utils::{load_config, maybe_install_deps, run_build_script};
use crate::compile::{cache, compile_and_emit, compile_and_test, pgo};
use std::path::Path;

pub fn execute_build(
    path: Option<&String>,
    output: Option<&String>,
    time: bool,
    release: bool,
    pgo: Option<&str>,
    pymodule: bool,
    module_name: Option<&str>,
    explain_copies: bool,
) {
    let original_dir = std::env::current_dir().unwrap();
    if let Some(p) = path {
        let path_obj = Path::new(p);
        if path_obj.is_file() || p.ends_with(".liv") {
            let default_module_name = if pymodule {
                path_obj
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.replace('-', "_"))
            } else {
                None
            };
            let module_name = module_name.or_else(|| default_module_name.as_deref());
            match output {
                Some(o) => compile_and_emit(
                    p,
                    o,
                    time,
                    release,
                    pgo,
                    pymodule,
                    module_name,
                    explain_copies,
                ),
                None => {
                    let (target, _) = cache::prepare(p, release);
                    if cache::is_fresh(&target) {
                        println!(
                            "\x1b[1;32mFinished\x1b[0m build `{}` (already up to date).",
                            target.binary_path
                        );
                    } else {
                        let effective_pgo = pgo
                            .map(String::from)
                            .or_else(|| pgo::auto_detect(target.hash()));
                        compile_and_emit(
                            p,
                            &target.binary_path,
                            time,
                            release,
                            effective_pgo.as_deref(),
                            false,
                            None,
                            explain_copies,
                        );
                        cache::record(&target);
                    }
                }
            }
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
                let (target, _) = cache::prepare(&pod.entry, release);
                if cache::is_fresh(&target) {
                    println!(
                        "\x1b[1;32mFinished\x1b[0m build `{}` (already up to date).",
                        target.binary_path
                    );
                } else {
                    println!("\x1b[1;32m   Compiling\x1b[0m {}", pod.name);
                    let effective_pgo = pgo::auto_detect(target.hash());
                    compile_and_emit(
                        &pod.entry,
                        &target.binary_path,
                        time,
                        release,
                        effective_pgo.as_deref(),
                        false,
                        None,
                        explain_copies,
                    );
                    cache::record(&target);
                }
            }
        }
    } else if let Some(pod) = config.pod {
        crate::compile::loader::set_pod_meta(crate::compile::loader::PodMeta {
            name: pod.name.clone(),
            version: pod.version.clone(),
            author: pod.author.clone().unwrap_or_default(),
        });
        run_build_script(time, release);
        let (target, _) = cache::prepare(&pod.entry, release);
        if cache::is_fresh(&target) {
            println!(
                "\x1b[1;32mFinished\x1b[0m build `{}` (already up to date).",
                target.binary_path
            );
        } else {
            let effective_pgo = pgo
                .map(String::from)
                .or_else(|| pgo::auto_detect(target.hash()));
            compile_and_emit(
                &pod.entry,
                &target.binary_path,
                time,
                release,
                effective_pgo.as_deref(),
                false,
                None,
                explain_copies,
            );
            cache::record(&target);
        }
    } else {
        eprintln!("error: no pod or workspace defined in pit.toml");
        std::process::exit(1);
    }
    let _ = std::env::set_current_dir(original_dir);
}

pub fn execute_test(time: bool, release: bool, _explain_copies: bool) {
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
                compile_and_test(&pod.entry, time, release, _explain_copies);
            }
        }
    } else if let Some(pod) = config.pod {
        crate::compile::loader::set_pod_meta(crate::compile::loader::PodMeta {
            name: pod.name.clone(),
            version: pod.version.clone(),
            author: pod.author.clone().unwrap_or_default(),
        });
        run_build_script(time, release);
        compile_and_test(&pod.entry, time, release, _explain_copies);
    } else {
        eprintln!("error: no pod or workspace defined in pit.toml");
        std::process::exit(1);
    }
    let _ = std::env::set_current_dir(original_dir);
}
