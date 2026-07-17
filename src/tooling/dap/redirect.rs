//! Debuggee stdout/stderr capture, shared by both frontends (`server.rs`'s
//! DAP framing and `headless.rs`'s newline-JSON). Protocol frames of either
//! kind travel over the process's real fd 1, the same descriptor a launched
//! program's `print()` writes to, so the two would corrupt each other on
//! the wire without this: before spawning the debuggee, fd 1 and fd 2 get
//! pointed at fresh pipes, and two pump threads hand whatever the debuggee
//! writes to a caller-supplied callback -- each frontend formats that as its
//! own `output` event. Restoring the original fds on debuggee exit lets the
//! process's own `eprintln!` diagnostics behave normally afterward.

use std::io::{self, Read, Write};
use std::sync::Arc;
use std::thread::JoinHandle;

type RawFd = i32;

/// A CRT file descriptor read/written directly through libc, sidestepping
/// `std::fs::File`'s raw-handle constructor which only exists on Unix (on
/// Windows `File` wraps a `HANDLE`, not the int fd `libc::pipe`/`dup` hand
/// back). Same libc calls work on both platforms once construction and
/// `pipe()`'s signature are the parts that differ per-target.
pub struct FdFile(RawFd);

impl FdFile {
    /// # Safety
    /// `fd` must be an open, owned CRT descriptor; ownership passes to the
    /// returned `FdFile`, which closes it on drop.
    pub unsafe fn new(fd: RawFd) -> Self {
        Self(fd)
    }
}

impl Read for FdFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = unsafe { libc::read(self.0, buf.as_mut_ptr() as *mut _, buf.len() as _) };
        if n < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(n as usize)
        }
    }
}

impl Write for FdFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = unsafe { libc::write(self.0, buf.as_ptr() as *const _, buf.len() as _) };
        if n < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(n as usize)
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for FdFile {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

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

#[cfg(unix)]
fn create_pipe() -> io::Result<[RawFd; 2]> {
    let mut fds = [0i32; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fds)
}

/// Windows' CRT `_pipe` takes an explicit buffer size and mode; `O_BINARY`
/// keeps the byte stream untranslated, matching Unix `pipe()` semantics.
#[cfg(windows)]
fn create_pipe() -> io::Result<[RawFd; 2]> {
    let mut fds = [0i32; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr(), 4096, libc::O_BINARY) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fds)
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
    let [read_fd, write_fd] = create_pipe()?;
    if unsafe { libc::dup2(write_fd, target_fd) } < 0 {
        let err = io::Error::last_os_error();
        unsafe {
            libc::close(read_fd);
            libc::close(write_fd);
        }
        return Err(err);
    }
    unsafe { libc::close(write_fd) };

    let mut read_file = unsafe { FdFile::new(read_fd) };
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
