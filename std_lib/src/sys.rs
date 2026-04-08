use crate::olive_str_internal;

#[unsafe(no_mangle)]
pub extern "C" fn olive_sys_hostname() -> i64 {
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        unsafe {
            if libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) == 0 {
                let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
                let hostname = std::str::from_utf8(&buf[..end]).unwrap_or("unknown");
                return olive_str_internal(hostname);
            }
        }
    }
    #[cfg(windows)]
    if let Ok(name) = std::env::var("COMPUTERNAME") {
        return olive_str_internal(&name);
    }
    olive_str_internal("unknown")
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_sys_pid() -> i64 {
    std::process::id() as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_sys_cpu_count() -> i64 {
    std::thread::available_parallelism()
        .map(|n| n.get() as i64)
        .unwrap_or(1)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_sys_platform() -> i64 {
    olive_str_internal(std::env::consts::OS)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_sys_arch() -> i64 {
    olive_str_internal(std::env::consts::ARCH)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_sys_memory_total() -> i64 {
    read_meminfo_field("MemTotal:")
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_sys_memory_free() -> i64 {
    read_meminfo_field("MemAvailable:").max(read_meminfo_field("MemFree:"))
}

fn read_meminfo_field(field: &str) -> i64 {
    let content = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    for line in content.lines() {
        if line.starts_with(field) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if let Some(kb_str) = parts.get(1)
                && let Ok(kb) = kb_str.parse::<i64>()
            {
                return kb * 1024;
            }
        }
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_sys_uptime() -> f64 {
    if let Ok(content) = std::fs::read_to_string("/proc/uptime") {
        let uptime_str = content.split_whitespace().next().unwrap_or("0");
        uptime_str.parse::<f64>().unwrap_or(0.0)
    } else {
        0.0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_sys_username() -> i64 {
    if let Ok(user) = std::env::var("USER").or_else(|_| std::env::var("USERNAME")) {
        return olive_str_internal(&user);
    }
    #[cfg(unix)]
    unsafe {
        let uid = libc::getuid();
        let pw = libc::getpwuid(uid);
        if !pw.is_null() {
            let name = std::ffi::CStr::from_ptr((*pw).pw_name).to_string_lossy();
            return olive_str_internal(&name);
        }
    }
    olive_str_internal("unknown")
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_sys_home_dir() -> i64 {
    if let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
        return olive_str_internal(&home);
    }
    olive_str_internal("")
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_sys_cwd() -> i64 {
    match std::env::current_dir() {
        Ok(p) => olive_str_internal(&p.to_string_lossy()),
        Err(_) => olive_str_internal(""),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_sys_chdir(path: i64) -> i64 {
    if path == 0 {
        return 0;
    }
    let p = crate::olive_str_from_ptr(path);
    if std::env::set_current_dir(&p).is_ok() {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from_ptr(ptr: i64) -> String {
        crate::olive_str_from_ptr(ptr)
    }

    #[test]
    fn pid_positive() {
        assert!(olive_sys_pid() > 0);
    }

    #[test]
    fn cpu_count_at_least_one() {
        assert!(olive_sys_cpu_count() >= 1);
    }

    #[test]
    fn platform_nonempty() {
        let p = from_ptr(olive_sys_platform());
        assert!(!p.is_empty());
    }

    #[test]
    fn arch_nonempty() {
        let a = from_ptr(olive_sys_arch());
        assert!(!a.is_empty());
    }

    #[test]
    fn cwd_nonempty() {
        let cwd = from_ptr(olive_sys_cwd());
        assert!(!cwd.is_empty());
    }

    #[test]
    fn hostname_nonempty() {
        let h = from_ptr(olive_sys_hostname());
        assert!(!h.is_empty());
    }
}
