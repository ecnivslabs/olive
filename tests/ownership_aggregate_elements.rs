//! Aggregate literals (dict/list/struct) store element pointers, so the
//! ownership pass must never emit a drop of an operand while the aggregate
//! still references it, and must transfer ownership when the source dies at
//! creation. A violation shows up only after the freed slot is recycled --
//! the allocator's freelist stamp overwrites the first word of the body --
//! so every test builds the aggregate, churns same-size-class allocations,
//! then reads the stored string back and asserts exact content.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

fn pit_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_pit"))
}

static UNIQUE: AtomicU64 = AtomicU64::new(0);

fn run_src(src: &str) -> (String, String) {
    let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("olive_agg_elem_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("main.liv");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(src.as_bytes()).unwrap();

    let out = Command::new(pit_bin())
        .arg("run")
        .arg(&path)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit run");
    let _ = std::fs::remove_dir_all(&dir);
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn assert_late_read(src: &str, expect: &str) {
    let (stdout, stderr) = run_src(src);
    assert!(
        stdout.contains(&format!("RESULT {expect}")),
        "expected late read {expect:?}; stdout: {stdout}; stderr: {stderr}"
    );
}

/// A concat operand inside a dict literal that dies at creation: ownership
/// transfers to the dict, and nothing frees the string while the dict lives.
#[test]
fn dict_literal_concat_operand_survives_churn() {
    assert_late_read(
        "fn build() -> {str: str}:\n    \
             let headers = {\"Authorization\": \"Bearer \" + \"public\", \"User-Agent\": \"x/local\"}\n    \
             return headers\n\n\
         fn churn():\n    \
             let mut j = 0\n    \
             while j < 200:\n        \
                 let c = \"churnchurn\" + str(j % 100)\n        \
                 j = j + 1\n\n\
         fn main():\n    \
             let h = build()\n    \
             churn()\n    \
             print(\"RESULT \" + h[\"Authorization\"])\n",
        "Bearer public",
    );
}

/// The operand stays live after the aggregate is built: the source's scope
/// drop must not fire while the aggregate holds the pointer.
#[test]
fn dict_literal_operand_used_after_store_survives_churn() {
    assert_late_read(
        "fn build() -> {str: str}:\n    \
             let name = \"Bearer \" + \"public\"\n    \
             let h = {\"Authorization\": name}\n    \
             let echo = name\n    \
             return h\n\n\
         fn churn():\n    \
             let mut j = 0\n    \
             while j < 200:\n        \
                 let c = \"churnchurn\" + str(j % 100)\n        \
                 j = j + 1\n\n\
         fn main():\n    \
             let h = build()\n    \
             churn()\n    \
             print(\"RESULT \" + h[\"Authorization\"])\n",
        "Bearer public",
    );
}

#[test]
fn list_literal_concat_operand_survives_churn() {
    assert_late_read(
        "fn build() -> [str]:\n    \
             return [\"Bearer \" + \"public\", \"x/local\"]\n\n\
         fn churn():\n    \
             let mut j = 0\n    \
             while j < 200:\n        \
                 let c = \"churnchurn\" + str(j % 100)\n        \
                 j = j + 1\n\n\
         fn main():\n    \
             let l = build()\n    \
             churn()\n    \
             print(\"RESULT \" + l[0])\n",
        "Bearer public",
    );
}

/// The crank request shape: a dict held by a struct returned through two
/// frames (builder then dispatcher), matching provider.build_request.
#[test]
fn struct_dict_through_two_frames_survives_churn() {
    assert_late_read(
        "struct Req:\n    \
             url: str\n    \
             headers: {str: str}\n    \
             body: str\n\n\
         fn build(model: str) -> Req:\n    \
             let body = \"{\\\"m\\\": \" + model + \"}\"\n    \
             let cred = model\n    \
             let mut token = \"public\"\n    \
             if cred != \"\":\n        \
                 token = cred\n    \
             let mut headers = {\"User-Agent\": \"x/local\"}\n    \
             headers[\"Authorization\"] = \"Bearer \" + token\n    \
             return Req(\"https://example.invalid\", headers, body)\n\n\
         fn dispatch(model: str) -> Req:\n    \
             match model:\n        \
                 \"public\":\n            \
                     return build(model)\n        \
                 _:\n            \
                     return Req(\"\", {}, \"\")\n\n\
         fn churn():\n    \
             let mut j = 0\n    \
             while j < 200:\n        \
                 let c = \"churnchurn\" + str(j % 100)\n        \
                 j = j + 1\n\n\
         fn main():\n    \
             let req = dispatch(\"public\")\n    \
             churn()\n    \
             print(\"RESULT \" + req.headers[\"Authorization\"])\n",
        "Bearer public",
    );
}

/// A dict crossing a module boundary keeps independent ownership of its
/// string values after the producing module's frame is gone.
#[test]
fn cross_module_dict_survives_churn() {
    let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("olive_agg_mod_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("auth.liv"),
        "struct Credential:\n    key: str\n\nfn get(pid: str) -> Credential | None:\n    if pid == \"never\":\n        return Credential(\"sk-x\")\n    return None\n",
    )
    .unwrap();
    let main_src = "import auth\n\n\
        fn build() -> {str: str}:\n    \
            let cred = auth.get(\"open\")\n    \
            let mut token = \"public\"\n    \
            if cred != None:\n        \
                token = cred.key\n    \
            let mut headers = {\"User-Agent\": \"x/local\"}\n    \
            headers[\"Authorization\"] = \"Bearer \" + token\n    \
            return headers\n\n\
        fn churn():\n    \
            let mut j = 0\n    \
            while j < 200:\n        \
                let c = \"churnchurn\" + str(j % 100)\n        \
                j = j + 1\n\n\
        fn main():\n    \
            let h = build()\n    \
            churn()\n    \
            print(\"RESULT \" + h[\"Authorization\"])\n";
    std::fs::write(dir.join("main.liv"), main_src).unwrap();

    let out = Command::new(pit_bin())
        .arg("run")
        .arg(dir.join("main.liv"))
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit run");
    let _ = std::fs::remove_dir_all(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("RESULT Bearer public"),
        "stdout: {stdout}; stderr: {stderr}"
    );
}

#[test]
fn nested_dict_literal_with_keys_iteration_survives_churn() {
    assert_late_read(
        "import json\n\n\
         fn build() -> {str: str}:\n    \
             let payload = {\n        \
                 \"model\": \"test-model\",\n        \
                 \"stream\": True,\n        \
                 \"stream_options\": {\"include_usage\": True},\n    \
             }\n    \
             let body = json.dumps(payload)\n    \
             let headers = {\"Authorization\": \"Bearer \" + \"public\", \"User-Agent\": \"x/local\"}\n    \
             return headers\n\n\
         fn churn():\n    \
             let mut j = 0\n    \
             while j < 200:\n        \
                 let c = \"churnchurn\" + str(j % 100)\n        \
                 j = j + 1\n\n\
         fn main():\n    \
             let h = build()\n    \
             churn()\n    \
             print(\"RESULT \" + h[\"Authorization\"])\n",
        "Bearer public",
    );
}

/// A for-loop variable over a container borrows the container's element
/// storage (`__olive_next` returns the raw slot pointer, no incref), so
/// appending it to a list that outlives the iterable must deep-copy.
#[test]
fn for_loop_element_appended_to_surviving_list_survives_churn() {
    assert_late_read(
        "import json\nimport string\n\n\
         fn build() -> list:\n    \
             let mut out: list = []\n    \
             for l in json_lines():\n        \
                 if len(l) == 0:\n            \
                     continue\n        \
                 out.append(l)\n    \
             return out\n\n\
         fn json_lines() -> list:\n    \
             return string.split(raw_text(), \"\\n\")\n\n\
         fn raw_text() -> str:\n    \
             return \"{\\\"k\\\": 1}\\n{\\\"k\\\": 2}\\n\"\n\n\
         fn churn():\n    \
             let mut j = 0\n    \
             while j < 200:\n        \
                 let c = \"churnchurn\" + str(j % 100)\n        \
                 j = j + 1\n\n\
         fn main():\n    \
             let l = build()\n    \
             churn()\n    \
             let d = json.loads(l[0])\n    \
             print(\"RESULT \" + str(d[\"k\"]))\n",
        "1",
    );
}

/// `d.get(k, default)` returns the stored word without an incref (a borrow
/// of the dict's storage), so a typed result stored into a surviving list
/// must transfer through a copy, not the raw slot.
#[test]
fn typed_dict_get_stored_into_list_survives_churn() {
    assert_late_read(
        "fn build() -> [str]:\n    \
             let d = {\"k\": \"alpha-value\"}\n    \
             let mut out: [str] = []\n    \
             out.append(d.get(\"k\", \"\"))\n    \
             out.append(d.get(\"missing\", \"beta-value\"))\n    \
             return out\n\n\
         fn churn():\n    \
             let mut j = 0\n    \
             while j < 200:\n        \
                 let c = \"churnchurn\" + str(j % 100)\n        \
                 j = j + 1\n\n\
         fn main():\n    \
             let l = build()\n    \
             churn()\n    \
             print(\"RESULT \" + l[0] + \"|\" + l[1])\n",
        "alpha-value|beta-value",
    );
}

/// The Any-typed shape behind JSON parsing: `.get()` on an untyped value,
/// then appended into a list that outlives the source dict.
#[test]
fn any_dict_get_stored_into_list_survives_churn() {
    assert_late_read(
        "import json\n\n\
         fn build() -> list:\n    \
             let d = json.loads(\"{\\\"k\\\": \\\"gamma-value\\\"}\")\n    \
             let mut out: list = []\n    \
             out.append(d.get(\"k\", \"\"))\n    \
             return out\n\n\
         fn churn():\n    \
             let mut j = 0\n    \
             while j < 200:\n        \
                 let c = \"churnchurn\" + str(j % 100)\n        \
                 j = j + 1\n\n\
         fn main():\n    \
             let l = build()\n    \
             churn()\n    \
             print(\"RESULT \" + l[0])\n",
        "gamma-value",
    );
}
