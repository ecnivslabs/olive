#[path = "support/program.rs"]
mod program;
use program::assert_both;

#[test]
fn explicit_init_parameters_are_not_matched_to_storage_fields() {
    assert_both(
        r#"struct Base:
    value: PyObject

impl Base:
    fn __init__(self, value: PyObject):
        self.value = value

struct Wrapper:
    base: Base

impl Wrapper:
    fn __init__(self, value: PyObject):
        self.base = Base(value)

fn main():
    let wrapper: Wrapper = Wrapper(None)
    print(wrapper.base.value == None)
"#,
        "True\n",
    );
}
