//! Debuggee stdout/stderr capture, shared by both frontends (`server.rs`'s
//! DAP framing and `headless.rs`'s newline-JSON). Protocol frames of either
//! kind travel over the process's real fd 1, the same descriptor a launched
//! program's `print()` writes to, so the two would corrupt each other on
//! the wire without this: before spawning the debuggee, fd 1 and fd 2 get
//! pointed at fresh pipes, and two pump threads hand whatever the debuggee
//! writes to a caller-supplied callback -- each frontend formats that as its
//! own `output` event. Restoring the original fds on debuggee exit lets the
//! process's own `eprintln!` diagnostics behave normally afterward.

use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{FromRawFd, RawFd};
use std::sync::Arc;
use std::thread::JoinHandle;

pub struct Redirect {
    saved_stdout: RawFd,
    saved_stderr: RawFd,
    stdout_pump: Option<JoinHandle<()>>,
    stderr_pump: Option<JoinHandle<()>>,
}

impl Redirect {
    /// Must be called before the debuggee thread starts running user code
    /// (it may still be parked at `wait_for_start`, that's fine): once this
    /// returns, anything written to fd 1/2 invokes `on_chunk(category,
    /// text)` instead of reaching the real terminal. `category` is
    /// `"stdout"` or `"stderr"`.
    pub fn install<F>(on_chunk: F) -> io::Result<Self>
    where
        F: Fn(&'static str, String) + Send + Sync + 'static,
    {
        let on_chunk = Arc::new(on_chunk);
        let saved_stdout = dup(1)?;
        let saved_stderr = dup(2)?;
        let stdout_pump = spawn_pump(1, "stdout", on_chunk.clone())?;
        let stderr_pump = spawn_pump(2, "stderr", on_chunk)?;
        Ok(Self {
            saved_stdout,
            saved_stderr,
            stdout_pump: Some(stdout_pump),
            stderr_pump: Some(stderr_pump),
        })
    }

    /// Restores fd 1/2 to their pre-`install` targets. Dropping the pipes'
    /// last write-end reference (fd 1/2 themselves) is what unblocks the
    /// pump threads' read loop with EOF, so join only after the `dup2`s.
    pub fn restore(mut self) {
        unsafe {
            libc::dup2(self.saved_stdout, 1);
            libc::dup2(self.saved_stderr, 2);
            libc::close(self.saved_stdout);
            libc::close(self.saved_stderr);
        }
        if let Some(h) = self.stdout_pump.take() {
            let _ = h.join();
        }
        if let Some(h) = self.stderr_pump.take() {
            let _ = h.join();
        }
    }
}

fn dup(fd: RawFd) -> io::Result<RawFd> {
    let r = unsafe { libc::dup(fd) };
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(r)
    }
}

/// Creates a pipe, points `target_fd` at its write end, and spawns a thread
/// that reads the other end, calling `on_chunk(category, text)` per read.
/// The thread exits once `target_fd` is reassigned elsewhere (`restore`)
/// and every other reference to the write end is gone.
fn spawn_pump(
    target_fd: RawFd,
    category: &'static str,
    on_chunk: Arc<dyn Fn(&'static str, String) + Send + Sync>,
) -> io::Result<JoinHandle<()>> {
    let mut fds = [0i32; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let [read_fd, write_fd] = fds;
    if unsafe { libc::dup2(write_fd, target_fd) } < 0 {
        let err = io::Error::last_os_error();
        unsafe {
            libc::close(read_fd);
            libc::close(write_fd);
        }
        return Err(err);
    }
    unsafe { libc::close(write_fd) };

    let mut read_file = unsafe { File::from_raw_fd(read_fd) };
    Ok(std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match read_file.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let text = String::from_utf8_lossy(&buf[..n]).into_owned();
                    on_chunk(category, text);
                }
            }
        }
    }))
}
