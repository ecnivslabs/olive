pub mod cranelift;

#[cfg(test)]
mod tests {
    #[test]
    fn test_module_structure() {
        assert_eq!(super::cranelift::POLL_PENDING, i64::MIN);
        assert_eq!(super::cranelift::KIND_SM_FUTURE, 5);
    }

    #[test]
    fn test_ffi_fn_entry_defaults() {
        let entry = super::cranelift::FfiFnEntry {
            jit_name: "test".into(),
            c_name: "test".into(),
            params: vec![],
            ret: None,
            is_vararg: false,
            n_fixed: 0,
            call_conv: None,
            use_sret: false,
        };
        assert_eq!(entry.jit_name, "test");
        assert!(!entry.is_vararg);
    }

    #[test]
    fn test_ffi_struct_field_layout_type() {
        let layout: super::cranelift::FfiStructFieldLayout =
            ("field".into(), 0, "i64".into(), None);
        assert_eq!(layout.0, "field");
        assert_eq!(layout.1, 0);
        assert_eq!(layout.2, "i64");
        assert_eq!(layout.3, None);
    }
}
