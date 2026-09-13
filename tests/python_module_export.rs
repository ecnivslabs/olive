use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static UNIQUE: AtomicU64 = AtomicU64::new(0);

fn python_command() -> Option<&'static str> {
    ["python3", "python"].iter().copied().find(|command| {
        Command::new(command)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
}

struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn module_export_preserves_f32_and_u64_scalar_abi() {
    let Some(python) = python_command() else {
        eprintln!("Python not available, skipping test");
        return;
    };

    let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let module_name = format!("olive_pymodule_abi_{}_{}", std::process::id(), id);
    let case = TestDir::new(&module_name);
    let source_path = case.0.join("module.liv");
    let suffix = if cfg!(windows) { "pyd" } else { "so" };
    let module_path = case.0.join(format!("{module_name}.{suffix}"));
    fs::write(
        &source_path,
        r#"fn f32_add(x: f32, y: f32) -> f32:
    return x + y

fn f32_identity(x: f32) -> f32:
    return x

fn u64_increment(x: u64) -> u64:
    return x + 1

fn u64_identity(x: u64) -> u64:
    return x
"#,
    )
    .unwrap();

    let build = Command::new(env!("CARGO_BIN_EXE_pit"))
        .arg("build")
        .arg(&source_path)
        .arg("--pymodule")
        .arg("--module-name")
        .arg(&module_name)
        .arg("-o")
        .arg(&module_path)
        .output()
        .expect("spawn pit build");
    assert!(
        build.status.success(),
        "module build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let check_path = case.0.join("check.py");
    fs::write(
        &check_path,
        format!(
            "import {module_name} as m\nassert m.f32_add(1.25, 2.5) == 3.75\nassert m.f32_identity(-0.5) == -0.5\nassert m.u64_increment(1 << 63) == (1 << 63) + 1\nassert m.u64_identity((1 << 64) - 1) == (1 << 64) - 1\n"
        ),
    )
    .unwrap();

    let check = Command::new(python)
        .arg(&check_path)
        .env("PYTHONPATH", &case.0)
        .output()
        .expect("spawn Python module check");
    assert!(
        check.status.success(),
        "module import/call failed: {}\n{}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );
}
