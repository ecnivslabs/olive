use super::utils::load_config;
use crate::compile::fix::run_fix;

/// Entry point for `pit fix`. Resolves the file to operate on the same way
/// `run` and `build` do, applies the compiler's machine-applicable suggestions,
/// and prints a one-line summary.
pub fn execute_fix(file: Option<&String>, dry_run: bool) {
    let entry = if let Some(f) = file {
        f.clone()
    } else {
        let config = load_config();
        match config.pod {
            Some(pod) => pod.entry,
            None => {
                eprintln!("error: no pod defined in pit.toml; pass a file to fix");
                std::process::exit(1);
            }
        }
    };

    match run_fix(&entry, dry_run) {
        Ok(report) => {
            if report.applied == 0 {
                println!("no machine-applicable fixes found");
            } else {
                let verb = if dry_run { "would apply" } else { "applied" };
                println!(
                    "{verb} {} fix(es) across {} file(s) [{}]",
                    report.applied,
                    report.files_changed,
                    report.codes.join(", ")
                );
            }
        }
        Err(()) => std::process::exit(1),
    }
}
