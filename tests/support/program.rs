use std::fs::{self, File};
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static UNIQUE: AtomicU64 = AtomicU64::new(0);

struct Case(PathBuf);

impl Case {
    fn new(source: &str) -> Self {
        let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("olive_program_{}_{id}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("main.liv"), source).unwrap();
        Self(dir)
    }

    fn execute(&self, command: &mut Command) -> (ExitStatus, String, String) {
        let stdout = self.0.join("stdout");
        let stderr = self.0.join("stderr");
        let mut child = command
            .stdin(Stdio::null())
            .stdout(File::create(&stdout).unwrap())
            .stderr(File::create(&stderr).unwrap())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("command timed out: {}", fs::read_to_string(stderr).unwrap());
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        (
            status,
            fs::read_to_string(stdout).unwrap(),
            fs::read_to_string(stderr).unwrap(),
        )
    }
}

impl Drop for Case {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

pub fn assert_both(source: &str, expected: &str) {
    assert_both_with(source, |status, stdout, stderr| {
        assert!(status.success(), "program exited {status}: {stderr}");
        assert_eq!(stdout, expected);
    });
}

pub fn assert_both_with(source: &str, check: impl Fn(ExitStatus, &str, &str)) {
    let case = Case::new(source);
    let source = case.0.join("main.liv");
    let pit = env!("CARGO_BIN_EXE_pit");
    let (status, stdout, stderr) = case.execute(Command::new(pit).arg("run").arg(&source));
    check(status, &stdout, &stderr);
    let binary = case.0.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    let (status, _, stderr) = case.execute(
        Command::new(pit)
            .arg("build")
            .arg("--release")
            .arg(&source)
            .arg("-o")
            .arg(&binary),
    );
    assert!(status.success(), "build exited {status}: {stderr}");
    let (status, stdout, stderr) = case.execute(&mut Command::new(binary));
    check(status, &stdout, &stderr);
}
