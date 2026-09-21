use crate::{OliveObj, olive_str_from_ptr, olive_str_internal};
use rustc_hash::FxHashMap as HashMap;
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
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
}

impl PipeBuf {
    fn new() -> Arc<Self> {
        Arc::new(PipeBuf {
            data: Mutex::new(Vec::new()),
            pending_utf8: Mutex::new(Vec::new()),
            cvar: Condvar::new(),
        })
    }

    fn push(&self, chunk: &[u8]) {
        let mut buf = self.data.lock().unwrap();
        if chunk.len() >= MAX_BUFFERED_BYTES {
            buf.clear();
            buf.extend_from_slice(&chunk[chunk.len() - MAX_BUFFERED_BYTES..]);
        } else {
            let overflow = buf
                .len()
                .saturating_add(chunk.len())
                .saturating_sub(MAX_BUFFERED_BYTES);
            if overflow > 0 {
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
        while buf.is_empty() && done.load(Ordering::SeqCst) == 0 {
            let (b, _) = self.cvar.wait_timeout(buf, PIPE_WAIT_SLICE).unwrap();
            buf = b;
        }
    }

    fn take_text(&self, done: &AtomicU32) -> String {
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
                if done.load(Ordering::SeqCst) == 0 && remainder.len() <= 3 {
                    *pending = remainder;
                } else {
                    text.push_str(&String::from_utf8_lossy(&remainder));
                }
                text
            }
        }
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

fn record_exit(child: &ChildShared, status: std::process::ExitStatus) {
    let code = status.code().unwrap_or(-1) as i64;
    let _ =
        child
            .exit_code
            .compare_exchange(EXIT_UNKNOWN, code, Ordering::AcqRel, Ordering::Acquire);
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        child
            .signal_code
            .store(status.signal().unwrap_or(0), Ordering::Release);
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
    if let Some(code) = child.exit_code() {
        return code;
    }
    let mut process = child.child.lock().unwrap();
    match process.wait() {
        Ok(status) => {
            record_exit(&child, status);
            child.exit_code().unwrap_or(-1)
        }
        Err(_) => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_process_wait_timeout(handle: i64, ms: i64) -> i64 {
    let Some(child) = child_for(handle) else {
        return -1;
    };
    let deadline = Instant::now() + Duration::from_millis(ms.max(0) as u64);
    loop {
        if let Some(code) = child.exit_code() {
            return code;
        }
        let mut process = child.child.lock().unwrap();
        match process.try_wait() {
            Ok(Some(status)) => {
                record_exit(&child, status);
                return child.exit_code().unwrap_or(-1);
            }
            Ok(None) => {}
            Err(_) => return -1,
        }
        drop(process);

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
    })
}

fn drain_pipe(snap: &PipeSnapshot) -> String {
    // Always wait for reader completion or data. A child can exit before its
    // pipe reader has drained the final bytes.
    snap.buf.wait_for_data(&snap.done);
    snap.buf.take_text(&snap.done)
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
    if child.exit_code().is_some() {
        return 1;
    }
    let process = child.child.lock().unwrap();
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
    if child.exit_code().is_some() {
        return 1;
    }
    if child.child.lock().unwrap().kill().is_ok() {
        1
    } else {
        0
    }
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

#[unsafe(no_mangle)]
/// Dropping a handle whose process is still running must not block the
/// caller waiting for it to exit on its own -- a long-lived child (a
/// persistent shell session, say) may never do that. Reap it if it has
/// already exited; otherwise kill it first, then wait, which is bounded.
pub extern "C" fn olive_process_close(handle: i64) {
    let entry = {
        let mut table = table().lock().unwrap();
        table.remove(&handle)
    };
    let Some(entry) = entry else {
        return;
    };
    if entry.child.exit_code().is_none() {
        let mut process = entry.child.child.lock().unwrap();
        if matches!(process.try_wait(), Ok(None)) {
            let _ = process.kill();
        }
        let _ = process.wait();
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
    fn pipe_utf8_fragments_are_reassembled() {
        let buf = PipeBuf::new();
        let done = AtomicU32::new(0);
        buf.push(&[b'a', 0xe2]);
        assert_eq!(buf.take_text(&done), "a");
        buf.push(&[0x82, 0xac]);
        done.store(1, Ordering::SeqCst);
        assert_eq!(buf.take_text(&done), "€");
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
