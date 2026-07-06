use crate::{olive_str_from_ptr, olive_str_internal};

#[unsafe(no_mangle)]
pub extern "C" fn olive_env_get(name: i64) -> i64 {
    if name == 0 {
        return 0;
    }
    match std::env::var(olive_str_from_ptr(name)) {
        Ok(val) => olive_str_internal(&val),
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_env_set(name: i64, val: i64) -> i64 {
    if name == 0 {
        return 0;
    }
    let key = olive_str_from_ptr(name);
    let value = if val == 0 {
        String::new()
    } else {
        olive_str_from_ptr(val)
    };
    unsafe { std::env::set_var(&key, &value) };
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_os_args() -> i64 {
    let ptrs: Vec<i64> = std::env::args().map(|a| olive_str_internal(&a)).collect();
    crate::list::list_from_vec(ptrs)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_os_exit(code: i64) {
    std::process::exit(code as i32);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::olive_str_internal;
    use crate::{KIND_LIST, StableVec};

    fn s(text: &str) -> i64 {
        olive_str_internal(text)
    }

    #[test]
    fn env_set_get() {
        olive_env_set(s("OLIVE_TEST_VAR"), s("hello_olive"));
        let result = olive_env_get(s("OLIVE_TEST_VAR"));
        assert_ne!(result, 0);
        assert_eq!(crate::olive_str_from_ptr(result), "hello_olive");
    }

    #[test]
    fn env_get_missing_returns_zero() {
        assert_eq!(olive_env_get(s("OLIVE_DEFINITELY_MISSING_XYZ_VAR")), 0);
    }

    #[test]
    fn os_args_returns_list() {
        let ptr = olive_os_args();
        assert_ne!(ptr, 0);
        let list = unsafe { &*(ptr as *const StableVec) };
        assert_eq!(list.kind, KIND_LIST);
        // at least the test binary name
        assert!(list.len >= 1);
    }

    #[test]
    fn env_get_null() {
        assert_eq!(olive_env_get(0), 0);
    }
}
