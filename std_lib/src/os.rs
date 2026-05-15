use crate::{StableVec, KIND_LIST, olive_str_from_ptr, olive_str_internal};

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
    let value = if val == 0 { String::new() } else { olive_str_from_ptr(val) };
    unsafe { std::env::set_var(&key, &value) };
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_os_args() -> i64 {
    let mut ptrs: Vec<i64> = std::env::args().map(|a| olive_str_internal(&a)).collect();
    let ptr = ptrs.as_mut_ptr();
    let cap = ptrs.capacity();
    let len = ptrs.len();
    std::mem::forget(ptrs);
    Box::into_raw(Box::new(StableVec { kind: KIND_LIST, ptr, cap, len })) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_os_exit(code: i64) {
    std::process::exit(code as i32);
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_os_exec(cmd: i64) -> i64 {
    if cmd == 0 {
        return 0;
    }
    let cmd_str = olive_str_from_ptr(cmd);
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(&cmd_str)
        .output();
    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            olive_str_internal(&stdout)
        }
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_os_exec_status(cmd: i64) -> i64 {
    if cmd == 0 {
        return -1;
    }
    let cmd_str = olive_str_from_ptr(cmd);
    match std::process::Command::new("sh").arg("-c").arg(&cmd_str).status() {
        Ok(s) => s.code().unwrap_or(-1) as i64,
        Err(_) => -1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::olive_str_internal;

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
    fn os_exec_echo() {
        let result = olive_os_exec(s("echo hello"));
        assert_ne!(result, 0);
        let out = crate::olive_str_from_ptr(result);
        assert!(out.contains("hello"));
    }

    #[test]
    fn os_exec_status_success() {
        assert_eq!(olive_os_exec_status(s("true")), 0);
    }

    #[test]
    fn os_exec_status_failure() {
        assert_ne!(olive_os_exec_status(s("false")), 0);
    }

    #[test]
    fn env_get_null() {
        assert_eq!(olive_env_get(0), 0);
    }
}
