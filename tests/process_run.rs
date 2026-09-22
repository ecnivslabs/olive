#[path = "support/program.rs"]
mod program;
use program::assert_both;

use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static UNIQUE: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("olive-process-run-{}-{}", std::process::id(), id));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn write_main(&self, source: &str) {
        fs::write(self.0.join("main.liv"), source).unwrap();
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
fn run_olive(case: &TestDir, timeout: Duration) -> Output {
    use std::os::unix::process::CommandExt;

    let mut command = Command::new(env!("CARGO_BIN_EXE_pit"));
    command
        .args(["run", "main.liv"])
        .current_dir(&case.0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait().unwrap() {
            Some(_) => return child.wait_with_output().unwrap(),
            None if Instant::now() >= deadline => {
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.wait();
                panic!("Olive process exceeded {timeout:?}");
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

#[test]
fn shell_uses_the_platform_command_interpreter() {
    #[cfg(unix)]
    let source = r#"import process

fn main():
    let output = process.shell("printf 'payload'")
    if output == -1:
        print("spawn failed")
        return
    print(output.stdout)
"#;
    #[cfg(windows)]
    let source = r#"import process

fn main():
    let output = process.shell("echo payload")
    if output == -1:
        print("spawn failed")
        return
    print(output.stdout.trim())
"#;
    assert_both(source, "payload\n");
}

#[test]
fn helper_reads_stdin_until_eof() {
    if std::env::var_os("OLIVE_PROCESS_HELPER").is_none() {
        return;
    }

    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input).unwrap();
    assert!(input.is_empty());
    if let Some(marker) = std::env::var_os("OLIVE_PROCESS_MARKER") {
        std::fs::write(marker, b"ok").unwrap();
    }
}

#[test]
fn command_run_closes_default_stdin_before_waiting() {
    let case = TestDir::new();
    case.write_main(
        r#"import os
import process

fn main():
    let helper = os.getenv("OLIVE_PROCESS_HELPER")
    let output = process.run([helper, "--exact", "helper_reads_stdin_until_eof"])
    print("done")
"#,
    );

    let marker = case.0.join("helper-ran");
    let mut child = Command::new(env!("CARGO_BIN_EXE_pit"))
        .args(["run", "main.liv"])
        .current_dir(&case.0)
        .env("OLIVE_PROCESS_HELPER", std::env::current_exe().unwrap())
        .env("OLIVE_PROCESS_MARKER", &marker)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        match child.try_wait().unwrap() {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("process.run did not close stdin before waiting");
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    let output = child.wait_with_output().unwrap();

    assert!(
        status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("done"));
    assert!(marker.is_file(), "process helper did not run");
}

#[cfg(unix)]
#[test]
fn command_run_does_not_wait_for_a_descendant_holding_stdout() {
    let case = TestDir::new();
    case.write_main(
        r#"import process

fn main():
    let output = process.shell("printf 'first'; sleep 3 & printf 'second'")
    if output == -1:
        print("spawn failed")
        return
    print(output.stdout)
    print(output.code)
"#,
    );

    let started = Instant::now();
    let output = run_olive(&case, Duration::from_secs(2));
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "direct child exit waited for descendant pipe closure"
    );
    assert_eq!(output.stdout, b"firstsecond\n0\n");
}

#[cfg(unix)]
#[test]
fn command_run_captures_complete_utf8_output() {
    let case = TestDir::new();
    case.write_main(
        r#"import process

fn main():
    let command = "i=0; while [ $i -lt 20000 ]; do printf x; i=$((i+1)); done; printf '€'"
    let output = process.shell(command)
    if output == -1:
        print("spawn failed")
        return
    print(len(output.stdout))
    print(output.stdout[0:1])
    print(output.stdout[20000:20001])
"#,
    );

    let output = run_olive(&case, Duration::from_secs(2));
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, "20003\nx\n€\n".as_bytes());
}
