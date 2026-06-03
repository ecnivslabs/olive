use super::utils::{aggregate_deps, load_config, maybe_install_deps, run_build_script};
use crate::compile::compile_and_bench;

/// `pit bench`: same project-discovery shape as `pit test` (workspace vs.
/// single pod), `#[bench]` in place of `#[test]`.
pub fn execute_bench(json: bool) {
    let original_dir = std::env::current_dir().unwrap();
    let config = load_config();
    let all_deps = aggregate_deps(&config);
    maybe_install_deps(&all_deps);

    if let Some(workspace) = config.workspace {
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
                run_build_script(false, true);
                if !json {
                    println!("\x1b[1;34mBenchmarking\x1b[0m {}", pod.name);
                }
                compile_and_bench(&pod.entry, json);
            }
        }
    } else if let Some(pod) = config.pod {
        crate::compile::loader::set_pod_meta(crate::compile::loader::PodMeta {
            name: pod.name.clone(),
            version: pod.version.clone(),
            author: pod.author.clone().unwrap_or_default(),
        });
        run_build_script(false, true);
        compile_and_bench(&pod.entry, json);
    } else {
        eprintln!("error: no pod or workspace defined in pit.toml");
        std::process::exit(1);
    }
    let _ = std::env::set_current_dir(original_dir);
}
