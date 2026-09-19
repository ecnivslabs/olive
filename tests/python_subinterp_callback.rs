use std::fs;
use std::process::Command;

fn run_case(source: &str, helper: &str) {
    let dir = std::env::temp_dir().join(format!(
        "olive_subinterp_callback_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("main.liv"), source).unwrap();
    fs::write(dir.join("threadhelper.py"), helper).unwrap();
    let pit = env!("CARGO_BIN_EXE_pit");
    let run = |program: &std::path::Path, args: &[&str], expected: Option<&str>| {
        let output = Command::new(program)
            .args(args)
            .current_dir(&dir)
            .env("PYTHONPATH", &dir)
            .env("OLIVE_PY_SUBINTERP", "1")
            .env("OLIVE_PY_SUBINTERP_SIZE", "2")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "status={} stdout={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if let Some(expected) = expected {
            assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
        }
        assert!(!String::from_utf8_lossy(&output.stderr).contains("Fatal Python error"));
    };
    run(std::path::Path::new(pit), &["run", "main.liv"], Some("7\n"));
    let binary = dir.join("main");
    run(
        std::path::Path::new(pit),
        &[
            "build",
            "--release",
            "main.liv",
            "-o",
            binary.to_str().unwrap(),
        ],
        None,
    );
    run(&binary, &[], Some("7\n"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn python_created_thread_reuses_borrowed_interpreter_state() {
    run_case(
        r#"import py "threadhelper" as h
import py "math" as math

fn callback(x: int) -> int:
    let r = math.sqrt(x * x)
    return int(r)

fn main():
    print(h.run_thread(callback, 7))
"#,
        r#"import threading

def run_thread(fn, x):
    result = []
    thread = threading.Thread(target=lambda: result.append(fn(x)))
    thread.start()
    thread.join()
    return result[0]
"#,
    );
}
