use crate::{OliveObj, olive_str_from_ptr, olive_str_internal};
use rustc_hash::FxHashMap as HashMap;
use std::io::{Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicI32, AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const STDIO_PIPE: i64 = 0;
pub const STDIO_INHERIT: i64 = 1;
pub const STDIO_NULL: i64 = 2;

pub const POLL_RUNNING: i64 = 0;
pub const POLL_EXITED: i64 = 1;
pub const POLL_UNKNOWN: i64 = -1;

pub const WAIT_TIMEOUT: i64 = -2;

/// Cap on buffered, undrained pipe output per stream. A child that produces
/// more than this before the caller reads keeps the newest bytes and drops
/// the rest, so a runaway child cannot exhaust host memory.
const MAX_BUFFERED_BYTES: usize = 64 * 1024 * 1024;
/// Longest a pipe read blocks for a chunk of output before re-checking
/// `done`; keeps a lost reader-thread wakeup from hanging the caller forever.
const PIPE_WAIT_SLICE: Duration = Duration::from_millis(50);
/// Interval between exit probes in `wait_timeout`.
const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(5);

struct PipeBuf {
    data: Mutex<Vec<u8>>,
    pending_utf8: Mutex<Vec<u8>>,
    cvar: Condvar,
    stop: AtomicU32,
    idle: AtomicU32,
    truncated: AtomicU32,
}

impl PipeBuf {
    fn new() -> Arc<Self> {
        Arc::new(PipeBuf {
            data: Mutex::new(Vec::new()),
            pending_utf8: Mutex::new(Vec::new()),
            cvar: Condvar::new(),
            stop: AtomicU32::new(0),
            idle: AtomicU32::new(0),
            truncated: AtomicU32::new(0),
        })
    }

    fn push(&self, chunk: &[u8]) {
        self.idle.store(0, Ordering::Release);
        let mut buf = self.data.lock().unwrap();
        if chunk.len() >= MAX_BUFFERED_BYTES {
            self.truncated.store(1, Ordering::Release);
            buf.clear();
            buf.extend_from_slice(&chunk[chunk.len() - MAX_BUFFERED_BYTES..]);
        } else {
            let overflow = buf
                .len()
                .saturating_add(chunk.len())
                .saturating_sub(MAX_BUFFERED_BYTES);
            if overflow > 0 {
                self.truncated.store(1, Ordering::Release);
                buf.drain(..overflow);
            }
            buf.extend_from_slice(chunk);
        }
        self.cvar.notify_all();
    }

    /// Blocks until at least one byte is buffered or the stream's reader has
    /// finished, whichever comes first. Bounded waits guard against a reader
    /// thread that died without flipping `done`.
    fn wait_for_data(&self, done: &AtomicU32) {
        let mut buf = self.data.lock().unwrap();
        while buf.is_empty()
            && done.load(Ordering::SeqCst) == 0
            && self.stop.load(Ordering::Acquire) == 0
        {
            let (b, _) = self.cvar.wait_timeout(buf, PIPE_WAIT_SLICE).unwrap();
            buf = b;
        }
    }

    fn take_text(&self, done: &AtomicU32, final_snapshot: bool) -> String {
        let mut data = self.data.lock().unwrap();
        let mut pending = self.pending_utf8.lock().unwrap();
        let bytes = std::mem::take(&mut *data);
        let mut combined = std::mem::take(&mut *pending);
        combined.extend_from_slice(&bytes);
        match std::str::from_utf8(&combined) {
            Ok(text) => text.to_string(),
            Err(error) => {
                let valid = error.valid_up_to();
                let mut text = String::from_utf8_lossy(&combined[..valid]).into_owned();
                let remainder = combined[valid..].to_vec();
                if !final_snapshot
                    && done.load(Ordering::SeqCst) == 0
                    && remainder.len() <= 3
                    && error.error_len().is_none()
                {
                    *pending = remainder;
                } else {
                    text.push_str(&String::from_utf8_lossy(&remainder));
                }
                text
            }
        }
    }

    fn wait_after_exit(&self, done: &AtomicU32, deadline: Instant) {
        let quiet_required = Duration::from_millis(50);
        let mut idle_since = None;
        while done.load(Ordering::Acquire) == 0
            && self.stop.load(Ordering::Acquire) == 0
            && Instant::now() < deadline
        {
            if self.idle.load(Ordering::Acquire) != 0 {
                let since = *idle_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= quiet_required {
                    break;
                }
            } else {
                idle_since = None;
            }
            drop(
                self.cvar
                    .wait_timeout(self.data.lock().unwrap(), WAIT_POLL_INTERVAL)
                    .unwrap()
                    .0,
            );
        }
    }

    fn was_truncated(&self) -> bool {
        self.truncated.load(Ordering::Acquire) != 0
    }

    fn request_stop(&self) {
        self.stop.store(1, Ordering::Release);
        self.cvar.notify_all();
    }

    /// Wakes `wait_for_data` sleepers when the reader finishes with the pipe
    /// still empty (child produced nothing).
    fn notify_drained(&self) {
        self.cvar.notify_all();
    }
}

#[cfg(unix)]
fn spawn_reader(
    mut src: impl Read + std::os::fd::AsRawFd + Send + 'static,
    buf: Arc<PipeBuf>,
    done: Arc<AtomicU32>,
) -> std::thread::JoinHandle<()> {
    let fd = src.as_raw_fd();
    let current_flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if current_flags >= 0 {
        unsafe {
            libc::fcntl(fd, libc::F_SETFL, current_flags | libc::O_NONBLOCK);
        }
    }
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            if buf.stop.load(Ordering::Acquire) != 0 {
                break;
            }
            match src.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => buf.push(&chunk[..n]),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    buf.idle.store(1, Ordering::Release);
                    buf.notify_drained();
                    std::thread::sleep(WAIT_POLL_INTERVAL);
                }
                Err(_) => break,
            }
        }
        done.store(1, Ordering::Release);
        buf.notify_drained();
    })
}

#[cfg(windows)]
fn spawn_reader(
    mut src: impl Read + std::os::windows::io::AsRawHandle + Send + 'static,
    buf: Arc<PipeBuf>,
    done: Arc<AtomicU32>,
) -> std::thread::JoinHandle<()> {
    use std::os::windows::io::AsRawHandle;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn PeekNamedPipe(
            handle: *mut std::ffi::c_void,
            buffer: *mut std::ffi::c_void,
            buffer_size: u32,
            bytes_read: *mut u32,
            bytes_available: *mut u32,
            left: *mut u32,
        ) -> i32;
    }

    std::thread::spawn(move || {
        let handle = src.as_raw_handle();
        let mut chunk = [0u8; 8192];
        loop {
            if buf.stop.load(Ordering::Acquire) != 0 {
                break;
            }
            let mut available = 0u32;
            let ready = unsafe {
                PeekNamedPipe(
                    handle as *mut std::ffi::c_void,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    &mut available,
                    std::ptr::null_mut(),
                )
            };
            if ready == 0 {
                break;
            }
            if available == 0 {
                buf.idle.store(1, Ordering::Release);
                buf.notify_drained();
                std::thread::sleep(WAIT_POLL_INTERVAL);
                continue;
            }
            match src.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => buf.push(&chunk[..n]),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        done.store(1, Ordering::Release);
        buf.notify_drained();
    })
}

#[cfg(not(any(unix, windows)))]
fn spawn_reader(
    mut src: impl Read + Send + 'static,
    buf: Arc<PipeBuf>,
    done: Arc<AtomicU32>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        while buf.stop.load(Ordering::Acquire) == 0 {
            match src.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => buf.push(&chunk[..n]),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        done.store(1, Ordering::Release);
        buf.notify_drained();
    })
}

const EXIT_UNKNOWN: i64 = i64::MIN;

struct ChildShared {
    child: Mutex<Child>,
    exit_code: AtomicI64,
    signal_code: AtomicI32,
}

impl ChildShared {
    fn new(child: Child) -> Self {
        Self {
            child: Mutex::new(child),
            exit_code: AtomicI64::new(EXIT_UNKNOWN),
            signal_code: AtomicI32::new(0),
        }
    }

    fn exit_code(&self) -> Option<i64> {
        let code = self.exit_code.load(Ordering::Acquire);
        (code != EXIT_UNKNOWN).then_some(code)
    }
}

struct ChildEntry {
    child: Arc<ChildShared>,
    stdin: Arc<Mutex<Option<std::process::ChildStdin>>>,
    stdout_buf: Arc<PipeBuf>,
    stderr_buf: Arc<PipeBuf>,
    stdout_done: Arc<AtomicU32>,
    stderr_done: Arc<AtomicU32>,
    stdout_reader: Option<std::thread::JoinHandle<()>>,
    stderr_reader: Option<std::thread::JoinHandle<()>>,
    drain_deadline: Arc<Mutex<Option<Instant>>>,
}

fn table() -> &'static Mutex<HashMap<i64, ChildEntry>> {
    static TABLE: OnceLock<Mutex<HashMap<i64, ChildEntry>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::default()))
}

fn child_for(handle: i64) -> Option<Arc<ChildShared>> {
    table()
        .lock()
        .unwrap()
        .get(&handle)
        .map(|e| e.child.clone())
}

fn stdin_for(handle: i64) -> Option<Arc<Mutex<Option<std::process::ChildStdin>>>> {
    table()
        .lock()
        .unwrap()
        .get(&handle)
        .map(|e| e.stdin.clone())
}

fn next_handle() -> i64 {
    static NEXT: AtomicI64 = AtomicI64::new(1);
    NEXT.fetch_add(1, Ordering::SeqCst)
}

fn stdio_for(mode: i64) -> Option<Stdio> {
    match mode {
        STDIO_PIPE => Some(Stdio::piped()),
        STDIO_INHERIT => Some(Stdio::inherit()),
        STDIO_NULL => Some(Stdio::null()),
        _ => None,
    }
}

fn list_str_vec(ptr: i64) -> Vec<String> {
    if ptr == 0 {
        return Vec::new();
    }
    let v = unsafe { &*(ptr as *const crate::StableVec) };
    let items = unsafe { std::slice::from_raw_parts(v.ptr, v.len) };
    items.iter().map(|&p| olive_str_from_ptr(p)).collect()
}

fn obj_str_pairs(ptr: i64) -> Vec<(String, String)> {
    if ptr == 0 {
        return Vec::new();
    }
    let obj = unsafe { &*(ptr as *const OliveObj) };
    obj.fields
        .iter()
        .filter_map(|(k, &v)| {
            crate::olive_str_as_str(k.0).map(|k| (k.to_string(), olive_str_from_ptr(v)))
        })
        .collect()
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_process_shell_argv(command: i64) -> i64 {
    if command == 0 {
        return 0;
    }
    let command = olive_str_from_ptr(command);
    #[cfg(windows)]
    let parts = ["cmd.exe".to_string(), "/C".to_string(), command];
    #[cfg(not(windows))]
    let parts = ["sh".to_string(), "-c".to_string(), command];
    let ptrs = parts
        .iter()
        .map(|part| olive_str_internal(part))
        .collect::<Vec<_>>();
    crate::list::list_from_vec(ptrs)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_process_spawn(
    argv_ptr: i64,
    cwd_ptr: i64,
    env_ptr: i64,
    env_clear: i64,
    stdin_mode: i64,
    stdout_mode: i64,
    stderr_mode: i64,
) -> i64 {
    let argv = list_str_vec(argv_ptr);
    if argv.is_empty() {
        return 0;
    }

    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]);

    let cwd = olive_str_from_ptr(cwd_ptr);
    if !cwd.is_empty() {
        cmd.current_dir(cwd);
    }

    if env_clear != 0 {
        cmd.env_clear();
    }
    for (k, v) in obj_str_pairs(env_ptr) {
        cmd.env(k, v);
    }

    let Some(stdin) = stdio_for(stdin_mode) else {
        return 0;
    };
    let Some(stdout) = stdio_for(stdout_mode) else {
        return 0;
    };
    let Some(stderr) = stdio_for(stderr_mode) else {
        return 0;
    };
    cmd.stdin(stdin);
    cmd.stdout(stdout);
    cmd.stderr(stderr);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return 0,
    };

    let stdout_buf = PipeBuf::new();
    let stderr_buf = PipeBuf::new();
    let stdout_done = Arc::new(AtomicU32::new(0));
    let stderr_done = Arc::new(AtomicU32::new(0));
    let mut stdout_reader = None;
    let mut stderr_reader = None;

    if stdout_mode == STDIO_PIPE {
        if let Some(out) = child.stdout.take() {
            stdout_reader = Some(spawn_reader(out, stdout_buf.clone(), stdout_done.clone()));
        } else {
            stdout_done.store(1, Ordering::SeqCst);
        }
    } else {
        stdout_done.store(1, Ordering::SeqCst);
    }

    if stderr_mode == STDIO_PIPE {
        if let Some(err) = child.stderr.take() {
            stderr_reader = Some(spawn_reader(err, stderr_buf.clone(), stderr_done.clone()));
        } else {
            stderr_done.store(1, Ordering::SeqCst);
        }
    } else {
        stderr_done.store(1, Ordering::SeqCst);
    }

    let stdin = Arc::new(Mutex::new(child.stdin.take()));
    let child = Arc::new(ChildShared::new(child));
    let handle = next_handle();

    table().lock().unwrap().insert(
        handle,
        ChildEntry {
            child,
            stdin,
            stdout_buf,
            stderr_buf,
            stdout_done,
            stderr_done,
            stdout_reader,
            stderr_reader,
            drain_deadline: Arc::new(Mutex::new(None)),
        },
    );

    handle
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_process_pid(handle: i64) -> i64 {
    let Some(child) = child_for(handle) else {
        return -1;
    };
    child.child.lock().unwrap().id() as i64
}

fn record_exit(child: &ChildShared, status: ExitStatus) {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        child
            .signal_code
            .store(status.signal().unwrap_or(0), Ordering::Release);
    }
    let code = status.code().map(|code| code as u32 as i64).unwrap_or(-1);
    let _ =
        child
            .exit_code
            .compare_exchange(EXIT_UNKNOWN, code, Ordering::AcqRel, Ordering::Acquire);
}

fn wait_for_exit(child: &ChildShared, timeout: Option<Duration>) -> i64 {
    if let Some(code) = child.exit_code() {
        return code;
    }
    let deadline = match timeout {
        Some(timeout) => match Instant::now().checked_add(timeout) {
            Some(deadline) => Some(deadline),
            None => return WAIT_TIMEOUT,
        },
        None => None,
    };
    loop {
        if let Some(deadline) = deadline
            && Instant::now() >= deadline
        {
            return WAIT_TIMEOUT;
        }
        if let Some(code) = child.exit_code() {
            return code;
        }

        let mut process = match child.child.try_lock() {
            Ok(process) => process,
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                std::thread::sleep(WAIT_POLL_INTERVAL);
                continue;
            }
        };
        if let Some(deadline) = deadline
            && Instant::now() >= deadline
        {
            return WAIT_TIMEOUT;
        }
        if let Some(code) = child.exit_code() {
            return code;
        }
        let result = process.try_wait();
        drop(process);
        match result {
            Ok(Some(status)) => {
                record_exit(child, status);
                return child.exit_code().unwrap_or(-1);
            }
            Ok(None) => {}
            Err(_) => return child.exit_code().unwrap_or(-1),
        }

        let Some(deadline) = deadline else {
            std::thread::sleep(WAIT_POLL_INTERVAL);
            continue;
        };
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return WAIT_TIMEOUT;
        }
        std::thread::sleep(WAIT_POLL_INTERVAL.min(remaining));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_process_poll(handle: i64) -> i64 {
    let Some(child) = child_for(handle) else {
        return POLL_UNKNOWN;
    };
    if child.exit_code().is_some() {
        return POLL_EXITED;
    }
    let mut process = child.child.lock().unwrap();
    match process.try_wait() {
        Ok(Some(status)) => {
            record_exit(&child, status);
            POLL_EXITED
        }
        Ok(None) => POLL_RUNNING,
        Err(_) => POLL_UNKNOWN,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_process_wait(handle: i64) -> i64 {
    let Some(child) = child_for(handle) else {
        return -1;
    };
    wait_for_exit(&child, None)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_process_wait_timeout(handle: i64, ms: i64) -> i64 {
    let Some(child) = child_for(handle) else {
        return -1;
    };
    wait_for_exit(&child, Some(Duration::from_millis(ms.max(0) as u64)))
}

/// Snapshot of one pipe's buffers taken while the table lock was held.
struct PipeSnapshot {
    child: Arc<ChildShared>,
    buf: Arc<PipeBuf>,
    done: Arc<AtomicU32>,
    drain_deadline: Arc<Mutex<Option<Instant>>>,
}

fn pipe_snapshot(handle: i64, stderr: bool) -> Option<PipeSnapshot> {
    let table = table().lock().unwrap();
    let e = table.get(&handle)?;
    Some(PipeSnapshot {
        child: e.child.clone(),
        buf: (if stderr { &e.stderr_buf } else { &e.stdout_buf }).clone(),
        done: (if stderr {
            &e.stderr_done
        } else {
            &e.stdout_done
        })
        .clone(),
        drain_deadline: e.drain_deadline.clone(),
    })
}

fn drain_pipe(snap: &PipeSnapshot) -> String {
    if snap.child.exit_code().is_none() {
        let _ = wait_for_exit(&snap.child, Some(Duration::from_millis(1)));
    }
    if snap.child.exit_code().is_some() {
        let deadline = {
            let mut shared = snap.drain_deadline.lock().unwrap();
            *shared.get_or_insert_with(|| Instant::now() + Duration::from_secs(1))
        };
        snap.buf.wait_after_exit(&snap.done, deadline);
        snap.buf
            .take_text(&snap.done, snap.done.load(Ordering::Acquire) != 0)
    } else {
        snap.buf.wait_for_data(&snap.done);
        snap.buf.take_text(&snap.done, false)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_process_read_stdout(handle: i64) -> i64 {
    let Some(snap) = pipe_snapshot(handle, false) else {
        return 0;
    };
    let text = drain_pipe(&snap);
    if text.is_empty() {
        0
    } else {
        olive_str_internal(&text)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_process_read_stderr(handle: i64) -> i64 {
    let Some(snap) = pipe_snapshot(handle, true) else {
        return 0;
    };
    let text = drain_pipe(&snap);
    if text.is_empty() {
        0
    } else {
        olive_str_internal(&text)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_process_stdout_truncated(handle: i64) -> i64 {
    table()
        .lock()
        .unwrap()
        .get(&handle)
        .is_some_and(|entry| entry.stdout_buf.was_truncated()) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_process_stderr_truncated(handle: i64) -> i64 {
    table()
        .lock()
        .unwrap()
        .get(&handle)
        .is_some_and(|entry| entry.stderr_buf.was_truncated()) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_process_write_stdin(handle: i64, data_ptr: i64) -> i64 {
    if data_ptr == 0 {
        return 0;
    }
    let Some(stdin) = stdin_for(handle) else {
        return 0;
    };
    let data = olive_str_from_ptr(data_ptr);
    let mut guard = stdin.lock().unwrap();
    match guard.as_mut() {
        Some(stdin) => {
            if stdin.write_all(data.as_bytes()).is_ok() && stdin.flush().is_ok() {
                1
            } else {
                0
            }
        }
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_process_close_stdin(handle: i64) -> i64 {
    let Some(stdin) = stdin_for(handle) else {
        return 0;
    };
    let mut guard = stdin.lock().unwrap();
    *guard = None;
    1
}

#[cfg(unix)]
fn send_signal(pid: i64, sig: libc::c_int) -> bool {
    unsafe { libc::kill(pid as libc::pid_t, sig) == 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_process_terminate(handle: i64) -> i64 {
    let Some(child) = child_for(handle) else {
        return 0;
    };
    let mut process = child.child.lock().unwrap();
    if child.exit_code().is_some() {
        return 1;
    }
    match process.try_wait() {
        Ok(Some(status)) => {
            record_exit(&child, status);
            return 1;
        }
        Ok(None) => {}
        Err(_) => return 0,
    }
    #[cfg(unix)]
    {
        if send_signal(process.id() as i64, libc::SIGTERM) {
            1
        } else {
            0
        }
    }
    #[cfg(not(unix))]
    {
        if process.kill().is_ok() { 1 } else { 0 }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_process_kill(handle: i64) -> i64 {
    let Some(child) = child_for(handle) else {
        return 0;
    };
    let mut process = child.child.lock().unwrap();
    if child.exit_code().is_some() {
        return 1;
    }
    if process.kill().is_ok() { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_process_exit_code(handle: i64) -> i64 {
    child_for(handle)
        .and_then(|child| child.exit_code())
        .unwrap_or(-1)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_process_signal_code(handle: i64) -> i64 {
    child_for(handle)
        .map(|child| child.signal_code.load(Ordering::Acquire) as i64)
        .unwrap_or(0)
}

fn stop_reader(reader: Option<std::thread::JoinHandle<()>>) {
    let Some(reader) = reader else {
        return;
    };
    let deadline = Instant::now() + Duration::from_millis(250);
    while !reader.is_finished() && Instant::now() < deadline {
        std::thread::sleep(WAIT_POLL_INTERVAL);
    }
    if reader.is_finished() {
        let _ = reader.join();
    }
}

/// then stops pipe readers so inherited descriptors cannot retain runtime
/// threads after the handle disappears.
#[unsafe(no_mangle)]
pub extern "C" fn olive_process_close(handle: i64) {
    let entry = {
        let mut table = table().lock().unwrap();
        table.remove(&handle)
    };
    let Some(entry) = entry else {
        return;
    };
    if entry.child.exit_code().is_none() {
        let should_wait = {
            let mut process = entry.child.child.lock().unwrap();
            match process.try_wait() {
                Ok(Some(status)) => {
                    record_exit(&entry.child, status);
                    false
                }
                Ok(None) | Err(_) => {
                    let _ = process.kill();
                    true
                }
            }
        };
        if should_wait {
            wait_for_exit(&entry.child, Some(Duration::from_secs(1)));
            if entry.child.exit_code().is_none() {
                let _ = wait_for_exit(&entry.child, None);
            }
        }
    }
    entry.stdout_buf.request_stop();
    entry.stderr_buf.request_stop();
    stop_reader(entry.stdout_reader);
    stop_reader(entry.stderr_reader);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::olive_str_internal;

    fn argv(parts: &[&str]) -> i64 {
        let ptrs: Vec<i64> = parts.iter().map(|s| olive_str_internal(s)).collect();
        crate::list::list_from_vec(ptrs)
    }

    #[test]
    fn spawn_missing_program_fails() {
        let h = olive_process_spawn(argv(&["__olive_test_does_not_exist__"]), 0, 0, 0, 2, 0, 0);
        assert_eq!(h, 0);
    }

    #[test]
    fn spawn_empty_argv_fails() {
        assert_eq!(olive_process_spawn(0, 0, 0, 0, 2, 0, 0), 0);
    }

    #[cfg(unix)]
    #[test]
    fn run_true_exits_zero() {
        let h = olive_process_spawn(argv(&["true"]), 0, 0, 0, 2, 2, 2);
        assert_ne!(h, 0);
        assert_eq!(olive_process_wait(h), 0);
        olive_process_close(h);
    }

    #[cfg(unix)]
    #[test]
    fn run_false_exits_nonzero() {
        let h = olive_process_spawn(argv(&["false"]), 0, 0, 0, 2, 2, 2);
        assert_eq!(olive_process_wait(h), 1);
        olive_process_close(h);
    }

    #[cfg(unix)]
    #[test]
    fn waiting_does_not_block_stdin_writes() {
        let h = olive_process_spawn(argv(&["cat"]), 0, 0, 0, 0, 0, 2);
        assert_ne!(h, 0);
        let waiter = std::thread::spawn(move || olive_process_wait(h));
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(
            olive_process_write_stdin(h, crate::olive_str_internal("hello")),
            1
        );
        assert_eq!(olive_process_close_stdin(h), 1);
        assert_eq!(waiter.join().unwrap(), 0);
        assert_eq!(
            crate::olive_str_from_ptr(olive_process_read_stdout(h)),
            "hello"
        );
        olive_process_close(h);
    }

    #[test]
    fn process_helper_sleeps_for_control_test() {
        if std::env::var_os("OLIVE_PROCESS_CONTROL_HELPER").is_some() {
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    #[test]
    fn kill_does_not_wait_for_a_blocked_waiter() {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "process::tests::process_helper_sleeps_for_control_test",
                "--nocapture",
            ])
            .env("OLIVE_PROCESS_CONTROL_HELPER", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = command.spawn().unwrap();
        let shared = Arc::new(ChildShared::new(child));
        let handle = next_handle();
        table().lock().unwrap().insert(
            handle,
            ChildEntry {
                child: shared,
                stdin: Arc::new(Mutex::new(None)),
                stdout_buf: PipeBuf::new(),
                stderr_buf: PipeBuf::new(),
                stdout_done: Arc::new(AtomicU32::new(1)),
                stderr_done: Arc::new(AtomicU32::new(1)),
                stdout_reader: None,
                stderr_reader: None,
                drain_deadline: Arc::new(Mutex::new(None)),
            },
        );

        let waiter = std::thread::spawn(move || olive_process_wait(handle));
        std::thread::sleep(Duration::from_millis(50));

        let start = Instant::now();
        let killer = std::thread::spawn(move || olive_process_kill(handle));
        assert_eq!(killer.join().unwrap(), 1);
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "kill waited for the child waiter"
        );
        assert_ne!(waiter.join().unwrap(), 0);
        olive_process_close(handle);
    }

    #[test]
    fn zero_timeout_does_not_wait_for_child_mutex() {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "process::tests::process_helper_sleeps_for_control_test",
                "--nocapture",
            ])
            .env("OLIVE_PROCESS_CONTROL_HELPER", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let shared = Arc::new(ChildShared::new(command.spawn().unwrap()));
        let handle = next_handle();
        table().lock().unwrap().insert(
            handle,
            ChildEntry {
                child: shared.clone(),
                stdin: Arc::new(Mutex::new(None)),
                stdout_buf: PipeBuf::new(),
                stderr_buf: PipeBuf::new(),
                stdout_done: Arc::new(AtomicU32::new(1)),
                stderr_done: Arc::new(AtomicU32::new(1)),
                stdout_reader: None,
                stderr_reader: None,
                drain_deadline: Arc::new(Mutex::new(None)),
            },
        );

        let process_guard = shared.child.lock().unwrap();
        let started = Instant::now();
        assert_eq!(olive_process_wait_timeout(handle, 0), WAIT_TIMEOUT);
        assert!(started.elapsed() < Duration::from_millis(100));
        drop(process_guard);
        olive_process_close(handle);
    }

    #[test]
    fn pipe_utf8_fragments_are_reassembled() {
        let buf = PipeBuf::new();
        let done = AtomicU32::new(0);
        buf.push(&[b'a', 0xe2]);
        assert_eq!(buf.take_text(&done, false), "a");
        buf.push(&[0x82, 0xac]);
        done.store(1, Ordering::SeqCst);
        assert_eq!(buf.take_text(&done, true), "€");
    }

    #[test]
    fn exited_child_preserves_utf8_fragment_while_reader_is_active() {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--help")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        child.wait().unwrap();
        let child = Arc::new(ChildShared::new(child));
        child.exit_code.store(0, Ordering::Release);
        let buf = PipeBuf::new();
        let done = Arc::new(AtomicU32::new(0));
        let snap = PipeSnapshot {
            child,
            buf: buf.clone(),
            done: done.clone(),
            drain_deadline: Arc::new(Mutex::new(Some(Instant::now()))),
        };
        buf.push(&[b'a', 0xe2]);
        assert_eq!(drain_pipe(&snap), "a");
        buf.push(&[0x82, 0xac]);
        done.store(1, Ordering::Release);
        assert_eq!(drain_pipe(&snap), "€");
    }

    #[test]
    fn pipe_cap_records_truncation() {
        let buf = PipeBuf::new();
        let done = AtomicU32::new(0);
        buf.push(&vec![b'x'; MAX_BUFFERED_BYTES + 1]);
        assert!(buf.was_truncated());
        assert_eq!(buf.take_text(&done, true).len(), MAX_BUFFERED_BYTES);
        assert!(buf.was_truncated());
    }

    #[test]
    fn invalid_stdio_modes_are_rejected_before_spawn() {
        let program = "__olive_invalid_stdio_program_that_must_not_spawn__";
        assert_eq!(olive_process_spawn(argv(&[program]), 0, 0, 0, 99, 0, 0), 0);
        assert_eq!(olive_process_spawn(argv(&[program]), 0, 0, 0, 0, -1, 0), 0);
        assert_eq!(olive_process_spawn(argv(&[program]), 0, 0, 0, 0, 0, 99), 0);
    }

    #[test]
    fn invalid_utf8_is_not_withheld_as_a_partial_sequence() {
        let buf = PipeBuf::new();
        let done = AtomicU32::new(0);
        buf.push(&[0x80]);
        assert_eq!(buf.take_text(&done, true), "\u{fffd}");
    }

    #[cfg(unix)]
    #[test]
    fn captures_stdout() {
        let h = olive_process_spawn(argv(&["echo", "hello"]), 0, 0, 0, 2, 0, 2);
        olive_process_wait(h);
        std::thread::sleep(Duration::from_millis(20));
        let out_ptr = olive_process_read_stdout(h);
        assert_ne!(out_ptr, 0);
        assert_eq!(olive_str_from_ptr(out_ptr).trim(), "hello");
        olive_process_close(h);
    }

    #[cfg(unix)]
    #[test]
    fn wait_timeout_on_slow_process() {
        let h = olive_process_spawn(argv(&["sleep", "1"]), 0, 0, 0, 2, 2, 2);
        let r = olive_process_wait_timeout(h, 10);
        assert_eq!(r, WAIT_TIMEOUT);
        olive_process_kill(h);
        olive_process_wait(h);
        olive_process_close(h);
    }

    #[cfg(unix)]
    #[test]
    fn kill_stops_process() {
        let h = olive_process_spawn(argv(&["sleep", "30"]), 0, 0, 0, 2, 2, 2);
        assert_eq!(olive_process_poll(h), POLL_RUNNING);
        assert_eq!(olive_process_kill(h), 1);
        let code = olive_process_wait(h);
        assert_ne!(code, 0);
        olive_process_close(h);
    }

    #[cfg(unix)]
    #[test]
    fn close_on_still_running_child_does_not_block() {
        let h = olive_process_spawn(argv(&["sleep", "30"]), 0, 0, 0, 2, 2, 2);
        assert_eq!(olive_process_poll(h), POLL_RUNNING);
        // Must return promptly: closing a handle for a process that never
        // exits on its own (a persistent shell session, say) must kill it
        // rather than block forever waiting for natural exit.
        olive_process_close(h);
    }

    #[test]
    fn unknown_handle_reports_error() {
        assert_eq!(olive_process_poll(999999), POLL_UNKNOWN);
        assert_eq!(olive_process_pid(999999), -1);
        assert_eq!(olive_process_wait(999999), -1);
    }

    #[cfg(unix)]
    #[test]
    fn stdin_roundtrip_with_cat() {
        let h = olive_process_spawn(argv(&["cat"]), 0, 0, 0, 0, 0, 2);
        olive_process_write_stdin(h, olive_str_internal("ping\n"));
        olive_process_close_stdin(h);
        olive_process_wait(h);
        std::thread::sleep(Duration::from_millis(20));
        let out_ptr = olive_process_read_stdout(h);
        assert_eq!(olive_str_from_ptr(out_ptr).trim(), "ping");
        olive_process_close(h);
    }
}
