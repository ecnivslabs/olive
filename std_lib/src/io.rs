use crate::{OliveObj, StableVec, KIND_LIST, KIND_OBJ, olive_str_from_ptr, olive_str_internal};
use rustc_hash::FxHashMap as HashMap;
use std::io::{Read, Seek, SeekFrom, Write};

#[unsafe(no_mangle)]
pub extern "C" fn olive_file_read(path: i64) -> i64 {
    if path == 0 {
        return 0;
    }
    let path_str = olive_str_from_ptr(path);
    match std::fs::read_to_string(&path_str) {
        Ok(content) => olive_str_internal(&content),
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_file_write(path: i64, data: i64) -> i64 {
    if path == 0 || data == 0 {
        return 0;
    }
    let path_str = olive_str_from_ptr(path);
    let data_str = olive_str_from_ptr(data);
    if std::fs::write(&path_str, data_str.as_bytes()).is_ok() { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_file_append(path: i64, data: i64) -> i64 {
    if path == 0 || data == 0 {
        return 0;
    }
    let path_str = olive_str_from_ptr(path);
    let data_str = olive_str_from_ptr(data);
    let mut f = match std::fs::OpenOptions::new().append(true).create(true).open(&path_str) {
        Ok(f) => f,
        Err(_) => return 0,
    };
    if f.write_all(data_str.as_bytes()).is_ok() { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_file_exists(path: i64) -> i64 {
    if path == 0 {
        return 0;
    }
    if std::path::Path::new(&olive_str_from_ptr(path)).exists() { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_file_delete(path: i64) -> i64 {
    if path == 0 {
        return 0;
    }
    let path_str = olive_str_from_ptr(path);
    let p = std::path::Path::new(&path_str);
    if p.is_dir() {
        if std::fs::remove_dir_all(p).is_ok() { 1 } else { 0 }
    } else if std::fs::remove_file(p).is_ok() {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_dir_create(path: i64) -> i64 {
    if path == 0 {
        return 0;
    }
    if std::fs::create_dir_all(&olive_str_from_ptr(path)).is_ok() { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_dir_list(path: i64) -> i64 {
    let path_str = if path == 0 { ".".to_string() } else { olive_str_from_ptr(path) };
    let entries = match std::fs::read_dir(&path_str) {
        Ok(e) => e,
        Err(_) => {
            let v = Box::new(StableVec { kind: KIND_LIST, ptr: std::ptr::null_mut(), cap: 0, len: 0 });
            return Box::into_raw(v) as i64;
        }
    };
    let mut ptrs: Vec<i64> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        ptrs.push(olive_str_internal(&name));
    }
    let ptr = ptrs.as_mut_ptr();
    let cap = ptrs.capacity();
    let len = ptrs.len();
    std::mem::forget(ptrs);
    Box::into_raw(Box::new(StableVec { kind: KIND_LIST, ptr, cap, len })) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_file_stat(path: i64) -> i64 {
    if path == 0 {
        return 0;
    }
    let path_str = olive_str_from_ptr(path);
    let meta = match std::fs::metadata(&path_str) {
        Ok(m) => m,
        Err(_) => return 0,
    };
    let mut fields = HashMap::default();
    fields.insert("size".to_string(), meta.len() as i64);
    fields.insert("is_file".to_string(), if meta.is_file() { 1 } else { 0 });
    fields.insert("is_dir".to_string(), if meta.is_dir() { 1 } else { 0 });
    fields.insert(
        "modified".to_string(),
        meta.modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
    );
    Box::into_raw(Box::new(OliveObj { kind: KIND_OBJ, fields })) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_file_copy(src: i64, dst: i64) -> i64 {
    if src == 0 || dst == 0 {
        return 0;
    }
    let src_str = olive_str_from_ptr(src);
    let dst_str = olive_str_from_ptr(dst);
    match std::fs::copy(&src_str, &dst_str) {
        Ok(_) => 1,
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_file_rename(src: i64, dst: i64) -> i64 {
    if src == 0 || dst == 0 {
        return 0;
    }
    let src_str = olive_str_from_ptr(src);
    let dst_str = olive_str_from_ptr(dst);
    if std::fs::rename(&src_str, &dst_str).is_ok() { 1 } else { 0 }
}

// Path operations

#[unsafe(no_mangle)]
pub extern "C" fn olive_path_join(a: i64, b: i64) -> i64 {
    let a_str = if a == 0 { String::new() } else { olive_str_from_ptr(a) };
    let b_str = if b == 0 { String::new() } else { olive_str_from_ptr(b) };
    let path = std::path::Path::new(&a_str).join(&b_str);
    olive_str_internal(&path.to_string_lossy())
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_path_dirname(path: i64) -> i64 {
    if path == 0 {
        return olive_str_internal(".");
    }
    let p = olive_str_from_ptr(path);
    match std::path::Path::new(&p).parent() {
        Some(parent) => {
            let s = parent.to_string_lossy();
            if s.is_empty() { olive_str_internal(".") } else { olive_str_internal(&s) }
        }
        None => olive_str_internal("."),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_path_basename(path: i64) -> i64 {
    if path == 0 {
        return olive_str_internal("");
    }
    let p = olive_str_from_ptr(path);
    match std::path::Path::new(&p).file_name() {
        Some(name) => olive_str_internal(&name.to_string_lossy()),
        None => olive_str_internal(""),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_path_ext(path: i64) -> i64 {
    if path == 0 {
        return olive_str_internal("");
    }
    let p = olive_str_from_ptr(path);
    match std::path::Path::new(&p).extension() {
        Some(ext) => olive_str_internal(&ext.to_string_lossy()),
        None => olive_str_internal(""),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_path_is_absolute(path: i64) -> i64 {
    if path == 0 {
        return 0;
    }
    let p = olive_str_from_ptr(path);
    if std::path::Path::new(&p).is_absolute() { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_path_stem(path: i64) -> i64 {
    if path == 0 {
        return olive_str_internal("");
    }
    let p = olive_str_from_ptr(path);
    match std::path::Path::new(&p).file_stem() {
        Some(stem) => olive_str_internal(&stem.to_string_lossy()),
        None => olive_str_internal(""),
    }
}

// Temp files

#[unsafe(no_mangle)]
pub extern "C" fn olive_temp_dir() -> i64 {
    olive_str_internal(&std::env::temp_dir().to_string_lossy())
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_temp_file() -> i64 {
    let tmp = std::env::temp_dir();
    let name = format!("olive_{}", uuid::Uuid::new_v4().simple());
    let path = tmp.join(name);
    // Create the file so the path is reserved
    if std::fs::File::create(&path).is_ok() {
        olive_str_internal(&path.to_string_lossy())
    } else {
        0
    }
}

// Stdin

#[unsafe(no_mangle)]
pub extern "C" fn olive_stdin_read() -> i64 {
    let mut buf = String::new();
    match std::io::stdin().read_to_string(&mut buf) {
        Ok(_) => olive_str_internal(&buf),
        Err(_) => olive_str_internal(""),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_stdin_read_line() -> i64 {
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) => olive_str_internal(""),
        Ok(_) => {
            // Strip trailing newline
            if line.ends_with('\n') {
                line.pop();
                if line.ends_with('\r') {
                    line.pop();
                }
            }
            olive_str_internal(&line)
        }
        Err(_) => olive_str_internal(""),
    }
}

// Seekable file handles
// Each handle is a Box<std::fs::File> returned as raw pointer

#[unsafe(no_mangle)]
pub extern "C" fn olive_file_open(path: i64, mode: i64) -> i64 {
    if path == 0 {
        return 0;
    }
    let path_str = olive_str_from_ptr(path);
    let mode_str = if mode == 0 { "r".to_string() } else { olive_str_from_ptr(mode) };
    let file = match mode_str.as_str() {
        "r" => std::fs::OpenOptions::new().read(true).open(&path_str),
        "w" => std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(&path_str),
        "a" => std::fs::OpenOptions::new().append(true).create(true).open(&path_str),
        "r+" => std::fs::OpenOptions::new().read(true).write(true).open(&path_str),
        "w+" => std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&path_str),
        _ => std::fs::OpenOptions::new().read(true).open(&path_str),
    };
    match file {
        Ok(f) => Box::into_raw(Box::new(f)) as i64,
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_file_close(handle: i64) {
    if handle != 0 {
        unsafe { drop(Box::from_raw(handle as *mut std::fs::File)) };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_file_read_n(handle: i64, n: i64) -> i64 {
    if handle == 0 {
        return olive_str_internal("");
    }
    let file = unsafe { &mut *(handle as *mut std::fs::File) };
    let mut buf = vec![0u8; n as usize];
    match file.read(&mut buf) {
        Ok(read) => {
            buf.truncate(read);
            let s = String::from_utf8_lossy(&buf).into_owned();
            olive_str_internal(&s)
        }
        Err(_) => olive_str_internal(""),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_file_write_str(handle: i64, data: i64) -> i64 {
    if handle == 0 || data == 0 {
        return 0;
    }
    let file = unsafe { &mut *(handle as *mut std::fs::File) };
    let data_str = olive_str_from_ptr(data);
    if file.write_all(data_str.as_bytes()).is_ok() { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_file_seek(handle: i64, offset: i64, whence: i64) -> i64 {
    if handle == 0 {
        return -1;
    }
    let file = unsafe { &mut *(handle as *mut std::fs::File) };
    let pos = match whence {
        0 => SeekFrom::Start(offset as u64),
        1 => SeekFrom::Current(offset),
        2 => SeekFrom::End(offset),
        _ => SeekFrom::Start(offset as u64),
    };
    match file.seek(pos) {
        Ok(new_pos) => new_pos as i64,
        Err(_) => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_file_tell(handle: i64) -> i64 {
    if handle == 0 {
        return -1;
    }
    let file = unsafe { &mut *(handle as *mut std::fs::File) };
    match file.stream_position() {
        Ok(pos) => pos as i64,
        Err(_) => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_file_read_lines(path: i64) -> i64 {
    let empty = || {
        Box::into_raw(Box::new(StableVec {
            kind: KIND_LIST,
            ptr: std::ptr::null_mut(),
            cap: 0,
            len: 0,
        })) as i64
    };
    if path == 0 {
        return empty();
    }
    let content = match std::fs::read_to_string(&olive_str_from_ptr(path)) {
        Ok(c) => c,
        Err(_) => return empty(),
    };
    let mut ptrs: Vec<i64> = content.lines().map(|l| olive_str_internal(l)).collect();
    let ptr = ptrs.as_mut_ptr();
    let cap = ptrs.capacity();
    let len = ptrs.len();
    std::mem::forget(ptrs);
    Box::into_raw(Box::new(StableVec { kind: KIND_LIST, ptr, cap, len })) as i64
}

// Buffered I/O

use std::io::BufRead;

struct BufReadHandle(std::io::BufReader<std::fs::File>);
struct BufWriteHandle(std::io::BufWriter<std::fs::File>);

#[unsafe(no_mangle)]
pub extern "C" fn olive_bufread_open(path: i64) -> i64 {
    if path == 0 {
        return 0;
    }
    let path_str = olive_str_from_ptr(path);
    match std::fs::File::open(&path_str) {
        Ok(f) => Box::into_raw(Box::new(BufReadHandle(std::io::BufReader::new(f)))) as i64,
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_bufread_line(br: i64) -> i64 {
    if br == 0 {
        return 0;
    }
    let handle = unsafe { &mut *(br as *mut BufReadHandle) };
    let mut line = String::new();
    match handle.0.read_line(&mut line) {
        Ok(0) => 0,
        Ok(_) => {
            if line.ends_with('\n') {
                line.pop();
                if line.ends_with('\r') {
                    line.pop();
                }
            }
            olive_str_internal(&line)
        }
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_bufread_close(br: i64) {
    if br != 0 {
        unsafe { drop(Box::from_raw(br as *mut BufReadHandle)) };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_bufwrite_open(path: i64) -> i64 {
    if path == 0 {
        return 0;
    }
    let path_str = olive_str_from_ptr(path);
    match std::fs::File::create(&path_str) {
        Ok(f) => Box::into_raw(Box::new(BufWriteHandle(std::io::BufWriter::new(f)))) as i64,
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_bufwrite_write(bw: i64, data: i64) -> i64 {
    if bw == 0 || data == 0 {
        return 0;
    }
    use std::io::Write;
    let handle = unsafe { &mut *(bw as *mut BufWriteHandle) };
    let text = olive_str_from_ptr(data);
    if handle.0.write_all(text.as_bytes()).is_ok() { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_bufwrite_flush(bw: i64) -> i64 {
    if bw == 0 {
        return 0;
    }
    use std::io::Write;
    let handle = unsafe { &mut *(bw as *mut BufWriteHandle) };
    if handle.0.flush().is_ok() { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_bufwrite_close(bw: i64) {
    if bw != 0 {
        unsafe { drop(Box::from_raw(bw as *mut BufWriteHandle)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::olive_str_internal;

    fn make_str(s: &str) -> i64 {
        olive_str_internal(s)
    }

    fn from_ptr(ptr: i64) -> String {
        crate::olive_str_from_ptr(ptr)
    }

    #[test]
    fn file_write_read_delete() {
        let path = make_str("/tmp/olive_io_test_rw.txt");
        let data = make_str("hello olive");
        assert_eq!(olive_file_write(path, data), 1);
        let result = olive_file_read(path);
        assert_ne!(result, 0);
        let content = from_ptr(result);
        assert_eq!(content, "hello olive");
        assert_eq!(olive_file_delete(path), 1);
        assert_eq!(olive_file_exists(path), 0);
    }

    #[test]
    fn file_append() {
        let path = make_str("/tmp/olive_io_test_append.txt");
        let _ = olive_file_delete(path);
        olive_file_append(path, make_str("line1\n"));
        olive_file_append(path, make_str("line2\n"));
        let content = from_ptr(olive_file_read(path));
        assert_eq!(content, "line1\nline2\n");
        olive_file_delete(path);
    }

    #[test]
    fn dir_create_list_delete() {
        let dir = make_str("/tmp/olive_io_test_dir");
        assert_eq!(olive_dir_create(dir), 1);
        let sub = make_str("/tmp/olive_io_test_dir/sub");
        assert_eq!(olive_dir_create(sub), 1);
        let list_ptr = olive_dir_list(dir);
        assert_ne!(list_ptr, 0);
        let list = unsafe { &*(list_ptr as *const StableVec) };
        assert!(list.len >= 1);
        assert_eq!(olive_file_delete(dir), 1);
    }

    #[test]
    fn file_stat_returns_obj() {
        let path = make_str("/tmp/olive_io_stat_test.txt");
        olive_file_write(path, make_str("data"));
        let obj_ptr = olive_file_stat(path);
        assert_ne!(obj_ptr, 0);
        let obj = unsafe { &*(obj_ptr as *const OliveObj) };
        assert_eq!(*obj.fields.get("is_file").unwrap(), 1);
        assert_eq!(*obj.fields.get("is_dir").unwrap(), 0);
        assert_eq!(*obj.fields.get("size").unwrap(), 4);
        olive_file_delete(path);
    }

    #[test]
    fn read_nonexistent_returns_zero() {
        let path = make_str("/tmp/olive_definitely_does_not_exist_xyz.txt");
        assert_eq!(olive_file_read(path), 0);
    }

    #[test]
    fn path_join_basic() {
        let a = make_str("/tmp");
        let b = make_str("file.txt");
        let result = from_ptr(olive_path_join(a, b));
        assert_eq!(result, "/tmp/file.txt");
    }

    #[test]
    fn path_dirname_and_basename() {
        let p = make_str("/tmp/foo/bar.txt");
        assert_eq!(from_ptr(olive_path_dirname(p)), "/tmp/foo");
        assert_eq!(from_ptr(olive_path_basename(p)), "bar.txt");
    }

    #[test]
    fn path_ext_and_stem() {
        let p = make_str("/tmp/file.tar.gz");
        assert_eq!(from_ptr(olive_path_ext(p)), "gz");
        assert_eq!(from_ptr(olive_path_stem(p)), "file.tar");
    }

    #[test]
    fn path_is_absolute() {
        assert_eq!(olive_path_is_absolute(make_str("/tmp/foo")), 1);
        assert_eq!(olive_path_is_absolute(make_str("relative/path")), 0);
    }

    #[test]
    fn temp_dir_nonempty() {
        let d = from_ptr(olive_temp_dir());
        assert!(!d.is_empty());
    }

    #[test]
    fn temp_file_creates_file() {
        let p = olive_temp_file();
        assert_ne!(p, 0);
        let path = from_ptr(p);
        assert!(!path.is_empty());
        assert_eq!(olive_file_exists(p), 1);
        olive_file_delete(p);
    }

    #[test]
    fn file_seek_and_tell() {
        let path = make_str("/tmp/olive_seek_test.txt");
        olive_file_write(path, make_str("hello world"));
        let handle = olive_file_open(path, make_str("r"));
        assert_ne!(handle, 0);
        assert_eq!(olive_file_tell(handle), 0);
        olive_file_seek(handle, 6, 0);
        assert_eq!(olive_file_tell(handle), 6);
        let chunk = from_ptr(olive_file_read_n(handle, 5));
        assert_eq!(chunk, "world");
        olive_file_close(handle);
        olive_file_delete(path);
    }

    #[test]
    fn file_read_lines_basic() {
        let path = make_str("/tmp/olive_lines_test.txt");
        olive_file_write(path, make_str("line1\nline2\nline3"));
        let list_ptr = olive_file_read_lines(path);
        assert_ne!(list_ptr, 0);
        let list = unsafe { &*(list_ptr as *const StableVec) };
        assert_eq!(list.len, 3);
        assert_eq!(from_ptr(unsafe { *list.ptr }), "line1");
        assert_eq!(from_ptr(unsafe { *list.ptr.add(1) }), "line2");
        assert_eq!(from_ptr(unsafe { *list.ptr.add(2) }), "line3");
        olive_file_delete(path);
    }

    #[test]
    fn file_read_lines_null() {
        let list_ptr = olive_file_read_lines(0);
        let list = unsafe { &*(list_ptr as *const StableVec) };
        assert_eq!(list.len, 0);
    }

    #[test]
    fn file_copy_and_rename() {
        let src = make_str("/tmp/olive_copy_src.txt");
        let dst = make_str("/tmp/olive_copy_dst.txt");
        let renamed = make_str("/tmp/olive_renamed.txt");
        olive_file_write(src, make_str("copy me"));
        assert_eq!(olive_file_copy(src, dst), 1);
        assert_eq!(from_ptr(olive_file_read(dst)), "copy me");
        assert_eq!(olive_file_rename(dst, renamed), 1);
        assert_eq!(olive_file_exists(dst), 0);
        assert_eq!(olive_file_exists(renamed), 1);
        olive_file_delete(src);
        olive_file_delete(renamed);
    }

    #[test]
    fn bufread_line_by_line() {
        let path = make_str("/tmp/olive_bufread_test.txt");
        olive_file_write(path, make_str("alpha\nbeta\ngamma"));
        let br = olive_bufread_open(path);
        assert_ne!(br, 0);
        assert_eq!(from_ptr(olive_bufread_line(br)), "alpha");
        assert_eq!(from_ptr(olive_bufread_line(br)), "beta");
        assert_eq!(from_ptr(olive_bufread_line(br)), "gamma");
        assert_eq!(olive_bufread_line(br), 0);
        olive_bufread_close(br);
        olive_file_delete(path);
    }

    #[test]
    fn bufwrite_and_flush() {
        let path = make_str("/tmp/olive_bufwrite_test.txt");
        let bw = olive_bufwrite_open(path);
        assert_ne!(bw, 0);
        assert_eq!(olive_bufwrite_write(bw, make_str("line1\n")), 1);
        assert_eq!(olive_bufwrite_write(bw, make_str("line2\n")), 1);
        assert_eq!(olive_bufwrite_flush(bw), 1);
        olive_bufwrite_close(bw);
        let content = from_ptr(olive_file_read(path));
        assert_eq!(content, "line1\nline2\n");
        olive_file_delete(path);
    }

    #[test]
    fn bufread_null_returns_zero() {
        assert_eq!(olive_bufread_open(0), 0);
    }

    #[test]
    fn bufwrite_null_returns_zero() {
        assert_eq!(olive_bufwrite_open(0), 0);
    }
}
