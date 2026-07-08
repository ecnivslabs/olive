use crate::{OliveObj, olive_str_from_ptr, olive_str_internal};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

// Streaming responses (LLM chat completions) can legitimately run far past
// 30s, so they can't use REQUEST_TIMEOUT, which bounds the whole
// request/response lifetime. Instead the stream agent bounds only how long
// a single read may block: a connection that's actively delivering chunks
// never trips this, one that's gone dead does.
const STREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const STREAM_READ_TIMEOUT: Duration = Duration::from_secs(60);

fn stream_agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout_connect(STREAM_CONNECT_TIMEOUT)
            .timeout_read(STREAM_READ_TIMEOUT)
            .build()
    })
}

thread_local! {
    static LAST_ERROR: RefCell<String> = const { RefCell::new(String::new()) };
}

fn set_last_error(msg: String) {
    LAST_ERROR.with(|e| *e.borrow_mut() = msg);
}

fn describe_error(e: &ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, resp) => {
            let body = resp.status_text().to_string();
            format!("http {} {}", code, body)
        }
        ureq::Error::Transport(t) => t.to_string(),
    }
}

const MAX_RETRIES: u32 = 3;
const RETRY_BASE_DELAY: Duration = Duration::from_millis(500);
const RETRY_MAX_DELAY: Duration = Duration::from_secs(4);

fn is_retryable_status(code: u16) -> bool {
    matches!(code, 429 | 500 | 502 | 503 | 504)
}

fn retry_delay(attempt: u32) -> Duration {
    let scaled = RETRY_BASE_DELAY.saturating_mul(1 << attempt);
    if scaled > RETRY_MAX_DELAY {
        RETRY_MAX_DELAY
    } else {
        scaled
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_http_last_error() -> i64 {
    LAST_ERROR.with(|e| olive_str_internal(&e.borrow()))
}

enum AsyncOutcome {
    Pending,
    Ok(String),
    Err(String),
}

fn async_table() -> &'static Mutex<HashMap<i64, AsyncOutcome>> {
    static TABLE: OnceLock<Mutex<HashMap<i64, AsyncOutcome>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_handle() -> i64 {
    static COUNTER: AtomicI64 = AtomicI64::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Spawns a plain OS thread to perform the request. The thread only ever
/// touches owned Rust values and the async table mutex; it never calls back
/// into the olive heap/string interner, since that interner is not safe to
/// touch from a thread the olive runtime doesn't know about. The result is
/// only lifted into an olive string later, on the calling olive thread, via
/// olive_http_take_result/olive_http_take_error.
fn spawn_post_json_async(url: String, body: String, headers: Vec<(String, String)>) -> i64 {
    let handle = next_handle();
    async_table()
        .lock()
        .unwrap()
        .insert(handle, AsyncOutcome::Pending);

    thread::spawn(move || {
        let mut attempt = 0;
        let outcome = loop {
            let mut req = ureq::post(&url)
                .timeout(REQUEST_TIMEOUT)
                .set("Content-Type", "application/json");
            for (k, v) in &headers {
                req = req.set(k, v);
            }
            let result = req.send_bytes(body.as_bytes());

            match result {
                Ok(resp) => match resp.into_string() {
                    Ok(s) => break AsyncOutcome::Ok(s),
                    Err(e) => break AsyncOutcome::Err(e.to_string()),
                },
                Err(ureq::Error::Status(code, resp)) => {
                    if is_retryable_status(code) && attempt + 1 < MAX_RETRIES {
                        thread::sleep(retry_delay(attempt));
                        attempt += 1;
                        continue;
                    }
                    let body = resp.status_text().to_string();
                    break AsyncOutcome::Err(format!("http {} {}", code, body));
                }
                Err(e @ ureq::Error::Transport(_)) => {
                    break AsyncOutcome::Err(describe_error(&e));
                }
            }
        };

        if let Ok(mut table) = async_table().lock() {
            table.insert(handle, outcome);
        }
    });

    handle
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_http_post_json_async(url_ptr: i64, body_ptr: i64) -> i64 {
    if url_ptr == 0 {
        return 0;
    }
    let url = url_from_ptr(url_ptr);
    let body = if body_ptr == 0 {
        String::new()
    } else {
        olive_str_from_ptr(body_ptr)
    };
    spawn_post_json_async(url, body, Vec::new())
}

/// headers_ptr is an olive dict of string -> string, copied into owned Rust
/// values here on the calling olive thread before the background thread
/// spawns, since the background thread must never touch the olive heap.
#[unsafe(no_mangle)]
pub extern "C" fn olive_http_post_json_async_headers(
    url_ptr: i64,
    body_ptr: i64,
    headers_ptr: i64,
) -> i64 {
    if url_ptr == 0 {
        return 0;
    }
    let url = url_from_ptr(url_ptr);
    let body = if body_ptr == 0 {
        String::new()
    } else {
        olive_str_from_ptr(body_ptr)
    };

    let mut headers = Vec::new();
    if headers_ptr != 0 {
        let obj = unsafe { &*(headers_ptr as *const OliveObj) };
        for (k, &v) in &obj.fields {
            if let Some(key_str) = crate::olive_str_as_str(k.0) {
                let val = olive_str_from_ptr(v);
                headers.push((key_str.to_string(), val));
            }
        }
    }

    spawn_post_json_async(url, body, headers)
}

/// 0 = pending, 1 = ready with a body, 2 = ready with an error, -1 = unknown handle.
#[unsafe(no_mangle)]
pub extern "C" fn olive_http_poll(handle: i64) -> i64 {
    let table = async_table().lock().unwrap();
    match table.get(&handle) {
        Some(AsyncOutcome::Pending) => 0,
        Some(AsyncOutcome::Ok(_)) => 1,
        Some(AsyncOutcome::Err(_)) => 2,
        None => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_http_take_result(handle: i64) -> i64 {
    let mut table = async_table().lock().unwrap();
    match table.remove(&handle) {
        Some(AsyncOutcome::Ok(s)) => olive_str_internal(&s),
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_http_take_error(handle: i64) -> i64 {
    let mut table = async_table().lock().unwrap();
    match table.remove(&handle) {
        Some(AsyncOutcome::Err(s)) => olive_str_internal(&s),
        _ => 0,
    }
}

fn url_from_ptr(ptr: i64) -> String {
    if ptr == 0 {
        return String::new();
    }
    let p = crate::string_slab::str_body(ptr);
    let c_str = unsafe { std::ffi::CStr::from_ptr(p as *const std::ffi::c_char) };
    c_str.to_string_lossy().into_owned()
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_http_get(url_ptr: i64) -> i64 {
    if url_ptr == 0 {
        return 0;
    }
    let url = url_from_ptr(url_ptr);
    match ureq::get(&url).timeout(REQUEST_TIMEOUT).call() {
        Ok(resp) => match resp.into_string() {
            Ok(body) => olive_str_internal(&body),
            Err(e) => {
                set_last_error(e.to_string());
                0
            }
        },
        Err(e) => {
            set_last_error(describe_error(&e));
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_http_post(url_ptr: i64, body_ptr: i64) -> i64 {
    if url_ptr == 0 {
        return 0;
    }
    let url = url_from_ptr(url_ptr);
    let body = if body_ptr == 0 {
        String::new()
    } else {
        olive_str_from_ptr(body_ptr)
    };
    match ureq::post(&url)
        .timeout(REQUEST_TIMEOUT)
        .send_bytes(body.as_bytes())
    {
        Ok(resp) => match resp.into_string() {
            Ok(s) => olive_str_internal(&s),
            Err(e) => {
                set_last_error(e.to_string());
                0
            }
        },
        Err(e) => {
            set_last_error(describe_error(&e));
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_http_post_json(url_ptr: i64, body_ptr: i64) -> i64 {
    if url_ptr == 0 {
        return 0;
    }
    let url = url_from_ptr(url_ptr);
    let body = if body_ptr == 0 {
        String::new()
    } else {
        olive_str_from_ptr(body_ptr)
    };
    match ureq::post(&url)
        .timeout(REQUEST_TIMEOUT)
        .set("Content-Type", "application/json")
        .send_bytes(body.as_bytes())
    {
        Ok(resp) => match resp.into_string() {
            Ok(s) => olive_str_internal(&s),
            Err(e) => {
                set_last_error(e.to_string());
                0
            }
        },
        Err(e) => {
            set_last_error(describe_error(&e));
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_http_put(url_ptr: i64, body_ptr: i64) -> i64 {
    if url_ptr == 0 {
        return 0;
    }
    let url = url_from_ptr(url_ptr);
    let body = if body_ptr == 0 {
        String::new()
    } else {
        olive_str_from_ptr(body_ptr)
    };
    match ureq::put(&url)
        .timeout(REQUEST_TIMEOUT)
        .send_bytes(body.as_bytes())
    {
        Ok(resp) => match resp.into_string() {
            Ok(s) => olive_str_internal(&s),
            Err(e) => {
                set_last_error(e.to_string());
                0
            }
        },
        Err(e) => {
            set_last_error(describe_error(&e));
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_http_delete(url_ptr: i64) -> i64 {
    if url_ptr == 0 {
        return 0;
    }
    let url = url_from_ptr(url_ptr);
    match ureq::delete(&url).call() {
        Ok(resp) => resp.status() as i64,
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_http_get_status(url_ptr: i64) -> i64 {
    if url_ptr == 0 {
        return 0;
    }
    let url = url_from_ptr(url_ptr);
    match ureq::get(&url).call() {
        Ok(resp) => resp.status() as i64,
        Err(ureq::Error::Status(code, _)) => code as i64,
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_http_get_with_headers(url_ptr: i64, headers_ptr: i64) -> i64 {
    if url_ptr == 0 {
        return 0;
    }
    let url = url_from_ptr(url_ptr);
    let mut req = ureq::get(&url);
    if headers_ptr != 0 {
        let obj = unsafe { &*(headers_ptr as *const OliveObj) };
        for (k, &v) in &obj.fields {
            let val = crate::olive_str_from_ptr(v);
            if let Some(key_str) = crate::olive_str_as_str(k.0) {
                req = req.set(key_str, &val);
            }
        }
    }
    match req.call() {
        Ok(resp) => match resp.into_string() {
            Ok(body) => olive_str_internal(&body),
            Err(_) => 0,
        },
        Err(_) => 0,
    }
}

struct StreamState {
    chunk: String,
    done: bool,
    error: Option<String>,
}

fn stream_table() -> &'static Mutex<HashMap<i64, StreamState>> {
    static TABLE: OnceLock<Mutex<HashMap<i64, StreamState>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Spawns a plain OS thread that reads the response body line by line as it
/// arrives and appends raw SSE lines into the stream table. Like
/// spawn_post_json_async, the thread only touches owned Rust values; parsing
/// the "data: ..." payloads into olive values happens later on the calling
/// olive thread via stream_take_chunk.
fn spawn_post_json_stream(url: String, body: String, headers: Vec<(String, String)>) -> i64 {
    let handle = next_handle();
    stream_table().lock().unwrap().insert(
        handle,
        StreamState {
            chunk: String::new(),
            done: false,
            error: None,
        },
    );

    thread::spawn(move || {
        let mut req = stream_agent()
            .post(&url)
            .set("Content-Type", "application/json")
            .set("Accept", "text/event-stream");
        for (k, v) in &headers {
            req = req.set(k, v);
        }

        let error = match req.send_bytes(body.as_bytes()) {
            Ok(resp) => {
                use std::io::BufRead;
                let mut reader = std::io::BufReader::new(resp.into_reader());
                let mut line = String::new();
                let mut read_err = None;
                loop {
                    line.clear();
                    match reader.read_line(&mut line) {
                        Ok(0) => break,
                        Ok(_) => {
                            if let Ok(mut table) = stream_table().lock()
                                && let Some(state) = table.get_mut(&handle)
                            {
                                state.chunk.push_str(&line);
                            }
                        }
                        Err(e) => {
                            read_err = Some(e.to_string());
                            break;
                        }
                    }
                }
                read_err
            }
            Err(ureq::Error::Status(code, resp)) => {
                Some(format!("http {} {}", code, resp.status_text()))
            }
            Err(e @ ureq::Error::Transport(_)) => Some(describe_error(&e)),
        };

        if let Ok(mut table) = stream_table().lock()
            && let Some(state) = table.get_mut(&handle)
        {
            state.error = error;
            state.done = true;
        }
    });

    handle
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_http_stream_start(url_ptr: i64, body_ptr: i64, headers_ptr: i64) -> i64 {
    if url_ptr == 0 {
        return 0;
    }
    let url = url_from_ptr(url_ptr);
    let body = if body_ptr == 0 {
        String::new()
    } else {
        olive_str_from_ptr(body_ptr)
    };

    let mut headers = Vec::new();
    if headers_ptr != 0 {
        let obj = unsafe { &*(headers_ptr as *const OliveObj) };
        for (k, &v) in &obj.fields {
            if let Some(key_str) = crate::olive_str_as_str(k.0) {
                let val = olive_str_from_ptr(v);
                headers.push((key_str.to_string(), val));
            }
        }
    }

    spawn_post_json_stream(url, body, headers)
}

/// 0 = pending (may still gain buffered chunks), 1 = done ok, 2 = done with
/// error, -1 = unknown handle. Buffered chunks accumulate before done is
/// reached too, so callers should drain stream_take_chunk on every poll
/// regardless of status.
#[unsafe(no_mangle)]
pub extern "C" fn olive_http_stream_poll(handle: i64) -> i64 {
    let table = stream_table().lock().unwrap();
    match table.get(&handle) {
        Some(state) if !state.done => 0,
        Some(state) if state.error.is_some() => 2,
        Some(_) => 1,
        None => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_http_stream_take_chunk(handle: i64) -> i64 {
    let mut table = stream_table().lock().unwrap();
    match table.get_mut(&handle) {
        Some(state) if !state.chunk.is_empty() => {
            let chunk = std::mem::take(&mut state.chunk);
            olive_str_internal(&chunk)
        }
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_http_stream_take_error(handle: i64) -> i64 {
    let table = stream_table().lock().unwrap();
    match table.get(&handle) {
        Some(state) => match &state.error {
            Some(e) => olive_str_internal(e),
            None => 0,
        },
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_http_stream_close(handle: i64) {
    stream_table().lock().unwrap().remove(&handle);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_get_returns_zero_on_bad_url() {
        let url = crate::olive_str_internal("http://localhost:19999/nonexistent_olive_test");
        assert_eq!(olive_http_get(url), 0);
    }

    #[test]
    fn http_get_null_url() {
        assert_eq!(olive_http_get(0), 0);
    }

    #[test]
    fn http_post_null_url() {
        assert_eq!(olive_http_post(0, 0), 0);
    }
}
