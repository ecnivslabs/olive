use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

fn pit_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_pit"))
}

static UNIQUE: AtomicU64 = AtomicU64::new(0);

fn temp_test_dir(prefix: &str) -> PathBuf {
    let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("{prefix}_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn test_mod_liv_directory_facade() {
    let dir = temp_test_dir("olive_mod_liv_facade");
    let pkg_dir = dir.join("math_pkg");
    std::fs::create_dir_all(&pkg_dir).unwrap();

    let mod_src = "struct Vector2:\n    x: int\n    y: int\n\nfn add(a: int, b: int) -> int:\n    return a + b\n";
    std::fs::write(pkg_dir.join("mod.liv"), mod_src).unwrap();

    let main_src = "import math_pkg\n\nfn main():\n    let v = math_pkg.Vector2(10, 20)\n    let sum = math_pkg.add(v.x, v.y)\n    print(sum)\n";
    let main_path = dir.join("main.liv");
    std::fs::write(&main_path, main_src).unwrap();

    let out = Command::new(pit_bin())
        .arg("run")
        .arg(&main_path)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit run");

    let _ = std::fs::remove_dir_all(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "execution failed: {stderr}");
    assert_eq!(stdout.trim(), "30", "stdout: {stdout}; stderr: {stderr}");
}

#[test]
fn test_mod_liv_optional_submodule_chained() {
    let dir = temp_test_dir("olive_opt_submod");
    let pkg_dir = dir.join("tokenizer");
    std::fs::create_dir_all(&pkg_dir).unwrap();

    let bpe_src = "fn encode(s: str) -> str:\n    return \"encoded:\" + s\n";
    std::fs::write(pkg_dir.join("bpe.liv"), bpe_src).unwrap();

    let model_src =
        "struct Config:\n    dim: int\n\nfn default_config() -> Config:\n    return Config(512)\n";
    std::fs::write(pkg_dir.join("model.liv"), model_src).unwrap();

    let main_src = "import tokenizer\n\nfn main():\n    let e = tokenizer.bpe.encode(\"hello\")\n    let cfg = tokenizer.model.default_config()\n    print(e)\n    print(cfg.dim)\n";
    let main_path = dir.join("main.liv");
    std::fs::write(&main_path, main_src).unwrap();

    let out = Command::new(pit_bin())
        .arg("run")
        .arg(&main_path)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit run");

    let _ = std::fs::remove_dir_all(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "execution failed: {stderr}");
    assert!(
        stdout.contains("encoded:hello"),
        "stdout: {stdout}; stderr: {stderr}"
    );
    assert!(stdout.contains("512"), "stdout: {stdout}; stderr: {stderr}");
}

#[test]
fn test_mod_liv_reexport() {
    let dir = temp_test_dir("olive_mod_reexport");
    let pkg_dir = dir.join("nlp");
    std::fs::create_dir_all(&pkg_dir).unwrap();

    let bpe_src = "fn bpe_encode(s: str) -> str:\n    return \"bpe:\" + s\n";
    std::fs::write(pkg_dir.join("bpe.liv"), bpe_src).unwrap();

    let mod_src =
        "from bpe import bpe_encode\n\nfn run(s: str) -> str:\n    return bpe_encode(s)\n";
    std::fs::write(pkg_dir.join("mod.liv"), mod_src).unwrap();

    let main_src = "import nlp\n\nfn main():\n    print(nlp.run(\"test\"))\n";
    let main_path = dir.join("main.liv");
    std::fs::write(&main_path, main_src).unwrap();

    let out = Command::new(pit_bin())
        .arg("run")
        .arg(&main_path)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit run");

    let _ = std::fs::remove_dir_all(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "execution failed: {stderr}");
    assert_eq!(
        stdout.trim(),
        "bpe:test",
        "stdout: {stdout}; stderr: {stderr}"
    );
}

#[test]
fn test_mod_liv_name_builtin() {
    let dir = temp_test_dir("olive_mod_name");
    let pkg_dir = dir.join("pkg_test");
    std::fs::create_dir_all(&pkg_dir).unwrap();

    let mod_src = "fn get_name() -> str:\n    return __name__\n";
    std::fs::write(pkg_dir.join("mod.liv"), mod_src).unwrap();

    let main_src = "import pkg_test\n\nfn main():\n    print(pkg_test.get_name())\n";
    let main_path = dir.join("main.liv");
    std::fs::write(&main_path, main_src).unwrap();

    let out = Command::new(pit_bin())
        .arg("run")
        .arg(&main_path)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit run");

    let _ = std::fs::remove_dir_all(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "execution failed: {stderr}");
    assert_eq!(
        stdout.trim(),
        "pkg_test",
        "stdout: {stdout}; stderr: {stderr}"
    );
}

#[test]
fn test_mod_liv_ambiguity_e0302() {
    let dir = temp_test_dir("olive_mod_ambig");
    let pkg_dir = dir.join("conflict");
    std::fs::create_dir_all(&pkg_dir).unwrap();

    std::fs::write(pkg_dir.join("mod.liv"), "fn foo():\n    pass\n").unwrap();
    std::fs::write(dir.join("conflict.liv"), "fn foo():\n    pass\n").unwrap();

    let main_src = "import conflict\n\nfn main():\n    pass\n";
    let main_path = dir.join("main.liv");
    std::fs::write(&main_path, main_src).unwrap();

    let out = Command::new(pit_bin())
        .arg("run")
        .arg(&main_path)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit run");

    let _ = std::fs::remove_dir_all(&dir);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "expected failure on ambiguous module definition"
    );
    assert!(
        stderr.contains("E0302"),
        "expected E0302 in stderr, got: {stderr}"
    );
    assert!(
        stderr.contains("ambiguous module"),
        "expected ambiguous module in stderr, got: {stderr}"
    );
}

#[test]
fn test_from_import_submodule() {
    let dir = temp_test_dir("olive_from_submod");
    let pkg_dir = dir.join("pkg");
    std::fs::create_dir_all(&pkg_dir).unwrap();

    let sub_src = "fn compute(x: int) -> int:\n    return x * 10\n";
    std::fs::write(pkg_dir.join("sub.liv"), sub_src).unwrap();

    let main_src = "from pkg.sub import compute\n\nfn main():\n    print(compute(5))\n";
    let main_path = dir.join("main.liv");
    std::fs::write(&main_path, main_src).unwrap();

    let out = Command::new(pit_bin())
        .arg("run")
        .arg(&main_path)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit run");

    let _ = std::fs::remove_dir_all(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "execution failed: {stderr}");
    assert_eq!(stdout.trim(), "50", "stdout: {stdout}; stderr: {stderr}");
}
