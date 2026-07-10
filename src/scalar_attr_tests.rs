//! Regression tests for attribute access on a scalar: this used to silently
//! type-check (falling through to a fresh type variable) and then crash at
//! runtime, dereferencing the raw scalar bits as an object pointer.
#[cfg(test)]
use crate::test_utils::check_codes;

#[test]
fn attr_on_int_literal_rejected() {
    let codes = check_codes("fn main():\n    let a = 1.foo\n    print(a)\n");
    assert!(codes.contains(&"E0422".to_string()), "codes: {codes:?}");
}

#[test]
fn attr_on_float_literal_rejected() {
    let codes = check_codes("fn main():\n    let a = 1.5.foo\n    print(a)\n");
    assert!(codes.contains(&"E0422".to_string()), "codes: {codes:?}");
}

#[test]
fn attr_on_bool_rejected() {
    let codes = check_codes("fn main():\n    let a = True.foo\n    print(a)\n");
    assert!(codes.contains(&"E0422".to_string()), "codes: {codes:?}");
}

#[test]
fn attr_on_typed_int_var_rejected() {
    let codes = check_codes("fn main():\n    let x: int = 5\n    let a = x.foo\n    print(a)\n");
    assert!(codes.contains(&"E0422".to_string()), "codes: {codes:?}");
}

#[test]
fn cast_methods_on_scalar_still_allowed() {
    // `.int()`, `.float()`, etc. are cast methods valid on any scalar; they
    // must not be caught by the new no-field-or-method rejection.
    let codes =
        check_codes("fn main():\n    let x: int = 5\n    let a = x.float()\n    print(a)\n");
    assert!(!codes.contains(&"E0422".to_string()), "codes: {codes:?}");
}
