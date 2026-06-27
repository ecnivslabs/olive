use super::utils::{load_config, maybe_install_deps, run_build_script};
use crate::compile::{compile_and_run, compile_and_run_aot, compile_hybrid};

#[derive(Debug, PartialEq, Eq)]
enum RunMode {
    Jit,
    Aot,
    Hybrid,
}

fn resolve_run_mode(
    is_project: bool,
    jit: bool,
    aot: bool,
    hybrid: bool,
    emit_ast: bool,
    emit_mir: bool,
) -> RunMode {
    if jit || emit_ast || emit_mir || (!is_project && !aot && !hybrid) {
        RunMode::Jit
    } else if aot {
        RunMode::Aot
    } else {
        RunMode::Hybrid
    }
}

#[allow(clippy::too_many_arguments)]
pub fn execute_run(
    file: Option<&String>,
    time: bool,
    emit_ast: bool,
    emit_mir: bool,
    jit: bool,
    aot: bool,
    hybrid: bool,
    release: bool,
) {
    let (entry, is_project) = if let Some(f) = file {
        (f.clone(), false)
    } else {
        let config = load_config();
        let all_deps = super::utils::aggregate_deps(&config);
        maybe_install_deps(&all_deps);
        if let Some(pod) = config.pod {
            crate::compile::loader::set_pod_meta(crate::compile::loader::PodMeta {
                name: pod.name.clone(),
                version: pod.version.clone(),
                author: pod.author.clone().unwrap_or_default(),
            });
            run_build_script(time, release);
            (pod.entry.clone(), true)
        } else {
            eprintln!("error: no pod defined in pit.toml to run");
            std::process::exit(1);
        }
    };

    let mode = resolve_run_mode(is_project, jit, aot, hybrid, emit_ast, emit_mir);
    match mode {
        RunMode::Jit => compile_and_run(&entry, true, time, emit_ast, emit_mir, release),
        RunMode::Aot => compile_and_run_aot(&entry, time, release),
        RunMode::Hybrid => compile_hybrid(&entry, time, release),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_run_mode_defaults_to_jit_for_standalone_file() {
        assert_eq!(
            resolve_run_mode(false, false, false, false, false, false),
            RunMode::Jit
        );
    }

    #[test]
    fn resolve_run_mode_defaults_to_hybrid_for_project() {
        assert_eq!(
            resolve_run_mode(true, false, false, false, false, false),
            RunMode::Hybrid
        );
    }

    #[test]
    fn resolve_run_mode_jit_flag_wins() {
        assert_eq!(
            resolve_run_mode(true, true, true, true, false, false),
            RunMode::Jit
        );
    }

    #[test]
    fn resolve_run_mode_aot_flag() {
        assert_eq!(
            resolve_run_mode(true, false, true, false, false, false),
            RunMode::Aot
        );
    }

    #[test]
    fn resolve_run_mode_hybrid_flag() {
        assert_eq!(
            resolve_run_mode(false, false, false, true, false, false),
            RunMode::Hybrid
        );
    }

    #[test]
    fn resolve_run_mode_emit_ast_triggers_jit() {
        assert_eq!(
            resolve_run_mode(true, false, false, false, true, false),
            RunMode::Jit
        );
    }

    #[test]
    fn resolve_run_mode_emit_mir_triggers_jit() {
        assert_eq!(
            resolve_run_mode(true, false, false, false, false, true),
            RunMode::Jit
        );
    }

    #[test]
    fn resolve_run_mode_aot_over_hybrid() {
        assert_eq!(
            resolve_run_mode(true, false, true, true, false, false),
            RunMode::Aot
        );
    }
}
