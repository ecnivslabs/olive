use rustc_hash::FxHashMap as HashMap;
use serde::Deserialize;
use std::io::Write;
use std::process::{Command, Stdio};

#[derive(Deserialize)]
struct RawInfo {
    #[serde(default)]
    status: String,
    types: Vec<String>,
    aliases: HashMap<String, String>,
    fns: HashMap<String, Vec<RawSig>>,
    #[serde(default)]
    fields: HashMap<String, HashMap<String, String>>,
    #[serde(default)]
    methods: HashMap<String, HashMap<String, Vec<RawSig>>>,
}

#[derive(Deserialize)]
struct RawSig {
    params: Vec<String>,
    ret: String,
}

pub type SigList = Vec<(Vec<String>, String)>;

pub struct PyiInfo {
    pub types: Vec<String>,
    pub aliases: HashMap<String, String>,
    pub fns: HashMap<String, SigList>,
    pub fields: HashMap<String, HashMap<String, String>>,
    pub methods: HashMap<String, HashMap<String, SigList>>,
}

const INSPECTOR: &str = include_str!("pyi_inspector.py");

/// Result of introspecting a Python module for type information.
pub enum PyiOutcome {
    /// A `.pyi` stub was found and parsed; static types are available.
    Found(PyiInfo),
    /// The module is importable but ships no stub: fall back to dynamic typing.
    NoStub,
    /// The module cannot be imported in this environment; the import will fail at runtime.
    ModuleNotFound,
    /// `python3` is not on PATH, so no introspection could run.
    Python3Missing,
    /// The inspector ran but failed; the contained string describes why.
    InspectorError(String),
}

pub fn query(module: &str) -> PyiOutcome {
    query_with(module, "python3")
}

fn query_with(module: &str, python: &str) -> PyiOutcome {
    let child = Command::new(python)
        .arg("-")
        .env("OLIVE_PYI_MODULE", module)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    let mut child = match child {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return PyiOutcome::Python3Missing,
        Err(e) => return PyiOutcome::InspectorError(e.to_string()),
    };

    if let Some(stdin) = child.stdin.as_mut() {
        let _ = stdin.write_all(INSPECTOR.as_bytes());
    }

    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => return PyiOutcome::InspectorError(e.to_string()),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("inspector exited with a non-zero status")
            .trim()
            .to_string();
        return PyiOutcome::InspectorError(detail);
    }

    let raw: RawInfo = match serde_json::from_slice(&output.stdout) {
        Ok(r) => r,
        Err(e) => return PyiOutcome::InspectorError(format!("invalid inspector output: {e}")),
    };

    match raw.status.as_str() {
        "no_module" => return PyiOutcome::ModuleNotFound,
        "no_stub" => return PyiOutcome::NoStub,
        _ => {}
    }

    if raw.types.is_empty() && raw.fns.is_empty() {
        return PyiOutcome::NoStub;
    }

    let convert_sigs =
        |sigs: Vec<RawSig>| -> SigList { sigs.into_iter().map(|s| (s.params, s.ret)).collect() };

    let fns = raw
        .fns
        .into_iter()
        .map(|(name, sigs)| (name, convert_sigs(sigs)))
        .collect();

    let methods = raw
        .methods
        .into_iter()
        .map(|(cls, mmap)| {
            let converted = mmap
                .into_iter()
                .map(|(m, sigs)| (m, convert_sigs(sigs)))
                .collect();
            (cls, converted)
        })
        .collect();

    PyiOutcome::Found(PyiInfo {
        types: raw.types,
        aliases: raw.aliases,
        fns,
        fields: raw.fields,
        methods,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn python3_available() -> bool {
        Command::new("python3")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[test]
    fn unimportable_module_reports_not_found() {
        if !python3_available() {
            return;
        }
        assert!(matches!(
            query("olive_definitely_missing_module_xyz"),
            PyiOutcome::ModuleNotFound
        ));
    }

    #[test]
    fn importable_module_without_stub_falls_back() {
        if !python3_available() {
            return;
        }
        assert!(matches!(query("json"), PyiOutcome::NoStub));
    }

    #[test]
    fn interpreter_not_found_is_python3_missing() {
        assert!(matches!(
            query_with("olive_no_such_python_xyz", "olive_no_such_python_xyz"),
            PyiOutcome::Python3Missing
        ));
    }

    #[test]
    fn failing_interpreter_is_inspector_error() {
        // `false` exists on every unix and always exits non-zero with no output.
        if Command::new("false").status().is_err() {
            return;
        }
        assert!(matches!(
            query_with("json", "false"),
            PyiOutcome::InspectorError(_)
        ));
    }
}
