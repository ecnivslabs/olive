use std::process::Command;

fn main() {
    let branch = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "main".to_string());

    let date = Command::new("date")
        .arg("+%b %e %Y, %H:%M:%S")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    println!("cargo:rustc-env=GIT_BRANCH={branch}");
    println!("cargo:rustc-env=BUILD_DATE={date}");
    println!("cargo:rerun-if-changed=src/main.rs");

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
    let target = std::env::var("TARGET").unwrap_or_default();
    let host = std::env::var("HOST").unwrap_or_default();
    let target_dir = if !target.is_empty() && target != host {
        format!("{}/target/{}/{}", manifest_dir, target, profile)
    } else {
        format!("{}/target/{}", manifest_dir, profile)
    };
    let mut search_dir = target_dir.clone();
    let mut lib_path = format!("{}/libolive_std.so", search_dir);
    if !std::path::Path::new(&lib_path).exists() {
        let deps_dir = format!("{}/deps", target_dir);
        let deps_lib = format!("{}/libolive_std.so", deps_dir);
        if std::path::Path::new(&deps_lib).exists() {
            search_dir = deps_dir;
            lib_path = deps_lib;
        }
    }

    println!("cargo::rustc-check-cfg=cfg(olive_std_linked)");
    if std::path::Path::new(&lib_path).exists() {
        println!("cargo:rustc-link-search=native={}", search_dir);
        println!("cargo:rustc-link-arg=-Wl,--no-as-needed,-lolive_std,--as-needed");
        println!("cargo:rustc-link-arg=-Wl,--enable-new-dtags,-rpath,$ORIGIN");
        println!("cargo:rustc-cfg=olive_std_linked");
    }
    println!("cargo:rerun-if-changed={}", lib_path);
}
