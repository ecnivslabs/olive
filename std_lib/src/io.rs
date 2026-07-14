use crate::{olive_str_from_ptr, olive_str_internal};
use rustc_hash::FxHashMap as HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, OnceLock};

fn olive_write_str_to_stdout(s: &str) {
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    let _ = handle.write_all(s.as_bytes());
    let _ = handle.flush();
}

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
    if std::fs::write(&path_str, data_str.as_bytes()).is_ok() {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_file_append(path: i64, data: i64) -> i64 {
    if path == 0 || data == 0 {
        return 0;
    }
    let path_str = olive_str_from_ptr(path);
    let data_str = olive_str_from_ptr(data);
    match std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path_str)
    {
        Ok(mut f) => {
            if f.write_all(data_str.as_bytes()).is_ok() {
                1
            } else {
                0
            }
        }
        Err(_) => return 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_file_exists(path: i64) -> i64 {
    if path == 0 {
        return 0;
    }
    if std::path::Path::new(&olive_str_from_ptr(path)).exists() {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_file_delete(path: i64) -> i64 {
    if path == 0 {
        return 0;
    }
    let path_str = olive_str_from_ptr(path);
    let p = std::path::Path::new(&path_str);
    if p.is_dir() {
        if std::fs::remove_dir_all(p).is_ok() {
            1
        } else {
            0
        }
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
    if std::fs::create_dir_all(olive_str_from_ptr(path)).is_ok() {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_dir_list(path: i64) -> i64 {
    let path_str = if path == 0 {
        ".".to_string()
    } else {
        olive_str_from_ptr(path)
    };
    let entries = match std::fs::read_dir(&path_str) {
        Ok(e) => e,
        Err(_) => {
            return crate::list::list_from_vec(Vec::new());
        }
    };
    let mut ptrs: Vec<i64> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        ptrs.push(olive_str_internal(&name));
    }
    crate::list::list_from_vec(ptrs)
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
    let link_meta = std::fs::symlink_metadata(&path_str).ok();
    let secs = |t: std::io::Result<std::time::SystemTime>| {
        t.ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    };
    let mode = file_mode(&meta);
    let mut fields = HashMap::default();
    fields.insert(
        crate::OliveStringKey(olive_str_internal("size")),
        crate::boxed::olive_box_int(meta.len() as i64),
    );
    fields.insert(
        crate::OliveStringKey(olive_str_internal("is_dir")),
        crate::boxed::olive_box_bool(meta.is_dir() as i64),
    );
    fields.insert(
        crate::OliveStringKey(olive_str_internal("is_file")),
        crate::boxed::olive_box_bool(meta.is_file() as i64),
    );
    fields.insert(
        crate::OliveStringKey(olive_str_internal("is_symlink")),
        crate::boxed::olive_box_bool(link_meta.map(|m| m.is_symlink()).unwrap_or(false) as i64),
    );
    fields.insert(
        crate::OliveStringKey(olive_str_internal("modified")),
        crate::boxed::olive_box_int(secs(meta.modified())),
    );
    fields.insert(
        crate::OliveStringKey(olive_str_internal("created")),
        crate::boxed::olive_box_int(secs(meta.created())),
    );
    fields.insert(
        crate::OliveStringKey(olive_str_internal("accessed")),
        crate::boxed::olive_box_int(secs(meta.accessed())),
    );
    fields.insert(
        crate::OliveStringKey(olive_str_internal("mode")),
        crate::boxed::olive_box_int(mode),
    );
    crate::obj::new_obj_from_map(fields)
}

#[cfg(unix)]
fn file_mode(meta: &std::fs::Metadata) -> i64 {
    use std::os::unix::fs::PermissionsExt;
    (meta.permissions().mode() & 0o7777) as i64
}

#[cfg(not(unix))]
fn file_mode(meta: &std::fs::Metadata) -> i64 {
    if meta.permissions().readonly() {
        0o444
    } else {
        0o644
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_mode(path: i64, mode: i64) -> i64 {
    if path == 0 {
        return 0;
    }
    let path_str = olive_str_from_ptr(path);
    set_mode_impl(&path_str, mode)
}

#[cfg(unix)]
fn set_mode_impl(path: &str, mode: i64) -> i64 {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(mode as u32 & 0o7777);
    if std::fs::set_permissions(path, perms).is_ok() {
        1
    } else {
        0
    }
}

#[cfg(not(unix))]
fn set_mode_impl(path: &str, mode: i64) -> i64 {
    let readonly = mode & 0o200 == 0;
    match std::fs::metadata(path) {
        Ok(meta) => {
            let mut perms = meta.permissions();
            perms.set_readonly(readonly);
            if std::fs::set_permissions(path, perms).is_ok() {
                1
            } else {
                0
            }
        }
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_mtime(path: i64, epoch_secs: i64) -> i64 {
    if path == 0 {
        return 0;
    }
    let path_str = olive_str_from_ptr(path);
    set_mtime_impl(&path_str, epoch_secs)
}

fn set_mtime_impl(path: &str, epoch_secs: i64) -> i64 {
    let mtime = std::time::SystemTime::UNIX_EPOCH
        + std::time::Duration::from_secs(epoch_secs.max(0) as u64);
    if std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|f| f.set_modified(mtime))
        .is_ok()
    {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_path_canonicalize(path: i64) -> i64 {
    if path == 0 {
        return 0;
    }
    match std::fs::canonicalize(olive_str_from_ptr(path)) {
        Ok(p) => olive_str_internal(&p.to_string_lossy()),
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_path_join_all(parts_ptr: i64) -> i64 {
    if parts_ptr == 0 {
        return olive_str_internal("");
    }
    let v = unsafe { &*(parts_ptr as *const crate::StableVec) };
    let items = unsafe { std::slice::from_raw_parts(v.ptr, v.len) };
    let mut path = std::path::PathBuf::new();
    for &p in items {
        path.push(olive_str_from_ptr(p));
    }
    olive_str_internal(&path.to_string_lossy())
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_path_relative(base: i64, path: i64) -> i64 {
    if base == 0 || path == 0 {
        return 0;
    }
    let base_str = olive_str_from_ptr(base);
    let path_str = olive_str_from_ptr(path);
    let cwd = std::env::current_dir().unwrap_or_default();
    let base_abs = cwd.join(&base_str);
    let path_abs = cwd.join(&path_str);

    let base_comps: Vec<_> = base_abs.components().collect();
    let path_comps: Vec<_> = path_abs.components().collect();

    let common = base_comps
        .iter()
        .zip(path_comps.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let mut result = std::path::PathBuf::new();
    for _ in common..base_comps.len() {
        result.push("..");
    }
    for comp in &path_comps[common..] {
        result.push(comp.as_os_str());
    }

    if result.as_os_str().is_empty() {
        olive_str_internal(".")
    } else {
        olive_str_internal(&result.to_string_lossy())
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_path_is_symlink(path: i64) -> i64 {
    if path == 0 {
        return 0;
    }
    match std::fs::symlink_metadata(olive_str_from_ptr(path)) {
        Ok(m) => {
            if m.is_symlink() {
                1
            } else {
                0
            }
        }
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_path_read_link(path: i64) -> i64 {
    if path == 0 {
        return 0;
    }
    match std::fs::read_link(olive_str_from_ptr(path)) {
        Ok(p) => olive_str_internal(&p.to_string_lossy()),
        Err(_) => 0,
    }
}

#[cfg(unix)]
fn make_symlink(target: &str, link: &str) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn make_symlink(target: &str, link: &str) -> std::io::Result<()> {
    let target_path = std::path::Path::new(target);
    if target_path.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_path_symlink(target: i64, link: i64) -> i64 {
    if target == 0 || link == 0 {
        return 0;
    }
    let t = olive_str_from_ptr(target);
    let l = olive_str_from_ptr(link);
    if make_symlink(&t, &l).is_ok() { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_path_hard_link(src: i64, dst: i64) -> i64 {
    if src == 0 || dst == 0 {
        return 0;
    }
    let s = olive_str_from_ptr(src);
    let d = olive_str_from_ptr(dst);
    if std::fs::hard_link(&s, &d).is_ok() {
        1
    } else {
        0
    }
}

/// Bounds a recursive walk so a symlink cycle or a pathological tree cannot
/// run unbounded: depth is capped, entries are capped, and each real
/// directory (by canonical path) is visited at most once.
const WALK_MAX_DEPTH: usize = 64;
const WALK_MAX_ENTRIES: usize = 200_000;

fn walk_dir(
    dir: &std::path::Path,
    depth: usize,
    visited: &mut std::collections::HashSet<std::path::PathBuf>,
    out: &mut Vec<String>,
) {
    if depth > WALK_MAX_DEPTH || out.len() >= WALK_MAX_ENTRIES {
        return;
    }
    let canon = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    if !visited.insert(canon) {
        return;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut children: Vec<_> = entries.flatten().collect();
    children.sort_by_key(|e| e.file_name());

    for entry in children {
        if out.len() >= WALK_MAX_ENTRIES {
            return;
        }
        let path = entry.path();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        out.push(path.to_string_lossy().into_owned());
        if is_dir {
            walk_dir(&path, depth + 1, visited, out);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_walk(root: i64) -> i64 {
    let root_str = if root == 0 {
        ".".to_string()
    } else {
        olive_str_from_ptr(root)
    };
    let mut out = Vec::new();
    let mut visited = std::collections::HashSet::new();
    walk_dir(std::path::Path::new(&root_str), 0, &mut visited, &mut out);
    let ptrs: Vec<i64> = out.iter().map(|s| olive_str_internal(s)).collect();
    crate::list::list_from_vec(ptrs)
}

fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if file_type.is_symlink() {
            let target = std::fs::read_link(&src_path)?;
            let _ = std::fs::remove_file(&dst_path);
            make_symlink(&target.to_string_lossy(), &dst_path.to_string_lossy())?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_copy_dir(src: i64, dst: i64) -> i64 {
    if src == 0 || dst == 0 {
        return 0;
    }
    let src_str = olive_str_from_ptr(src);
    let dst_str = olive_str_from_ptr(dst);
    if copy_dir_recursive(
        std::path::Path::new(&src_str),
        std::path::Path::new(&dst_str),
    )
    .is_ok()
    {
        1
    } else {
        0
    }
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
    if std::fs::rename(&src_str, &dst_str).is_ok() {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_path_join(a: i64, b: i64) -> i64 {
    let a_str = if a == 0 {
        String::new()
    } else {
        olive_str_from_ptr(a)
    };
    let b_str = if b == 0 {
        String::new()
    } else {
        olive_str_from_ptr(b)
    };
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
            if s.is_empty() {
                olive_str_internal(".")
            } else {
                olive_str_internal(&s)
            }
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
    if std::path::Path::new(&p).is_absolute() {
        1
    } else {
        0
    }
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

#[unsafe(no_mangle)]
pub extern "C" fn olive_temp_dir() -> i64 {
    olive_str_internal(&std::env::temp_dir().to_string_lossy())
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_temp_file() -> i64 {
    let tmp = std::env::temp_dir();
    let name = format!("olive_{}", uuid::Uuid::new_v4().simple());
    let path = tmp.join(name);
    match std::fs::File::create(&path) {
        Ok(_) => olive_str_internal(&path.to_string_lossy()),
        Err(_) => 0,
    }
}

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
    match read_line_trimmed(&mut std::io::stdin().lock()) {
        LineRead::Line(line) => olive_str_internal(&line),
        _ => olive_str_internal(""),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_input(prompt_ptr: i64) -> i64 {
    if prompt_ptr != 0 {
        let prompt = olive_str_from_ptr(prompt_ptr);
        olive_write_str_to_stdout(&prompt);
    }
    match read_line_trimmed(&mut std::io::stdin().lock()) {
        LineRead::Line(line) => olive_str_internal(&line),
        _ => olive_str_internal(""),
    }
}

/// Every file-handle native takes an opaque integer the language treats as a
/// plain value, so it can be copied, stored in fields, and closed twice from
/// two struct copies (`File.__drop__` runs on each copy that still holds a
/// nonzero handle). A raw pointer makes any of those a double-free or
/// use-after-free. Handles are therefore indices into this table with a
/// generation baked into the high bits: a stale copy of a closed handle fails
/// lookup instead of dereferencing freed memory.
enum IoHandle {
    File(std::fs::File),
    BufRead(std::io::BufReader<std::fs::File>),
    BufWrite(std::io::BufWriter<std::fs::File>),
}

const HANDLE_GEN_SHIFT: u32 = 32;
const HANDLE_INDEX_MASK: i64 = 0x7FFF_FFFF;

fn handles() -> &'static Mutex<HashMap<i64, IoHandle>> {
    static TABLE: OnceLock<Mutex<HashMap<i64, IoHandle>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::default()))
}

fn next_handle_id() -> i64 {
    static NEXT: AtomicI64 = AtomicI64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed) & HANDLE_INDEX_MASK
}

fn handle_index(handle: i64) -> i64 {
    handle & HANDLE_INDEX_MASK
}

fn register_handle(entry: IoHandle) -> i64 {
    let id = next_handle_id();
    handles().lock().unwrap().insert(id, entry);
    id | ((id as u64) << HANDLE_GEN_SHIFT) as i64
}

fn take_handle(handle: i64) -> Option<IoHandle> {
    handles().lock().unwrap().remove(&handle_index(handle))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_file_open(path: i64, mode: i64) -> i64 {
    if path == 0 {
        return 0;
    }
    let path_str = olive_str_from_ptr(path);
    let mode_str = if mode == 0 {
        "r".to_string()
    } else {
        olive_str_from_ptr(mode)
    };
    let file = match mode_str.as_str() {
        "w" => std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path_str),
        "a" => std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path_str),
        "r+" => std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path_str),
        "w+" => std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path_str),
        _ => std::fs::OpenOptions::new().read(true).open(&path_str),
    };
    match file {
        Ok(f) => register_handle(IoHandle::File(f)),
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_file_close(handle: i64) {
    if handle != 0 {
        take_handle(handle);
    }
}

fn with_file<R>(handle: i64, f: impl FnOnce(&mut std::fs::File) -> R) -> Option<R> {
    let mut table = handles().lock().unwrap();
    match table.get_mut(&handle_index(handle)) {
        Some(IoHandle::File(file)) => Some(f(file)),
        _ => None,
    }
}

/// Reads up to `n` bytes. Distinguishes the three outcomes the ""-only
/// contract cannot carry: a dead/foreign handle returns null, an I/O error
/// returns 1, and a successful read (EOF included) returns the string.
#[unsafe(no_mangle)]
pub extern "C" fn olive_file_read_n(handle: i64, n: i64) -> i64 {
    if handle == 0 {
        return 0;
    }
    let want = if n <= 0 || n > MAX_READ_BYTES as i64 {
        MAX_READ_BYTES as i64
    } else {
        n
    };
    let mut buf = vec![0u8; want as usize];
    match with_file(handle, |file| file.read(&mut buf)) {
        Some(Ok(0)) => olive_str_internal(""),
        Some(Ok(read)) => {
            buf.truncate(read);
            let s = String::from_utf8_lossy(&buf).into_owned();
            olive_str_internal(&s)
        }
        Some(Err(_)) => 1,
        None => 0,
    }
}

const MAX_READ_BYTES: usize = 1 << 30;

#[unsafe(no_mangle)]
pub extern "C" fn olive_file_write_str(handle: i64, data: i64) -> i64 {
    if handle == 0 || data == 0 {
        return 0;
    }
    let data_str = olive_str_from_ptr(data);
    match with_file(handle, |file| file.write_all(data_str.as_bytes())) {
        Some(Ok(())) => 1,
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_file_seek(handle: i64, offset: i64, whence: i64) -> i64 {
    if handle == 0 {
        return -1;
    }
    let pos = match whence {
        1 => SeekFrom::Current(offset),
        2 => SeekFrom::End(offset),
        _ => {
            if offset < 0 {
                return -1;
            }
            SeekFrom::Start(offset as u64)
        }
    };
    match with_file(handle, |file| file.seek(pos)) {
        Some(Ok(new_pos)) => new_pos as i64,
        _ => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_file_tell(handle: i64) -> i64 {
    if handle == 0 {
        return -1;
    }
    match with_file(handle, |file| file.stream_position()) {
        Some(Ok(pos)) => pos as i64,
        _ => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_file_read_lines(path: i64) -> i64 {
    let empty = || crate::list::list_from_vec(Vec::new());
    if path == 0 {
        return empty();
    }
    let content = match std::fs::read_to_string(olive_str_from_ptr(path)) {
        Ok(c) => c,
        Err(_) => return empty(),
    };
    let ptrs: Vec<i64> = content.lines().map(olive_str_internal).collect();
    crate::list::list_from_vec(ptrs)
}

enum LineRead {
    Line(String),
    Eof,
    Err,
}

/// Shared by stdin and buffered-file line reads: strips a trailing "\r\n" or
/// "\n" so CRLF and LF sources yield identical strings.
fn read_line_trimmed<R: std::io::BufRead>(reader: &mut R) -> LineRead {
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) => LineRead::Eof,
        Ok(_) => {
            if line.ends_with('\n') {
                line.pop();
                if line.ends_with('\r') {
                    line.pop();
                }
            }
            LineRead::Line(line)
        }
        Err(_) => LineRead::Err,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_bufread_open(path: i64) -> i64 {
    if path == 0 {
        return 0;
    }
    let path_str = olive_str_from_ptr(path);
    match std::fs::File::open(&path_str) {
        Ok(f) => register_handle(IoHandle::BufRead(std::io::BufReader::new(f))),
        Err(_) => 0,
    }
}

/// Reads one line. Null on a dead/foreign handle or I/O error, empty string
/// at EOF, so the canonical `while (l = read_line()) != ""` loop terminates
/// without silently treating errors as end-of-file.
#[unsafe(no_mangle)]
pub extern "C" fn olive_bufread_line(br: i64) -> i64 {
    if br == 0 {
        return 0;
    }
    let mut table = handles().lock().unwrap();
    let reader = match table.get_mut(&handle_index(br)) {
        Some(IoHandle::BufRead(r)) => r,
        _ => return 0,
    };
    match read_line_trimmed(reader) {
        LineRead::Line(line) => olive_str_internal(&line),
        LineRead::Eof => olive_str_internal(""),
        LineRead::Err => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_bufread_close(br: i64) {
    if br != 0 {
        take_handle(br);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_bufwrite_open(path: i64) -> i64 {
    if path == 0 {
        return 0;
    }
    let path_str = olive_str_from_ptr(path);
    match std::fs::File::create(&path_str) {
        Ok(f) => register_handle(IoHandle::BufWrite(std::io::BufWriter::new(f))),
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_bufwrite_write(bw: i64, data: i64) -> i64 {
    if bw == 0 || data == 0 {
        return 0;
    }
    use std::io::Write;
    let text = olive_str_from_ptr(data);
    let mut table = handles().lock().unwrap();
    match table.get_mut(&handle_index(bw)) {
        Some(IoHandle::BufWrite(w)) => {
            if w.write_all(text.as_bytes()).is_ok() {
                1
            } else {
                0
            }
        }
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_bufwrite_flush(bw: i64) -> i64 {
    if bw == 0 {
        return 0;
    }
    use std::io::Write;
    let mut table = handles().lock().unwrap();
    match table.get_mut(&handle_index(bw)) {
        Some(IoHandle::BufWrite(w)) => {
            if w.flush().is_ok() {
                1
            } else {
                0
            }
        }
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_bufwrite_close(bw: i64) {
    if bw != 0 {
        take_handle(bw);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::olive_str_internal;
    use crate::{OliveObj, StableVec};

    fn make_str(s: &str) -> i64 {
        olive_str_internal(s)
    }

    fn from_ptr(ptr: i64) -> String {
        crate::olive_str_from_ptr(ptr)
    }

    fn temp_path(filename: &str) -> i64 {
        let p = std::env::temp_dir().join(filename);
        olive_str_internal(&p.to_string_lossy())
    }

    #[test]
    fn file_write_read_delete() {
        let path = temp_path("olive_io_test_rw.txt");
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
        let path = temp_path("olive_io_test_append.txt");
        let _ = olive_file_delete(path);
        olive_file_append(path, make_str("line1\n"));
        olive_file_append(path, make_str("line2\n"));
        let content = from_ptr(olive_file_read(path));
        assert_eq!(content, "line1\nline2\n");
        olive_file_delete(path);
    }

    #[test]
    fn dir_create_list_delete() {
        let dir = temp_path("olive_io_test_dir");
        assert_eq!(olive_dir_create(dir), 1);
        let sub_path = std::env::temp_dir().join("olive_io_test_dir").join("sub");
        let sub = olive_str_internal(&sub_path.to_string_lossy());
        assert_eq!(olive_dir_create(sub), 1);
        let list_ptr = olive_dir_list(dir);
        assert_ne!(list_ptr, 0);
        let list = unsafe { &*(list_ptr as *const StableVec) };
        assert!(list.len >= 1);
        assert_eq!(olive_file_delete(dir), 1);
    }

    #[test]
    fn file_stat_returns_obj() {
        let path = temp_path("olive_io_stat_test.txt");
        olive_file_write(path, make_str("data"));
        let obj_ptr = olive_file_stat(path);
        assert_ne!(obj_ptr, 0);
        let obj = unsafe { &*(obj_ptr as *const OliveObj) };
        let field = |name: &str| {
            crate::boxed::olive_unbox_int(
                *obj.fields
                    .get(&crate::OliveStringKey(olive_str_internal(name)))
                    .unwrap(),
            )
        };
        assert_eq!(field("is_file"), 1);
        assert_eq!(field("is_dir"), 0);
        assert_eq!(field("size"), 4);
        olive_file_delete(path);
    }

    #[test]
    fn read_nonexistent_returns_zero() {
        let path = temp_path("olive_definitely_does_not_exist_xyz.txt");
        assert_eq!(olive_file_read(path), 0);
    }

    #[test]
    fn path_join_basic() {
        let a = temp_path("olive_tmp");
        let b = make_str("file.txt");
        let result = from_ptr(olive_path_join(a, b));
        let expected = std::env::temp_dir().join("olive_tmp").join("file.txt");
        assert_eq!(result, expected.to_string_lossy());
    }

    #[test]
    fn path_dirname_and_basename() {
        let p_path = std::env::temp_dir().join("foo").join("bar.txt");
        let p = olive_str_internal(&p_path.to_string_lossy());
        let expected_dir = std::env::temp_dir().join("foo");
        assert_eq!(
            from_ptr(olive_path_dirname(p)),
            expected_dir.to_string_lossy()
        );
        assert_eq!(from_ptr(olive_path_basename(p)), "bar.txt");
    }

    #[test]
    fn path_ext_and_stem() {
        let p_path = std::env::temp_dir().join("file.tar.gz");
        let p = olive_str_internal(&p_path.to_string_lossy());
        assert_eq!(from_ptr(olive_path_ext(p)), "gz");
        assert_eq!(from_ptr(olive_path_stem(p)), "file.tar");
    }

    #[test]
    fn path_is_absolute() {
        let abs_p = temp_path("foo");
        assert_eq!(olive_path_is_absolute(abs_p), 1);
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
        let path = temp_path("olive_seek_test.txt");
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
    fn read_n_eof_vs_error_vs_dead_handle() {
        let path = temp_path("olive_read_n_eof.txt");
        olive_file_write(path, make_str("0123456789"));
        let handle = olive_file_open(path, make_str("r"));
        for expected in ["0123", "4567", "89"] {
            let chunk = olive_file_read_n(handle, 4);
            assert_ne!(chunk, 0);
            assert_ne!(chunk, 1);
            assert_eq!(from_ptr(chunk), expected);
        }
        assert_eq!(from_ptr(olive_file_read_n(handle, 4)), "");
        olive_file_close(handle);
        assert_eq!(olive_file_read_n(handle, 4), 0);
        olive_file_delete(path);
    }

    #[test]
    fn double_close_is_absorbed() {
        let path = temp_path("olive_double_close.txt");
        olive_file_write(path, make_str("x"));
        let handle = olive_file_open(path, make_str("r"));
        assert_ne!(handle, 0);
        olive_file_close(handle);
        olive_file_close(handle);
        let again = olive_file_open(path, make_str("r"));
        assert_ne!(again, 0);
        assert_ne!(again & HANDLE_INDEX_MASK, handle & HANDLE_INDEX_MASK);
        olive_file_close(again);
        olive_file_delete(path);
    }

    #[test]
    fn operations_on_closed_handle_fail_cleanly() {
        let path = temp_path("olive_use_after_close.txt");
        olive_file_write(path, make_str("x"));
        let handle = olive_file_open(path, make_str("w+"));
        assert_ne!(handle, 0);
        olive_file_close(handle);
        assert_eq!(olive_file_read_n(handle, 4), 0);
        assert_eq!(olive_file_tell(handle), -1);
        assert_eq!(olive_file_seek(handle, 0, 0), -1);
        assert_eq!(olive_file_write_str(handle, make_str("y")), 0);
        olive_file_delete(path);
    }

    #[test]
    fn read_n_garbage_handle_returns_null() {
        assert_eq!(olive_file_read_n(12345, 4), 0);
        assert_eq!(olive_file_read_n(-1, 4), 0);
        assert_eq!(olive_file_tell(12345), -1);
        assert_eq!(olive_file_seek(12345, 0, 0), -1);
        assert_eq!(olive_bufread_line(12345), 0);
        assert_eq!(olive_bufwrite_flush(12345), 0);
    }

    #[test]
    fn open_failure_leaks_no_handle() {
        let before = handles().lock().unwrap().len();
        for _ in 0..100 {
            assert_eq!(
                olive_file_open(make_str("/nonexistent_dir_xyz/f.txt"), make_str("r")),
                0
            );
            assert_eq!(
                olive_bufread_open(make_str("/nonexistent_dir_xyz/f.txt")),
                0
            );
            assert_eq!(
                olive_bufwrite_open(make_str("/nonexistent_dir_xyz/f.txt")),
                0
            );
        }
        assert_eq!(handles().lock().unwrap().len(), before);
    }

    #[test]
    fn bufread_eof_and_partial_line() {
        let path = temp_path("olive_bufread_eof.txt");
        olive_file_write(path, make_str("alpha\nbeta\ngamma"));
        let br = olive_bufread_open(path);
        assert_ne!(br, 0);
        for expected in ["alpha", "beta", "gamma"] {
            let line = olive_bufread_line(br);
            assert_ne!(line, 0);
            assert_eq!(from_ptr(line), expected);
        }
        assert_eq!(from_ptr(olive_bufread_line(br)), "");
        olive_bufread_close(br);
        assert_eq!(olive_bufread_line(br), 0);
        olive_file_delete(path);
    }

    #[test]
    fn bufwrite_double_close_absorbed() {
        let path = temp_path("olive_bw_double_close.txt");
        let bw = olive_bufwrite_open(path);
        assert_ne!(bw, 0);
        assert_eq!(olive_bufwrite_write(bw, make_str("kept")), 1);
        olive_bufwrite_close(bw);
        olive_bufwrite_close(bw);
        assert_eq!(olive_bufwrite_flush(bw), 0);
        assert_eq!(from_ptr(olive_file_read(path)), "kept");
        olive_file_delete(path);
    }

    #[test]
    fn bufread_crlf_line_endings() {
        let path = temp_path("olive_bufread_crlf.txt");
        olive_file_write(path, make_str("a\r\nb\r\n"));
        let br = olive_bufread_open(path);
        assert_eq!(from_ptr(olive_bufread_line(br)), "a");
        assert_eq!(from_ptr(olive_bufread_line(br)), "b");
        assert_eq!(from_ptr(olive_bufread_line(br)), "");
        olive_bufread_close(br);
        olive_file_delete(path);
    }

    #[test]
    fn file_read_lines_basic() {
        let path = temp_path("olive_lines_test.txt");
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
        let src = temp_path("olive_copy_src.txt");
        let dst = temp_path("olive_copy_dst.txt");
        let renamed = temp_path("olive_renamed.txt");
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
        let path = temp_path("olive_bufread_test.txt");
        olive_file_write(path, make_str("alpha\nbeta\ngamma\n"));
        let br = olive_bufread_open(path);
        assert_ne!(br, 0);
        assert_eq!(from_ptr(olive_bufread_line(br)), "alpha");
        assert_eq!(from_ptr(olive_bufread_line(br)), "beta");
        assert_eq!(from_ptr(olive_bufread_line(br)), "gamma");
        assert_ne!(olive_bufread_line(br), 0);
        assert_eq!(from_ptr(olive_bufread_line(br)), "");
        assert_eq!(from_ptr(olive_bufread_line(br)), "");
        olive_bufread_close(br);
        assert_eq!(olive_bufread_line(br), 0);
        olive_file_delete(path);
    }

    #[test]
    fn bufwrite_flushes_on_close() {
        let path = temp_path("olive_bw_close_flush.txt");
        let bw = olive_bufwrite_open(path);
        assert_ne!(bw, 0);
        assert_eq!(olive_bufwrite_write(bw, make_str("unflushed")), 1);
        drop(take_handle(bw));
        assert_eq!(from_ptr(olive_file_read(path)), "unflushed");
        olive_file_delete(path);
    }

    #[test]
    fn bufwrite_and_flush() {
        let path = temp_path("olive_bufwrite_test.txt");
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
