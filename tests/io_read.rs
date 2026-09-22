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

#[cfg(unix)]
#[test]
fn file_read_reports_io_errors_instead_of_returning_an_invalid_string() {
    assert_fault_both(
        r#"import io

fn main():
    let file = io.open(".", "r")
    print(file.read())
"#,
        "[E0700]",
        "file exceeds the maximum readable size",
    );
}
