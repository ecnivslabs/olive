#[path = "support/program.rs"]
mod program;
use program::{assert_both, assert_fault_both};

#[test]
fn file_read_returns_owned_contents() {
    assert_both(
        r#"import io

fn main():
    let path = io.temp_file()
    io.write_file(path, "payload")
    let file = io.open(path, "r")
    print(file.read())
    io.delete(path)
"#,
        "payload\n",
    );
}

#[test]
fn file_read_rejects_a_closed_handle() {
    assert_fault_both(
        r#"import io

fn main():
    let file = io.open("missing-file-for-file-read", "r")
    print(file.read())
"#,
        "[E0700]",
        "cannot read from a closed file",
    );
}

#[test]
fn read_n_reports_invalid_and_dead_handles_as_none() {
    assert_both(
        r#"import io

fn main():
    let path = io.temp_file()
    io.write_file(path, "abcdef")
    let file = io.open(path, "r")
    let value = io.read_n(file.handle, 3)
    if value == None:
        print("none")
    else:
        print(value)
    print(io.read_n(file.handle, -1) == None)
    print(io.read_n(file.handle, 9223372036854775807) == None)
    let stale = file.handle
    file.close()
    print(io.read_n(stale, 1) == None)
    io.delete(path)
"#,
        "abc\nTrue\nTrue\nTrue\n",
    );
}

#[test]
fn file_read_rejects_write_only_handles() {
    assert_fault_both(
        r#"import io

fn main():
    let path = io.temp_file()
    let file = io.open(path, "w")
    print(file.read())
    io.delete(path)
"#,
        "[E0700]",
        "file is not readable",
    );
}
