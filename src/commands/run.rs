use super::utils::{load_config, maybe_install_deps, run_build_script};
use crate::compile::{compile_and_run, compile_and_run_aot, compile_hybrid};

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
        if let Some(pod) = config.pod {
            maybe_install_deps(&config.dependencies);
            run_build_script(time, release);
            (pod.entry.clone(), true)
        } else {
            eprintln!("error: no pod defined in pit.toml to run");
            std::process::exit(1);
        }
    };

    if jit || emit_ast || emit_mir || (!is_project && !aot && !hybrid) {
        compile_and_run(&entry, true, time, emit_ast, emit_mir, release);
    } else if aot {
        compile_and_run_aot(&entry, time, release);
    } else {
        compile_hybrid(&entry, time, release);
    }
}
