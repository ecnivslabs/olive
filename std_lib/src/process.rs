use crate::{OliveObj, olive_str_from_ptr, olive_str_internal};
use rustc_hash::FxHashMap as HashMap;
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
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
    cvar: Condvar,
}

impl PipeBuf {
    fn new() -> Arc<Self> {
        Arc::new(PipeBuf {
            data: Mutex::new(Vec::new()),
            cvar: Condvar::new(),
        })
    }

    fn push(&self, chunk: &[u8]) {
        let mut buf = self.data.lock().unwrap();
        if buf.len() >= MAX_BUFFERED_BYTES {
            return;
        }
        let room = MAX_BUFFERED_BYTES - buf.len();
        let take = chunk.len().min(room);
        buf.extend_from_slice(&chunk[..take]);
        if take > 0 {
            self.cvar.notify_all();
        }
    }

    /// Blocks until at least one byte is buffered or the stream's reader has
    /// finished, whichever comes first. Bounded waits guard against a reader
    /// thread that died without flipping `done`.
    fn wait_for_data(&self, done: &AtomicU32) {
        let mut buf = self.data.lock().unwrap();
        while buf.is_empty() && done.load(Ordering::SeqCst) == 0 {
            let (b, _) = self.cvar.wait_timeout(buf, PIPE_WAIT_SLICE).unwrap();
            buf = b;
        }
    }

    fn take(&self) -> Vec<u8> {
        std::mem::take(&mut self.data.lock().unwrap())
    }

    /// Wakes `wait_for_data` sleepers when the reader finishes with the pipe
    /// still empty (child produced nothing).
    fn notify_drained(&self) {
        self.cvar.notify_all();
    }
}

fn spawn_reader(mut src: impl Read + Send + 'static, buf: Arc<PipeBuf>, done: Arc<AtomicU32>) {
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match src.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => buf.push(&chunk[..n]),
                Err(_) => break,
            }
        }
        done.store(1, Ordering::SeqCst);
        buf.notify_drained();
    });
}

struct ChildEntry {
    child: Child,
    stdin: Option<std::process::ChildStdin>,
    stdout_buf: Arc<PipeBuf>,
    stderr_buf: Arc<PipeBuf>,
    stdout_done: Arc<AtomicU32>,
    stderr_done: Arc<AtomicU32>,
    exit_code: Option<i32>,
    signal_code: i32,
}

fn table() -> &'static Mutex<HashMap<i64, ChildEntry>> {
    static TABLE: OnceLock<Mutex<HashMap<i64, ChildEntry>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::default()))
}

fn next_handle() -> i64 {
    static NEXT: AtomicI64 = AtomicI64::new(1);
    NEXT.fetch_add(1, Ordering::SeqCst)
}

fn stdio_for(mode: i64) -> Stdio {
    match mode {
        STDIO_INHERIT => Stdio::inherit(),
        STDIO_NULL => Stdio::null(),
        _ => Stdio::piped(),
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

    cmd.stdin(stdio_for(stdin_mode));
    cmd.stdout(stdio_for(stdout_mode));
    cmd.stderr(stdio_for(stderr_mode));

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return 0,
    };

    let stdout_buf = PipeBuf::new();
    let stderr_buf = PipeBuf::new();
    let stdout_done = Arc::new(AtomicU32::new(0));
    let stderr_done = Arc::new(AtomicU32::new(0));

    if stdout_mode == STDIO_PIPE {
        if let Some(out) = child.stdout.take() {
            spawn_reader(out, stdout_buf.clone(), stdout_done.clone());
        } else {
            stdout_done.store(1, Ordering::SeqCst);
        }
    } else {
        stdout_done.store(1, Ordering::SeqCst);
    }

    if stderr_mode == STDIO_PIPE {
        if let Some(err) = child.stderr.take() {
            spawn_reader(err, stderr_buf.clone(), stderr_done.clone());
        } else {
            stderr_done.store(1, Ordering::SeqCst);
        }
    } else {
        stderr_done.store(1, Ordering::SeqCst);
    }

    let stdin = child.stdin.take();
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
            exit_code: None,
            signal_code: 0,
        },
    );

    handle
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_process_pid(handle: i64) -> i64 {
    let table = table().lock().unwrap();
    match table.get(&handle) {
        Some(e) => e.child.id() as i64,
        None => -1,
    }
}

fn record_exit(entry: &mut ChildEntry, status: std::process::ExitStatus) {
    entry.exit_code = Some(status.code().unwrap_or(-1));
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        entry.signal_code = status.signal().unwrap_or(0);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_process_poll(handle: i64) -> i64 {
    let mut table = table().lock().unwrap();
    let entry = match table.get_mut(&handle) {
        Some(e) => e,
        None => return POLL_UNKNOWN,
    };

    if entry.exit_code.is_some() {
        return POLL_EXITED;
    }

    match entry.child.try_wait() {
        Ok(Some(status)) => {
            record_exit(entry, status);
            POLL_EXITED
        }
        Ok(None) => POLL_RUNNING,
        Err(_) => POLL_UNKNOWN,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_process_wait(handle: i64) -> i64 {
    let mut table = table().lock().unwrap();
    let entry = match table.get_mut(&handle) {
        Some(e) => e,
        None => return -1,
    };

    if let Some(code) = entry.exit_code {
        return code as i64;
    }

    match entry.child.wait() {
        Ok(status) => {
            record_exit(entry, status);
            entry.exit_code.unwrap() as i64
        }
        Err(_) => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_process_wait_timeout(handle: i64, ms: i64) -> i64 {
    let deadline = Instant::now() + Duration::from_millis(ms.max(0) as u64);

    // Lock only for each try_wait probe; sleeping happens with the global
    // table lock released so waits on one handle never stall others.
    loop {
        {
            let mut table = table().lock().unwrap();
            let entry = match table.get_mut(&handle) {
                Some(e) => e,
                None => return -1,
            };

            if let Some(code) = entry.exit_code {
                return code as i64;
            }

            match entry.child.try_wait() {
                Ok(Some(status)) => {
                    record_exit(entry, status);
                    return entry.exit_code.unwrap() as i64;
                }
                Ok(None) => {}
                Err(_) => return -1,
            }
        }

        if Instant::now() >= deadline {
            return WAIT_TIMEOUT;
        }

        std::thread::sleep(
            WAIT_POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

/// Snapshot of one pipe's buffers taken while the table lock was held.
struct PipeSnapshot {
    buf: Arc<PipeBuf>,
    done: Arc<AtomicU32>,
    exited: bool,
}

fn pipe_snapshot(handle: i64, stderr: bool) -> Option<PipeSnapshot> {
    let table = table().lock().unwrap();
    let e = table.get(&handle)?;
    Some(PipeSnapshot {
        buf: (if stderr { &e.stderr_buf } else { &e.stdout_buf }).clone(),
        done: (if stderr {
            &e.stderr_done
        } else {
            &e.stdout_done
        })
        .clone(),
        exited: e.exit_code.is_some(),
    })
}

fn drain_pipe(snap: &PipeSnapshot) -> Vec<u8> {
    // Block briefly for the first chunk when the child is still running, so a
    // read right after spawn sees early output; never hold the table lock
    // while waiting, or spawn/poll/close of any other handle would stall.
    if !snap.exited {
        snap.buf.wait_for_data(&snap.done);
    }
    snap.buf.take()
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_process_read_stdout(handle: i64) -> i64 {
    let Some(snap) = pipe_snapshot(handle, false) else {
        return 0;
    };
    let bytes = drain_pipe(&snap);
    if bytes.is_empty() {
        0
    } else {
        olive_str_internal(&String::from_utf8_lossy(&bytes))
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_process_read_stderr(handle: i64) -> i64 {
    let Some(snap) = pipe_snapshot(handle, true) else {
        return 0;
    };
    let bytes = drain_pipe(&snap);
    if bytes.is_empty() {
        0
    } else {
        olive_str_internal(&String::from_utf8_lossy(&bytes))
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_process_write_stdin(handle: i64, data_ptr: i64) -> i64 {
    if data_ptr == 0 {
        return 0;
    }
    let data = olive_str_from_ptr(data_ptr);
    let mut table = table().lock().unwrap();
    match table.get_mut(&handle) {
        Some(e) => match &mut e.stdin {
            Some(stdin) => {
                if stdin.write_all(data.as_bytes()).is_ok() && stdin.flush().is_ok() {
                    1
                } else {
                    0
                }
            }
            None => 0,
        },
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_process_close_stdin(handle: i64) -> i64 {
    let mut table = table().lock().unwrap();
    match table.get_mut(&handle) {
        Some(e) => {
            e.stdin = None;
            1
        }
        None => 0,
    }
}

#[cfg(unix)]
fn send_signal(pid: i64, sig: libc::c_int) -> bool {
    unsafe { libc::kill(pid as libc::pid_t, sig) == 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_process_terminate(handle: i64) -> i64 {
    let mut table = table().lock().unwrap();
    let entry = match table.get_mut(&handle) {
        Some(e) => e,
        None => return 0,
    };

    if entry.exit_code.is_some() {
        return 1;
    }

    #[cfg(unix)]
    {
        if send_signal(entry.child.id() as i64, libc::SIGTERM) {
            1
        } else {
            0
        }
    }
    #[cfg(not(unix))]
    {
        if entry.child.kill().is_ok() { 1 } else { 0 }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_process_kill(handle: i64) -> i64 {
    let mut table = table().lock().unwrap();
    let entry = match table.get_mut(&handle) {
        Some(e) => e,
        None => return 0,
    };

    if entry.exit_code.is_some() {
        return 1;
    }

    if entry.child.kill().is_ok() { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_process_exit_code(handle: i64) -> i64 {
    let table = table().lock().unwrap();
    match table.get(&handle) {
        Some(e) => e.exit_code.map(|c| c as i64).unwrap_or(-1),
        None => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_process_signal_code(handle: i64) -> i64 {
    let table = table().lock().unwrap();
    match table.get(&handle) {
        Some(e) => e.signal_code as i64,
        None => 0,
    }
}

#[unsafe(no_mangle)]
/// Dropping a handle whose process is still running must not block the
/// caller waiting for it to exit on its own -- a long-lived child (a
/// persistent shell session, say) may never do that. Reap it if it has
/// already exited; otherwise kill it first, then wait, which is bounded.
pub extern "C" fn olive_process_close(handle: i64) {
    let mut entry = {
        let mut table = table().lock().unwrap();
        table.remove(&handle)
    };
    if let Some(ref mut entry) = entry
        && entry.exit_code.is_none()
    {
        if matches!(entry.child.try_wait(), Ok(None)) {
            let _ = entry.child.kill();
        }
        let _ = entry.child.wait();
    }
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

    #[test]
    fn run_true_exits_zero() {
        let h = olive_process_spawn(argv(&["true"]), 0, 0, 0, 2, 2, 2);
        assert_ne!(h, 0);
        assert_eq!(olive_process_wait(h), 0);
        olive_process_close(h);
    }

    #[test]
    fn run_false_exits_nonzero() {
        let h = olive_process_spawn(argv(&["false"]), 0, 0, 0, 2, 2, 2);
        assert_eq!(olive_process_wait(h), 1);
        olive_process_close(h);
    }

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

    #[test]
    fn wait_timeout_on_slow_process() {
        let h = olive_process_spawn(argv(&["sleep", "1"]), 0, 0, 0, 2, 2, 2);
        let r = olive_process_wait_timeout(h, 10);
        assert_eq!(r, WAIT_TIMEOUT);
        olive_process_kill(h);
        olive_process_wait(h);
        olive_process_close(h);
    }

    #[test]
    fn kill_stops_process() {
        let h = olive_process_spawn(argv(&["sleep", "30"]), 0, 0, 0, 2, 2, 2);
        assert_eq!(olive_process_poll(h), POLL_RUNNING);
        assert_eq!(olive_process_kill(h), 1);
        let code = olive_process_wait(h);
        assert_ne!(code, 0);
        olive_process_close(h);
    }

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
