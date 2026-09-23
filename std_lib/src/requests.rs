use crate::{olive_str_from_ptr, olive_str_internal};
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
// 10 minutes: streaming LLM completions can sit idle for minutes between
// chunks (provider "thinking" gaps), so a shorter bound would kill live
// streams. This only bounds idle time between bytes, not total response
// length, so a dead-but-half-open connection is still cut.
const STREAM_READ_TIMEOUT: Duration = Duration::from_secs(600);

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

fn clear_last_error() {
    LAST_ERROR.with(|e| e.borrow_mut().clear());
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
        // Single attempt: POST is not idempotent, so automatic retries
        // could replay a side effect the server already applied.
        let mut req = ureq::post(&url)
            .timeout(REQUEST_TIMEOUT)
            .set("Content-Type", "application/json");
        for (k, v) in &headers {
            req = req.set(k, v);
        }
        let outcome = match req.send_bytes(body.as_bytes()) {
            Ok(resp) => match resp.into_string() {
                Ok(s) => AsyncOutcome::Ok(s),
                Err(e) => AsyncOutcome::Err(e.to_string()),
            },
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.status_text().to_string();
                AsyncOutcome::Err(format!("http {} {}", code, body))
            }
            Err(e @ ureq::Error::Transport(_)) => AsyncOutcome::Err(describe_error(&e)),
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

    let headers = match copy_headers(headers_ptr) {
        Ok(headers) => headers,
        Err(error) => {
            set_last_error(error);
            return 0;
        }
    };

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

/// Consumes only the matching terminal outcome. A mismatched accessor
/// leaves the entry in place so the other outcome stays readable; a
/// pending entry is never destroyed by a take.
#[unsafe(no_mangle)]
pub extern "C" fn olive_http_take_result(handle: i64) -> i64 {
    let mut table = async_table().lock().unwrap();
    match table.get(&handle) {
        Some(AsyncOutcome::Ok(_)) => match table.remove(&handle) {
            Some(AsyncOutcome::Ok(s)) => olive_str_internal(&s),
            _ => 0,
        },
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_http_take_error(handle: i64) -> i64 {
    let mut table = async_table().lock().unwrap();
    match table.get(&handle) {
        Some(AsyncOutcome::Err(_)) => match table.remove(&handle) {
            Some(AsyncOutcome::Err(s)) => olive_str_internal(&s),
            _ => 0,
        },
        _ => 0,
    }
}

fn copy_headers(ptr: i64) -> Result<Vec<(String, String)>, String> {
    if ptr == 0 {
        return Ok(Vec::new());
    }
    if crate::olive_is_obj(ptr) != 1 {
        return Err("headers must be a string dictionary".to_string());
    }
    let obj = unsafe { &*(ptr as *const crate::OliveObj) };
    let mut headers = Vec::with_capacity(obj.fields.len());
    for (key, &value) in &obj.fields {
        let key = crate::olive_str_as_str(key.0)
            .ok_or_else(|| "header names must be strings".to_string())?;
        let value = crate::olive_str_as_str(value)
            .ok_or_else(|| "header values must be strings".to_string())?;
        headers.push((key.to_string(), value.to_string()));
    }
    Ok(headers)
}

fn url_from_ptr(ptr: i64) -> String {
    if ptr == 0 {
        return String::new();
    }
    crate::olive_str_from_ptr(ptr)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_http_str_is_null(value: i64) -> i64 {
    (value == 0) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_http_get(url_ptr: i64) -> i64 {
    clear_last_error();
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
    clear_last_error();
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
    clear_last_error();
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
    clear_last_error();
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
    clear_last_error();
    if url_ptr == 0 {
        return 0;
    }
    let url = url_from_ptr(url_ptr);
    match ureq::delete(&url).timeout(REQUEST_TIMEOUT).call() {
        Ok(resp) => resp.status() as i64,
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_http_get_status(url_ptr: i64) -> i64 {
    clear_last_error();
    if url_ptr == 0 {
        return 0;
    }
    let url = url_from_ptr(url_ptr);
    match ureq::get(&url).timeout(REQUEST_TIMEOUT).call() {
        Ok(resp) => resp.status() as i64,
        Err(ureq::Error::Status(code, _)) => code as i64,
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_http_get_with_headers(url_ptr: i64, headers_ptr: i64) -> i64 {
    clear_last_error();
    if url_ptr == 0 {
        return 0;
    }
    let url = url_from_ptr(url_ptr);
    let headers = match copy_headers(headers_ptr) {
        Ok(headers) => headers,
        Err(error) => {
            set_last_error(error);
            return 0;
        }
    };
    let mut req = ureq::get(&url).timeout(REQUEST_TIMEOUT);
    for (key, value) in headers {
        req = req.set(&key, &value);
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

const MAX_STREAM_LINE_BYTES: usize = 1024 * 1024;
const MAX_STREAM_BUFFER_BYTES: usize = 8 * 1024 * 1024;

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
                            let mut table = match stream_table().lock() {
                                Ok(table) => table,
                                Err(_) => break,
                            };
                            let Some(state) = table.get_mut(&handle) else {
                                // Closed while reading: stop consuming.
                                break;
                            };
                            if line.len() > MAX_STREAM_LINE_BYTES
                                || state.chunk.len() + line.len() > MAX_STREAM_BUFFER_BYTES
                            {
                                read_err = Some("stream response exceeded size limits".to_string());
                                break;
                            }
                            state.chunk.push_str(&line);
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

    let headers = match copy_headers(headers_ptr) {
        Ok(headers) => headers,
        Err(error) => {
            set_last_error(error);
            return 0;
        }
    };

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
    fn malformed_header_container_is_rejected() {
        assert!(copy_headers(123).is_err());
        assert!(copy_headers(crate::list::list_from_vec(vec![])).is_err());
    }

    #[test]
    fn valid_string_headers_are_copied() {
        let headers = crate::obj::olive_obj_new();
        crate::obj::olive_obj_set(
            headers,
            crate::olive_str_internal("X-Test"),
            crate::olive_str_internal("value"),
        );
        let copied = copy_headers(headers).unwrap();
        assert_eq!(copied, vec![("X-Test".to_string(), "value".to_string())]);
        crate::obj::olive_free_obj(headers);
    }

    #[test]
    fn http_post_null_url() {
        assert_eq!(olive_http_post(0, 0), 0);
    }

    #[test]
    fn mismatched_take_preserves_the_other_outcome() {
        let ok_handle = next_handle();
        async_table()
            .lock()
            .unwrap()
            .insert(ok_handle, AsyncOutcome::Ok("body".to_string()));
        assert_eq!(olive_http_take_error(ok_handle), 0);
        assert_eq!(olive_http_poll(ok_handle), 1);
        assert_ne!(olive_http_take_result(ok_handle), 0);
        assert_eq!(olive_http_poll(ok_handle), -1);

        let err_handle = next_handle();
        async_table()
            .lock()
            .unwrap()
            .insert(err_handle, AsyncOutcome::Err("boom".to_string()));
        assert_eq!(olive_http_take_result(err_handle), 0);
        assert_eq!(olive_http_poll(err_handle), 2);
        assert_ne!(olive_http_take_error(err_handle), 0);
        assert_eq!(olive_http_poll(err_handle), -1);
    }

    #[test]
    fn take_never_destroys_a_pending_entry() {
        let handle = next_handle();
        async_table()
            .lock()
            .unwrap()
            .insert(handle, AsyncOutcome::Pending);
        assert_eq!(olive_http_take_result(handle), 0);
        assert_eq!(olive_http_take_error(handle), 0);
        assert_eq!(olive_http_poll(handle), 0);
        async_table().lock().unwrap().remove(&handle);
    }

    #[test]
    fn new_operation_clears_stale_error() {
        set_last_error("stale".to_string());
        olive_http_get(0);
        let err = olive_http_last_error();
        assert_eq!(crate::olive_str_from_ptr(err), "");
    }
}
