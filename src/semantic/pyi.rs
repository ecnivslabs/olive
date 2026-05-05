use rustc_hash::FxHashMap as HashMap;
use serde::Deserialize;
use std::io::Write;
use std::process::{Command, Stdio};

#[derive(Deserialize)]
struct RawInfo {
    types: Vec<String>,
    aliases: HashMap<String, String>,
    fns: HashMap<String, Vec<RawSig>>,
}

#[derive(Deserialize)]
struct RawSig {
    params: Vec<String>,
    ret: String,
}

pub struct PyiInfo {
    pub types: Vec<String>,
    pub aliases: HashMap<String, String>,
    pub fns: HashMap<String, Vec<(Vec<String>, String)>>,
}

const INSPECTOR: &str = include_str!("pyi_inspector.py");

pub fn query(module: &str) -> Option<PyiInfo> {
    let mut child = Command::new("python3")
        .arg("-")
        .env("OLIVE_PYI_MODULE", module)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    {
        let stdin = child.stdin.as_mut()?;
        let _ = stdin.write_all(INSPECTOR.as_bytes());
    }

    let output = child.wait_with_output().ok()?;
    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }

    let raw: RawInfo = serde_json::from_slice(&output.stdout).ok()?;
    if raw.types.is_empty() && raw.fns.is_empty() {
        return None;
    }

    let fns = raw
        .fns
        .into_iter()
        .map(|(name, sigs)| {
            let converted = sigs.into_iter().map(|s| (s.params, s.ret)).collect();
            (name, converted)
        })
        .collect();

    Some(PyiInfo {
        types: raw.types,
        aliases: raw.aliases,
        fns,
    })
}
