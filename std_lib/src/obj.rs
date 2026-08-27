use crate::slab::GenSlab;
use crate::*;
use std::cell::UnsafeCell;

thread_local! {
    static OBJ_SLAB: UnsafeCell<GenSlab> =
        const { UnsafeCell::new(GenSlab::with_cleanup(std::mem::size_of::<OliveObj>(), release_obj_storage)) };
}

pub(crate) unsafe fn release_obj_storage(body: *mut u8) {
    let obj = unsafe { &mut *(body as *mut OliveObj) };
    drop(std::mem::take(&mut obj.fields));
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_new() -> i64 {
    let slab_alloc = |sl: &mut GenSlab| {
        let (body, fresh) = sl.alloc();
        let o = body as *mut OliveObj;
        unsafe {
            if fresh || cfg!(debug_assertions) {
                std::ptr::write(
                    o,
                    OliveObj {
                        kind: KIND_OBJ,
                        fields: HashMap::default(),
                    },
                );
            } else {
                (*o).kind = KIND_OBJ;
                (*o).fields.clear();
            }
        }
        body as i64
    };
    unsafe {
        let active = crate::slab::ACTIVE_SLABS.get();
        if !active.is_null() {
            slab_alloc(&mut (*active).obj)
        } else {
            OBJ_SLAB.with(|sl| slab_alloc(&mut *sl.get()))
        }
    }
}

/// Builds a dict object around an already-populated field map.
pub(crate) fn new_obj_from_map(mut fields: HashMap<OliveStringKey, i64>) -> i64 {
    let ptr = olive_obj_new();
    unsafe {
        std::mem::swap(&mut (*(ptr as *mut OliveObj)).fields, &mut fields);
    }
    ptr
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_set(obj_ptr: i64, attr: i64, val: i64) -> i64 {
    obj_store(obj_ptr, attr, val, None)
}

/// `d[k] = v` for value-owning dicts: stores like `olive_obj_set`, releasing
/// the displaced value through `val_desc` (the value type's own descriptor)
/// so overwriting a heap value does not leak it. `key_desc` selects the key
/// classification exactly like `olive_obj_set_typed`; `0` keeps the untyped
/// heuristic. Mirrors `__olive_set_index_any`'s replace discipline, including
/// its self-assignment guard.
#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_set_replacing_typed(
    obj_ptr: i64,
    attr: i64,
    val: i64,
    key_desc: i64,
    val_desc: i64,
) -> i64 {
    crate::hash_typed::with_key_descriptor(key_desc, || {
        obj_store(obj_ptr, attr, val, Some(val_desc as *const u8))
    })
}

/// Shared insert core. Returns `obj_ptr` in every case, matching
/// `olive_obj_set`. When `val_desc` is set, a displaced value is released
/// through it; otherwise the old value is left alone for the caller (plain
/// `olive_obj_set`, aggregate init, and `update_typed`'s deferred-free
/// protocol all rely on taking no action here).
fn obj_store(obj_ptr: i64, attr: i64, val: i64, val_desc: Option<*const u8>) -> i64 {
    if obj_ptr == 0 {
        panic!("Null pointer dereference: attempted to set attribute on a null object");
    }
    if !crate::slab::ptr_is_slab_body(obj_ptr) {
        return obj_ptr;
    }
    let kind = unsafe { *(obj_ptr as *const i64) };
    if kind == KIND_PYOBJECT {
        return python::olive_py_setattr(obj_ptr as *mut std::ffi::c_void, attr, val) as i64;
    }
    let m = unsafe { &mut *(obj_ptr as *mut OliveObj) };
    // A heap string key is a caller value that will be freed at its scope
    // exit; the dict keeps a private copy so its stored key never dangles.
    // Literals store directly: they live forever. The copy gate validates
    // without dereferencing (`store_key_needs_owned_copy`): a raw odd int
    // above the string-tag floor looks like a string pointer to the
    // magnitude heuristic, and copying its bits as string bytes faults.
    // Untagged attribute names are read-only interned symbols, kept as-is.
    let old = if crate::store_key_needs_owned_copy(attr)
        && !m.fields.contains_key(&OliveStringKey(attr))
    {
        // Length-preserving copy: the key may hold an embedded NUL
        // (Python-derived strings keep them), which a `strlen` re-copy
        // would truncate, collapsing distinct keys.
        let owned = crate::string_slab::str_alloc(crate::string::olive_str_to_bytes(attr));
        m.fields.insert(OliveStringKey(owned), val);
        None
    } else {
        m.fields.insert(OliveStringKey(attr), val)
    };
    if let (Some(old), Some(desc)) = (old, val_desc)
        && old != val
    {
        let mut pos = 0usize;
        crate::free_typed::free_val(old, desc, &mut pos);
    }
    obj_ptr
}
#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_get(obj_ptr: i64, attr: i64) -> i64 {
    if obj_ptr == 0 || !crate::slab::ptr_is_slab_body(obj_ptr) {
        return 0;
    }
    let kind = unsafe { *(obj_ptr as *const i64) };
    if kind == KIND_PYOBJECT {
        return python::olive_py_dict_get_default(obj_ptr, attr, 0);
    }
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    *m.fields.get(&OliveStringKey(attr)).unwrap_or(&0)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_get_checked(obj_ptr: i64, attr: i64, loc: i64) -> i64 {
    if obj_ptr == 0 || !crate::slab::ptr_is_slab_body(obj_ptr) {
        crate::panic::olive_nil_index_fail(loc);
        return 0;
    }
    let kind = unsafe { *(obj_ptr as *const i64) };
    if kind == KIND_PYOBJECT {
        return python::olive_py_dict_get_default(obj_ptr, attr, 0);
    }
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    if let Some(&val) = m.fields.get(&OliveStringKey(attr)) {
        val
    } else {
        crate::panic::olive_key_fail(attr, loc);
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_get_default(obj_ptr: i64, attr: i64, default: i64) -> i64 {
    get_default_impl(obj_ptr, attr, default, false)
}

/// `.get` whose result feeds a tag-encoded slot (`Any`, `int | str`, ...):
/// a hit on a raw stored word is boxed so it reads back self-describing, the
/// same way values entering an `Any`-valued dict are boxed at `set`. The
/// caller passes the `default` already boxed.
#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_get_default_boxed(obj_ptr: i64, attr: i64, default: i64) -> i64 {
    get_default_impl(obj_ptr, attr, default, true)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_get_boxed(obj_ptr: i64, attr: i64) -> i64 {
    box_stored(olive_obj_get(obj_ptr, attr))
}

fn get_default_impl(obj_ptr: i64, attr: i64, default: i64, boxed: bool) -> i64 {
    if obj_ptr == 0 || !crate::slab::ptr_is_slab_body(obj_ptr) {
        return default;
    }
    let kind = unsafe { *(obj_ptr as *const i64) };
    if kind == KIND_PYOBJECT {
        // `.get(key, default)` is dict-lookup semantics: index by key (not
        // attribute) and fall back to `default`, matching Python's `dict.get`.
        return python::olive_py_dict_get_default(obj_ptr, attr, default);
    }
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    match m.fields.get(&OliveStringKey(attr)) {
        Some(&v) if boxed => box_stored(v),
        Some(&v) => v,
        None => default,
    }
}

fn box_stored(v: i64) -> i64 {
    // Already self-describing: heap objects (slab pointers), strings (bit-0
    // pointers), inline immediates (TAG_INT/TAG_BOOL/TAG_NULL), or zero.
    if crate::is_active_object(v) || v & 1 == 1 || v & boxed::TAG_MASK != 0 || v < 0x10000 {
        return v;
    }
    // A raw scalar wider than the tag space from a concrete-typed dict:
    // box it like `set` into an Any-valued dict would. Bool and None raw
    // words are 0/1, caught by the magnitude guard, but they only share a
    // dict with ints in the already-tagged case anyway.
    boxed::olive_box_int(v)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_remove(obj_ptr: i64, attr: i64) -> i64 {
    olive_obj_remove_inner(obj_ptr, attr, None)
}

/// Shared body for `remove`: returns the displaced value. `key_desc` (the
/// key type's own descriptor) releases a heap-owning displaced key through
/// the static key type; without it only tagged strings classify and struct
/// keys strand.
pub(crate) fn olive_obj_remove_inner(obj_ptr: i64, attr: i64, key_desc: Option<i64>) -> i64 {
    // SAFETY: body moved verbatim from the extern below — same contract (live dict; map ops stay inside it).
    if obj_ptr == 0 || !crate::slab::ptr_is_slab_body(obj_ptr) {
        return 0;
    }
    let m = unsafe { &mut *(obj_ptr as *mut OliveObj) };
    match m.fields.remove_entry(&OliveStringKey(attr)) {
        Some((k, v)) => {
            free_displaced_key(k.0, key_desc);
            v
        }
        None => 0,
    }
}

/// Releases a displaced dict key: odd-tagged strings free directly, and
/// every other word classifies as a no-op (literals, immediates). With the
/// key type's own descriptor the word releases through the static key
/// type instead, so struct keys free exactly rather than stranding.
pub(crate) fn free_displaced_key(key: i64, key_desc: Option<i64>) {
    match key_desc {
        Some(desc) => {
            let mut pos = 0usize;
            crate::free_typed::free_val(key, desc as *const u8, &mut pos);
        }
        None => {
            if crate::is_tagged_str_key(key) {
                crate::olive_free_str(key);
            }
        }
    }
}

/// `d.pop(k)`: removes and returns the value, faulting if `k` is absent.
#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_pop_checked(obj_ptr: i64, attr: i64, loc: i64) -> i64 {
    olive_obj_pop_checked_inner(obj_ptr, attr, loc, None)
}

pub(crate) fn olive_obj_pop_checked_inner(
    obj_ptr: i64,
    attr: i64,
    loc: i64,
    key_desc: Option<i64>,
) -> i64 {
    // SAFETY: body moved verbatim from the extern below — same contract (live dict; map ops stay inside it).
    if obj_ptr == 0 || !crate::slab::ptr_is_slab_body(obj_ptr) {
        crate::panic::olive_nil_index_fail(loc);
    }
    let m = unsafe { &mut *(obj_ptr as *mut OliveObj) };
    match m.fields.remove_entry(&OliveStringKey(attr)) {
        Some((k, v)) => {
            free_displaced_key(k.0, key_desc);
            v
        }
        None => {
            crate::panic::olive_bounds_fail(0, m.fields.len() as i64, loc);
            0
        }
    }
}

/// `d.pop(k, default)`: non-faulting, returns `default` when `k` is absent.
#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_pop_default(obj_ptr: i64, attr: i64, default: i64) -> i64 {
    olive_obj_pop_default_inner(obj_ptr, attr, default, None)
}

pub(crate) fn olive_obj_pop_default_inner(
    obj_ptr: i64,
    attr: i64,
    default: i64,
    key_desc: Option<i64>,
) -> i64 {
    // SAFETY: body moved verbatim from the extern below — same contract (live dict; map ops stay inside it).
    if obj_ptr == 0 || !crate::slab::ptr_is_slab_body(obj_ptr) {
        return default;
    }
    let m = unsafe { &mut *(obj_ptr as *mut OliveObj) };
    match m.fields.remove_entry(&OliveStringKey(attr)) {
        Some((k, v)) => {
            free_displaced_key(k.0, key_desc);
            v
        }
        None => default,
    }
}

/// `d.setdefault(k, v)`: returns the existing value, or inserts and returns `v`.
#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_setdefault(
    obj_ptr: i64,
    attr: i64,
    default: i64,
    val_desc: i64,
) -> i64 {
    if obj_ptr == 0 {
        panic!("Null pointer dereference: attempted to use setdefault on a null object");
    }
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    if let Some(&v) = m.fields.get(&OliveStringKey(attr)) {
        // `default` was transferred here; the hit keeps the stored value, so
        // the orphaned default must be released (guard the pathological
        // same-pointer pass-through, mirroring the replacing stores). A raw
        // struct's header word is a field count, not a kind tag, so an
        // untyped release would misread it: free through the value
        // descriptor when the caller supplied one.
        if default != v {
            if val_desc != 0 {
                crate::free_typed::olive_free_typed(default, val_desc);
            } else {
                crate::free_any_word(default);
            }
        }
        return v;
    }
    olive_obj_set(obj_ptr, attr, default);
    default
}

/// `d.update(other)`: merges `other`'s entries into `d` (overwrite on key
/// conflict), returns `d`. `other` keeps its own entries; values are raw
/// words here, see `olive_obj_update_typed` for heap-owning values.
#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_update(obj_ptr: i64, other_ptr: i64) -> i64 {
    if obj_ptr == 0 || other_ptr == 0 || !crate::slab::ptr_is_slab_body(other_ptr) {
        return obj_ptr;
    }
    // Only dicts carry a field map; anything else reads the word as a map
    // header. The checker rejects these statically; this is the dynamic
    // backstop. (`None` keeps its historical silent no-op.)
    if other_ptr != 0 && unsafe { *(other_ptr as *const i64) } != KIND_OBJ {
        let kind_name = olive_str_from_ptr(olive_typeof_str(other_ptr));
        crate::panic::abort(
            &format!("`update` requires a dict argument, got `{kind_name}`"),
            None,
        );
    }
    // Snapshotted before inserting: `olive_obj_set` can rehash `obj_ptr`'s
    // map, and when the two arguments alias (or a key copy allocates while
    // both maps share state through re-entrant codegen paths) an iterator
    // left live across the inserts reads freed buckets.
    let entries: Vec<(i64, i64)> = {
        let om = unsafe { &*(other_ptr as *const OliveObj) };
        om.fields.iter().map(|(k, &v)| (k.0, v)).collect()
    };
    for (k, v) in entries {
        olive_obj_set(obj_ptr, k, v);
    }
    obj_ptr
}

/// `d.clear()`: empties the dict in place (freeing owned keys and values),
/// returns it.
#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_clear(obj_ptr: i64) -> i64 {
    if obj_ptr == 0 {
        return obj_ptr;
    }
    let m = unsafe { &mut *(obj_ptr as *mut OliveObj) };
    for &val in m.fields.values() {
        crate::free_any_word(val);
    }
    for k in m.fields.keys() {
        if crate::is_tagged_str_key(k.0) {
            crate::olive_free_str(k.0);
        }
    }
    m.fields.clear();
    obj_ptr
}

/// Descriptor-driven `d.clear()`: values release through the dict's static
/// value type instead of kind dispatch, which misreads raw struct payloads;
/// keys release through the static key type the same way (identical to the
/// tagged check for `str`/`int` keys, and exact for structural keys).
/// `dict_desc` is the full `Dict(K, V)` descriptor. Element hooks run
/// through the usual typed-free registry path when the last reference
/// goes away.
#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_clear_typed(obj_ptr: i64, dict_desc: i64) -> i64 {
    if obj_ptr == 0 {
        return obj_ptr;
    }
    // SAFETY: same contract as the untyped snapshot/clear above — the
    // compiler passes a live dict of the statically described key/value
    // types. Copies and releases go through the descriptor, never raw
    // kind dispatch.
    let m = unsafe { &mut *(obj_ptr as *mut OliveObj) };
    let desc = dict_desc as *const u8;
    let mut key_pos = 1usize;
    crate::format::skip(desc, &mut key_pos);
    let val_start = key_pos;
    let fields = std::mem::take(&mut m.fields);
    for (k, v) in fields {
        if v != 0 {
            let mut vp = val_start;
            crate::free_typed::free_val(v, desc, &mut vp);
        }
        if k.0 != 0 {
            let mut kp = 1usize;
            crate::free_typed::free_val(k.0, desc, &mut kp);
        }
    }
    obj_ptr
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_in_obj(key: i64, obj_ptr: i64) -> i64 {
    if obj_ptr == 0 {
        panic!("Null pointer dereference: attempted to check 'in' on a null object");
    }
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    if m.fields.contains_key(&OliveStringKey(key)) {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_len(obj_ptr: i64) -> i64 {
    if obj_ptr == 0 {
        panic!("Null pointer dereference: attempted to get length of a null object");
    }
    unsafe { (*(obj_ptr as *const OliveObj)).fields.len() as i64 }
}

/// A `__drop__` hook as a callable word, for per-value cleanup below.
type ElementDropHook = extern "C" fn(i64) -> i64;

/// Runs a struct value's `__drop__` for every live value of a dict whose
/// value type statically carries one, zeroing each entry as it goes so the
/// dict's own drop (which follows) only frees keys. Mirrors
/// `olive_list_drop_each_struct`: the typed value free releases storage but
/// never runs user code.
#[unsafe(no_mangle)]
pub extern "C" fn olive_dict_drop_each_struct(ptr: i64, hook: i64) {
    if ptr == 0 || hook == 0 {
        return;
    }
    let hook: ElementDropHook = unsafe { std::mem::transmute(hook as usize) };
    let obj = unsafe { &mut *(ptr as *mut OliveObj) };
    for v in obj.fields.values_mut() {
        if *v != 0 {
            let elem = *v;
            *v = 0;
            hook(elem);
        }
    }
}

/// Union values: only struct-boxed members decode into the hook (the shell
/// is released by the unbox); scalars pass through to the ordinary drop
/// untouched, so only hooked arms are zeroed.
#[unsafe(no_mangle)]
pub extern "C" fn olive_dict_drop_each_union(ptr: i64, hook: i64) {
    if ptr == 0 || hook == 0 {
        return;
    }
    let hook: ElementDropHook = unsafe { std::mem::transmute(hook as usize) };
    let obj = unsafe { &mut *(ptr as *mut OliveObj) };
    for v in obj.fields.values_mut() {
        if *v != 0 && crate::boxed::olive_any_is_struct_box(*v) != 0 {
            let inner = crate::struct_box::olive_struct_unbox_take(*v);
            *v = 0;
            hook(inner);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_free_obj(ptr: i64) {
    if ptr == 0 {
        return;
    }
    let Some(is_global) = crate::slab::slab_membership(ptr) else {
        return;
    };
    // Ownership is checked against the arena the slot actually lives in: a
    // dict received over a channel sits in the global escape arena, and a
    // purely local owns_addr scan would leak it (plus every field it owns).
    // The check itself stays because a struct slot's header word is a field
    // count that collides with KIND_OBJ.
    let is_ours = if is_global {
        crate::slab::with_escape_arena(|| obj_slab_owns(ptr))
    } else {
        obj_slab_owns(ptr)
    };
    if !is_ours {
        return;
    }
    if crate::slab::slot_is_live(ptr) {
        unsafe {
            let obj = &mut *(ptr as *mut OliveObj);
            for &val in obj.fields.values() {
                crate::free_any_word(val);
            }
            // Keys free like values, except raw structs: headerless field
            // counts collide with every kind tag, so the untyped free cannot
            // tell them apart and would corrupt the heap. Those leak here as
            // before; typed drops still reclaim them through descriptors.
            // Any-erased dicts hold no raw structs (all boxed), so freeing
            // the rest precisely reclaims boxes, enums, sequences, and owned
            // strings with no gaps.
            for k in obj.fields.keys() {
                if !crate::struct_obj::owns_struct_raw(k.0) {
                    crate::free_any_word(k.0);
                }
            }
            obj.fields.clear();
        }
    }
    free_obj_slot_raw_with(ptr, Some(is_global));
}

/// Whether `v` lives in an object slab. Gates dict key reads so raw structs
/// (whose headers collide with the object kind) never read past their slots.
pub(crate) fn owns_obj(v: i64) -> bool {
    obj_slab_owns(v)
}

fn obj_slab_owns(ptr: i64) -> bool {
    unsafe {
        let active = crate::slab::ACTIVE_SLABS.get();
        if !active.is_null() {
            (*active).obj.owns_addr(ptr as usize)
        } else {
            OBJ_SLAB.with(|sl| (*sl.get()).owns_addr(ptr as usize))
        }
    }
}

pub(crate) fn free_obj_slot_raw(ptr: i64) {
    free_obj_slot_raw_with(ptr, None);
}

/// `known_global` skips the chunk lookup when the caller already classified
/// `ptr` a moment ago (e.g. `olive_free_obj`'s own span check).
pub(crate) fn free_obj_slot_raw_with(ptr: i64, known_global: Option<bool>) {
    if !crate::slab::slot_is_live(ptr) {
        return;
    }
    #[cfg(debug_assertions)]
    unsafe {
        release_obj_storage(ptr as *mut u8);
    }
    let is_global = known_global.unwrap_or_else(|| crate::slab::chunk_is_global(ptr as usize));
    if is_global {
        crate::slab::with_escape_arena(|| free_obj_slot_raw_local(ptr));
    } else {
        free_obj_slot_raw_local(ptr);
    }
}

fn free_obj_slot_raw_local(ptr: i64) {
    unsafe {
        let active = crate::slab::ACTIVE_SLABS.get();
        if !active.is_null() {
            (*active).obj.free(ptr as *mut u8);
        } else {
            OBJ_SLAB.with(|sl| {
                (&mut *sl.get()).free(ptr as *mut u8);
            });
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_dict_new_reuse(old_ptr: i64, bump: i64) -> i64 {
    if old_ptr == 0 {
        return olive_obj_new();
    }
    if bump != 0 {
        unsafe {
            let gen_ptr = (old_ptr as *mut std::sync::atomic::AtomicU64).sub(1);
            let g = crate::slab::advance_generation(
                (*gen_ptr).load(std::sync::atomic::Ordering::Relaxed),
                2,
            );
            (*gen_ptr).store(g, std::sync::atomic::Ordering::Release);
        }
    }
    old_ptr
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_is_obj(val: i64) -> i64 {
    if val == 0 || (val & 1) != 0 {
        return 0;
    }
    let kind = unsafe { *(val as *const i64) };
    if kind == KIND_OBJ { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_keys(obj_ptr: i64) -> i64 {
    if obj_ptr == 0 {
        return crate::list::list_from_vec(Vec::new());
    }
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    let mut visited = rustc_hash::FxHashMap::default();
    // Keys are Any words: a `{int: _}` dict's key is a raw integer, and
    // olive_copy would read it as a tagged string pointer and strlen raw
    // int bits. copy_any dispatches on the actual runtime tag.
    let keys: Vec<i64> = m
        .fields
        .keys()
        .map(|k| crate::copy_typed::copy_any(k.0, &mut visited))
        .collect();
    crate::list::list_from_vec(keys)
}

/// Descriptor-driven `keys()`: each key copies through the dict's static
/// key type instead of kind dispatch. A raw struct key misreads by kind
/// (a 1-field header is `KIND_LIST`), so struct-keyed dicts must take
/// this entry; the compiler selects it exactly when the key type can
/// hold a raw struct. `dict_desc` is the full `Dict(K, V)` descriptor
/// with the key encoding at offset 1.
#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_keys_typed(obj_ptr: i64, dict_desc: i64) -> i64 {
    if obj_ptr == 0 {
        return crate::list::list_from_vec(Vec::new());
    }
    // SAFETY: same contract as the untyped snapshot/clear above — the
    // compiler passes a live dict of the statically described key/value
    // types. Copies and releases go through the descriptor, never raw
    // kind dispatch.
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    let mut visited = rustc_hash::FxHashMap::default();
    let desc = dict_desc as *const u8;
    let keys: Vec<i64> = m
        .fields
        .keys()
        .map(|k| {
            let mut pos = 1usize;
            crate::copy_typed::copy_val(k.0, desc, &mut pos, &mut visited)
        })
        .collect();
    crate::list::list_from_vec(keys)
}

/// Returns a list of `[key, value]` pairs, backing `for k, v in d.items()`.
#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_items(obj_ptr: i64) -> i64 {
    if obj_ptr == 0 {
        return crate::list::olive_list_new(0);
    }
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    let mut visited = rustc_hash::FxHashMap::default();
    let pairs: Vec<(i64, i64)> = m
        .fields
        .iter()
        .map(|(k, &v)| {
            (
                crate::copy_typed::copy_any(k.0, &mut visited),
                crate::copy_typed::copy_any(v, &mut visited),
            )
        })
        .collect();
    let outer = crate::list::olive_list_new(pairs.len() as i64);
    for (i, (k, v)) in pairs.iter().enumerate() {
        let pair = crate::list::olive_list_new(2);
        crate::list::olive_list_set(pair, 0, *k);
        crate::list::olive_list_set(pair, 1, *v);
        crate::list::olive_list_set(outer, i as i64, pair);
    }
    outer
}

/// Descriptor-driven `items()`: values copy through the dict's static value
/// type for the same reason as `olive_obj_values_typed` above; keys keep
/// the untyped copy (strings and scalars classify exactly by kind).
/// `dict_desc` is the full `Dict(K, V)` descriptor; the key part is skipped
/// to reach the value descriptor, mirroring `olive_obj_update_typed`.
#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_items_typed(obj_ptr: i64, dict_desc: i64) -> i64 {
    if obj_ptr == 0 {
        return crate::list::olive_list_new(0);
    }
    // SAFETY: same contract as the untyped snapshot/clear above — the
    // compiler passes a live dict of the statically described key/value
    // types. Copies and releases go through the descriptor, never raw
    // kind dispatch.
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    let mut visited = rustc_hash::FxHashMap::default();
    let desc = dict_desc as *const u8;
    let mut key_pos = 1usize;
    crate::format::skip(desc, &mut key_pos);
    let val_start = key_pos;
    let pairs: Vec<(i64, i64)> = m
        .fields
        .iter()
        .map(|(k, &v)| {
            let mut kp = 1usize;
            let mut vp = val_start;
            (
                crate::copy_typed::copy_val(k.0, desc, &mut kp, &mut visited),
                crate::copy_typed::copy_val(v, desc, &mut vp, &mut visited),
            )
        })
        .collect();
    let outer = crate::list::olive_list_new(pairs.len() as i64);
    for (i, (k, v)) in pairs.iter().enumerate() {
        let pair = crate::list::olive_list_new(2);
        crate::list::olive_list_set(pair, 0, *k);
        crate::list::olive_list_set(pair, 1, *v);
        crate::list::olive_list_set(outer, i as i64, pair);
    }
    outer
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_values(obj_ptr: i64) -> i64 {
    if obj_ptr == 0 {
        return crate::list::list_from_vec(Vec::new());
    }
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    let mut visited = rustc_hash::FxHashMap::default();
    let values: Vec<i64> = m
        .fields
        .values()
        .map(|&v| crate::copy_typed::copy_any(v, &mut visited))
        .collect();
    crate::list::list_from_vec(values)
}

/// Descriptor-driven `values()`: copies each value through the dict's static
/// value type instead of kind dispatch. A raw struct word misreads by kind
/// (a 1-field header is `KIND_LIST`), so struct-valued dicts must take this
/// entry; the compiler selects it exactly when the value type owns heap
/// data, mirroring `setdefault`'s typed dispatch. `dict_desc` is the full
/// `Dict(K, V)` descriptor; the key part is skipped to reach the value
/// descriptor, mirroring `olive_obj_update_typed`.
#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_values_typed(obj_ptr: i64, dict_desc: i64) -> i64 {
    if obj_ptr == 0 {
        return crate::list::list_from_vec(Vec::new());
    }
    // SAFETY: same contract as the untyped snapshot/clear above — the
    // compiler passes a live dict of the statically described key/value
    // types. Copies and releases go through the descriptor, never raw
    // kind dispatch.
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    let mut visited = rustc_hash::FxHashMap::default();
    let desc = dict_desc as *const u8;
    let mut key_pos = 1usize;
    crate::format::skip(desc, &mut key_pos);
    let val_start = key_pos;
    let values: Vec<i64> = m
        .fields
        .values()
        .map(|&v| {
            let mut pos = val_start;
            crate::copy_typed::copy_val(v, desc, &mut pos, &mut visited)
        })
        .collect();
    crate::list::list_from_vec(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::olive_str_internal;

    fn s(text: &str) -> i64 {
        olive_str_internal(text)
    }

    fn make_obj(pairs: &[(&str, i64)]) -> i64 {
        let obj = olive_obj_new();
        for (k, v) in pairs {
            olive_obj_set(obj, s(k), *v);
        }
        obj
    }

    #[test]
    fn values_typed_shares_struct_value() {
        use crate::format::{D_DICT, D_STR, D_STRUCT_SHARED};
        use crate::slab::slot_is_live;
        let desc = [
            D_DICT,
            D_STR,
            D_STRUCT_SHARED,
            14,
            b'R',
            14,
            14,
            b's',
            D_STR,
        ];
        let desc_ptr = desc.as_ptr() as i64;
        let text = olive_str_internal("typed dict value resource field");
        let value = crate::olive_struct_alloc(1);
        unsafe { *((value as *mut i64).add(1)) = text };
        let obj = olive_obj_new();
        olive_obj_set(obj, s("k"), value);
        let vs = olive_obj_values_typed(obj, desc_ptr);
        assert_eq!(crate::list::olive_list_len(vs), 1);
        assert_eq!(crate::list::olive_list_get(vs, 0), value);
        assert!(slot_is_live(value));
        let list_desc = [
            crate::format::D_LIST,
            D_STRUCT_SHARED,
            14,
            b'R',
            14,
            14,
            b's',
            D_STR,
        ];
        crate::free_typed::olive_free_typed(vs, list_desc.as_ptr() as i64);
        assert!(slot_is_live(value));
        crate::free_typed::olive_free_typed(obj, desc_ptr);
        assert!(!slot_is_live(value));
    }

    #[test]
    fn items_typed_shares_struct_value() {
        use crate::format::{D_DICT, D_LIST, D_STR, D_STRUCT_SHARED};
        use crate::slab::slot_is_live;
        let desc = [
            D_DICT,
            D_STR,
            D_STRUCT_SHARED,
            14,
            b'R',
            14,
            14,
            b's',
            D_STR,
        ];
        let desc_ptr = desc.as_ptr() as i64;
        let text = olive_str_internal("typed dict items resource field");
        let value = crate::olive_struct_alloc(1);
        unsafe { *((value as *mut i64).add(1)) = text };
        let obj = olive_obj_new();
        olive_obj_set(obj, s("k"), value);
        let ps = olive_obj_items_typed(obj, desc_ptr);
        assert_eq!(crate::list::olive_list_len(ps), 1);
        let pair = crate::list::olive_list_get(ps, 0);
        assert_eq!(
            crate::olive_str_from_ptr(crate::list::olive_list_get(pair, 0)),
            "k"
        );
        assert_eq!(crate::list::olive_list_get(pair, 1), value);
        crate::free_typed::olive_free_typed(obj, desc_ptr);
        assert!(slot_is_live(value));
        let outer_desc = [
            D_LIST,
            crate::format::D_TUPLE,
            3,
            D_STR,
            D_STRUCT_SHARED,
            14,
            b'R',
            14,
            14,
            b's',
            D_STR,
        ];
        crate::free_typed::olive_free_typed(ps, outer_desc.as_ptr() as i64);
        assert!(!slot_is_live(value));
    }

    #[test]
    fn new_obj_creates_empty() {
        let obj = olive_obj_new();
        assert_ne!(obj, 0);
        assert_eq!(olive_obj_len(obj), 0);
    }

    #[test]
    fn set_and_get() {
        let obj = olive_obj_new();
        olive_obj_set(obj, s("key"), 42);
        assert_eq!(olive_obj_get(obj, s("key")), 42);
    }

    #[test]
    fn get_missing_key() {
        let obj = olive_obj_new();
        assert_eq!(olive_obj_get(obj, s("nonexistent")), 0);
    }

    #[test]
    fn get_default_present() {
        let obj = make_obj(&[("x", 1)]);
        assert_eq!(olive_obj_get_default(obj, s("x"), 99), 1);
    }

    #[test]
    fn get_default_missing() {
        let obj = olive_obj_new();
        assert_eq!(olive_obj_get_default(obj, s("missing"), 99), 99);
    }

    #[test]
    fn overwrite_value() {
        let obj = make_obj(&[("x", 1)]);
        olive_obj_set(obj, s("x"), 99);
        assert_eq!(olive_obj_get(obj, s("x")), 99);
    }

    #[test]
    fn remove_key() {
        let obj = make_obj(&[("a", 1), ("b", 2)]);
        let removed = olive_obj_remove(obj, s("a"));
        assert_eq!(removed, 1);
        assert_eq!(olive_obj_get(obj, s("a")), 0);
        assert_eq!(olive_obj_len(obj), 1);
    }

    #[test]
    fn remove_nonexistent() {
        let obj = make_obj(&[("x", 1)]);
        assert_eq!(olive_obj_remove(obj, s("y")), 0);
    }

    #[test]
    fn in_obj_true() {
        let obj = make_obj(&[("key", 42)]);
        assert_eq!(olive_in_obj(s("key"), obj), 1);
    }

    #[test]
    fn in_obj_false() {
        let obj = make_obj(&[("key", 42)]);
        assert_eq!(olive_in_obj(s("nope"), obj), 0);
    }

    #[test]
    fn len_multiple_fields() {
        let obj = make_obj(&[("a", 1), ("b", 2), ("c", 3)]);
        assert_eq!(olive_obj_len(obj), 3);
    }

    #[test]
    fn keys_list() {
        let obj = make_obj(&[("x", 1), ("y", 2)]);
        let keys_ptr = olive_obj_keys(obj);
        assert_ne!(keys_ptr, 0);
        let s = unsafe { &*(keys_ptr as *const StableVec) };
        assert_eq!(s.len, 2);
    }

    #[test]
    fn keys_list_int_keys() {
        let obj = olive_obj_new();
        olive_obj_set(obj, 2, 10);
        olive_obj_set(obj, 4, 20);
        let keys_ptr = olive_obj_keys(obj);
        let s = unsafe { &*(keys_ptr as *const StableVec) };
        assert_eq!(s.len, 2);
        let k0 = unsafe { *s.ptr };
        let k1 = unsafe { *s.ptr.add(1) };
        assert!((k0 == 2 && k1 == 4) || (k0 == 4 && k1 == 2));
    }

    #[test]
    fn values_list() {
        let obj = make_obj(&[("a", 10), ("b", 20)]);
        let vals_ptr = olive_obj_values(obj);
        assert_ne!(vals_ptr, 0);
        let s = unsafe { &*(vals_ptr as *const StableVec) };
        assert_eq!(s.len, 2);
        let v0 = unsafe { *s.ptr };
        let v1 = unsafe { *s.ptr.add(1) };
        assert!((v0 == 10 && v1 == 20) || (v0 == 20 && v1 == 10));
    }

    #[test]
    fn is_obj_true() {
        let obj = olive_obj_new();
        assert_eq!(olive_is_obj(obj), 1);
    }

    #[test]
    fn is_obj_false() {
        assert_eq!(olive_is_obj(0), 0);
        assert_eq!(olive_is_obj(1), 0);
        assert_eq!(olive_is_obj(1 | 1), 0);
    }

    #[test]
    fn free_obj_no_panic() {
        let obj = make_obj(&[("x", 1)]);
        olive_free_obj(obj);

        let obj2 = olive_obj_new();
        assert_ne!(obj2, 0);
    }

    #[test]
    fn clear_releases_string_values() {
        let obj = olive_obj_new();
        let a = olive_str_internal("alpha");
        let b = olive_str_internal("beta");
        let ga = crate::string_slab::olive_str_gen_of(a);
        let gb = crate::string_slab::olive_str_gen_of(b);
        olive_obj_set(obj, 1, a);
        olive_obj_set(obj, 2, b);
        olive_obj_clear(obj);
        assert_eq!(olive_obj_len(obj), 0);
        assert_eq!(crate::string_slab::olive_str_gen_stale(a, ga), 1);
        assert_eq!(crate::string_slab::olive_str_gen_stale(b, gb), 1);
        olive_free_obj(obj);
    }

    #[test]
    fn free_releases_string_values() {
        let obj = olive_obj_new();
        let a = olive_str_internal("gamma");
        let ga = crate::string_slab::olive_str_gen_of(a);
        olive_obj_set(obj, 1, a);
        olive_free_obj(obj);
        assert_eq!(crate::string_slab::olive_str_gen_stale(a, ga), 1);
    }

    #[test]
    fn replacing_set_releases_displaced_string_value() {
        use crate::format::D_STR;
        let val_desc = [D_STR];
        let desc_ptr = val_desc.as_ptr() as i64;
        let dict = olive_obj_new();
        let old = olive_str_internal("old-dict-val");
        let gold = crate::string_slab::olive_str_gen_of(old);
        olive_obj_set(dict, 1, old);
        let new = olive_str_internal("new-dict-val");
        let gnew = crate::string_slab::olive_str_gen_of(new);
        olive_obj_set_replacing_typed(dict, 1, new, 0, desc_ptr);
        assert_eq!(olive_obj_get(dict, 1), new);
        assert_eq!(crate::string_slab::olive_str_gen_stale(old, gold), 1);
        assert_eq!(crate::olive_str_from_ptr(new), "new-dict-val");
        olive_free_obj(dict);
        assert_eq!(crate::string_slab::olive_str_gen_stale(new, gnew), 1);
    }

    #[test]
    fn replacing_set_self_assignment_keeps_value() {
        use crate::format::D_STR;
        let val_desc = [D_STR];
        let desc_ptr = val_desc.as_ptr() as i64;
        let dict = olive_obj_new();
        let a = olive_str_internal("same-dict-val");
        let g = crate::string_slab::olive_str_gen_of(a);
        olive_obj_set(dict, 1, a);
        olive_obj_set_replacing_typed(dict, 1, a, 0, desc_ptr);
        assert_eq!(olive_obj_get(dict, 1), a);
        assert_eq!(crate::string_slab::olive_str_gen_stale(a, g), 0);
        olive_free_obj(dict);
        assert_eq!(crate::string_slab::olive_str_gen_stale(a, g), 1);
    }

    #[test]
    fn plain_set_leaves_displaced_value_for_caller() {
        // Contract `update_typed`'s deferred-free protocol relies on: plain
        // `olive_obj_set` takes no action on the value it displaces.
        let dict = olive_obj_new();
        let old = olive_str_internal("displaced-dict-val");
        let gold = crate::string_slab::olive_str_gen_of(old);
        olive_obj_set(dict, 1, old);
        let new = olive_str_internal("fresh-dict-val");
        let gnew = crate::string_slab::olive_str_gen_of(new);
        olive_obj_set(dict, 1, new);
        assert_eq!(olive_obj_get(dict, 1), new);
        assert_eq!(crate::string_slab::olive_str_gen_stale(old, gold), 0);
        crate::olive_free_str(old);
        assert_eq!(crate::string_slab::olive_str_gen_stale(old, gold), 1);
        olive_free_obj(dict);
        assert_eq!(crate::string_slab::olive_str_gen_stale(new, gnew), 1);
    }

    #[test]
    fn setdefault_hit_releases_orphaned_default() {
        let dict = olive_obj_new();
        let kept = olive_str_internal("kept-val");
        let gk = crate::string_slab::olive_str_gen_of(kept);
        olive_obj_set(dict, 1, kept);
        let dropped = olive_str_internal("dropped-val");
        let gd = crate::string_slab::olive_str_gen_of(dropped);
        let got = olive_obj_setdefault(dict, 1, dropped, 0);
        assert_eq!(got, kept);
        assert_eq!(crate::olive_str_from_ptr(got), "kept-val");
        assert_eq!(crate::string_slab::olive_str_gen_stale(dropped, gd), 1);
        olive_free_obj(dict);
        assert_eq!(crate::string_slab::olive_str_gen_stale(kept, gk), 1);
    }

    #[test]
    fn setdefault_miss_stores_and_returns_default() {
        let dict = olive_obj_new();
        let d = olive_str_internal("fresh-default");
        let g = crate::string_slab::olive_str_gen_of(d);
        let got = olive_obj_setdefault(dict, 7, d, 0);
        assert_eq!(got, d);
        assert_eq!(olive_obj_get(dict, 7), d);
        assert_eq!(crate::string_slab::olive_str_gen_stale(d, g), 0);
        olive_free_obj(dict);
        assert_eq!(crate::string_slab::olive_str_gen_stale(d, g), 1);
    }

    #[test]
    fn setdefault_hit_frees_struct_default_through_desc() {
        use crate::format::{D_DICT, D_INT, D_STR, D_STRUCT};
        // One-field struct `P` with a string field `x`, inside
        // `dict[int, P]`: the hit releases the orphaned default through
        // the value descriptor instead of kind dispatch (which would read
        // the struct's field-count header as a kind tag). Teardown goes
        // through the typed dict free, the path compiled code uses.
        let struct_desc = [D_STRUCT, 14, b'P', 14, 14, b'x', D_STR];
        let dict_desc = [D_DICT, D_INT, D_STRUCT, 14, b'P', 14, 14, b'x', D_STR];
        let dict = olive_obj_new();
        let kept = crate::struct_obj::olive_struct_alloc(1);
        let ks = crate::olive_str_internal("kept");
        let gk = crate::string_slab::olive_str_gen_of(ks);
        unsafe { *((kept + 8) as *mut i64) = ks };
        olive_obj_set(dict, 1, kept);
        let orphan = crate::struct_obj::olive_struct_alloc(1);
        let s = crate::olive_str_internal("orphan-payload");
        let g = crate::string_slab::olive_str_gen_of(s);
        unsafe { *((orphan + 8) as *mut i64) = s };
        let got = olive_obj_setdefault(dict, 1, orphan, struct_desc.as_ptr() as i64);
        assert_eq!(got, kept);
        assert_eq!(crate::string_slab::olive_str_gen_stale(s, g), 1);
        crate::free_typed::olive_free_typed(dict, dict_desc.as_ptr() as i64);
        assert_eq!(crate::string_slab::olive_str_gen_stale(ks, gk), 1);
    }

    #[test]
    fn clear_typed_releases_struct_values_and_keys() {
        use crate::format::{D_DICT, D_STR, D_STRUCT};
        use crate::slab::slot_is_live;
        let dict_desc = [D_DICT, D_STR, D_STRUCT, 14, b'P', 14, 14, b'x', D_STR];
        let dict = olive_obj_new();
        let value = crate::struct_obj::olive_struct_alloc(1);
        let text = crate::olive_str_internal("typed dict clear payload");
        let gt = crate::string_slab::olive_str_gen_of(text);
        unsafe { *((value + 8) as *mut i64) = text };
        let key = crate::olive_str_internal("typed-dict-clear-heap-key-0123456789");
        let gk = crate::string_slab::olive_str_gen_of(key);
        olive_obj_set(dict, key, value);
        olive_obj_clear_typed(dict, dict_desc.as_ptr() as i64);
        assert!(!slot_is_live(value));
        assert_eq!(crate::string_slab::olive_str_gen_stale(text, gt), 1);
        // The dict keeps a private copy of string keys; the caller's own
        // key stays live until the caller frees it, like a scope exit.
        assert_eq!(crate::string_slab::olive_str_gen_stale(key, gk), 0);
        crate::olive_free_str(key);
        assert_eq!(crate::string_slab::olive_str_gen_stale(key, gk), 1);
        assert_eq!(olive_obj_len(dict), 0);
        assert!(slot_is_live(dict));
        olive_free_obj(dict);
    }

    #[test]
    fn keys_typed_copies_struct_keys() {
        use crate::format::{D_DICT, D_INT, D_STR, D_STRUCT_SHARED};
        use crate::slab::slot_is_live;
        let desc = [
            D_DICT,
            D_STRUCT_SHARED,
            14,
            b'K',
            14,
            14,
            b'k',
            D_STR,
            D_INT,
        ];
        let desc_ptr = desc.as_ptr() as i64;
        let text = olive_str_internal("typed keys resource field value");
        let key = crate::olive_struct_alloc(1);
        unsafe { *((key as *mut i64).add(1)) = text };
        let obj = olive_obj_new();
        olive_obj_set(obj, key, 7);
        let ks = olive_obj_keys_typed(obj, desc_ptr);
        assert_eq!(crate::list::olive_list_len(ks), 1);
        assert_eq!(crate::list::olive_list_get(ks, 0), key);
        assert!(slot_is_live(key));
        let list_desc = [
            crate::format::D_LIST,
            D_STRUCT_SHARED,
            14,
            b'K',
            14,
            14,
            b'k',
            D_STR,
        ];
        crate::free_typed::olive_free_typed(ks, list_desc.as_ptr() as i64);
        assert!(slot_is_live(key));
        crate::free_typed::olive_free_typed(obj, desc_ptr);
        assert!(!slot_is_live(key));
    }
}
