use crate::python::python_bindings::PyInterpreterConfig;
use crate::python::python_noop;
use crate::python::*;
use std::os::raw::{c_int, c_void};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

const MAX_POOL: i32 = 64;

static POOL_ACTIVE: AtomicBool = AtomicBool::new(false);
static POOL_SIZE: AtomicI32 = AtomicI32::new(0);
static NEXT_SLOT: AtomicI32 = AtomicI32::new(0);

static mut INTERP_STATES: [*mut c_void; MAX_POOL as usize] =
    [std::ptr::null_mut(); MAX_POOL as usize];
static mut INIT_THREAD_STATES: [*mut c_void; MAX_POOL as usize] =
    [std::ptr::null_mut(); MAX_POOL as usize];

const PY_INTERPRETER_CONFIG_OWN_GIL: c_int = 2;

thread_local! {
    static SUBINTERP_SLOT: std::cell::Cell<i32> = const { std::cell::Cell::new(-1) };
    static SUBINTERP_TS: std::cell::Cell<*mut c_void> = const { std::cell::Cell::new(std::ptr::null_mut()) };
}

unsafe fn interp_state_raw() -> *mut *mut c_void {
    std::ptr::addr_of_mut!(INTERP_STATES).cast()
}

unsafe fn init_ts_raw() -> *mut *mut c_void {
    std::ptr::addr_of_mut!(INIT_THREAD_STATES).cast()
}

unsafe fn read_interp_state(idx: usize) -> *mut c_void {
    unsafe { *interp_state_raw().add(idx) }
}

unsafe fn write_interp_state(idx: usize, val: *mut c_void) {
    unsafe { interp_state_raw().add(idx).write(val) };
}

unsafe fn read_init_ts(idx: usize) -> *mut c_void {
    unsafe { *init_ts_raw().add(idx) }
}

unsafe fn write_init_ts(idx: usize, val: *mut c_void) {
    unsafe { init_ts_raw().add(idx).write(val) };
}

macro_rules! noop_eq {
    ($ptr:expr, $noop:expr) => {{
        let p: *const () = $ptr as *const ();
        let n: *const () = $noop as *const ();
        p == n
    }};
}

pub fn pool_is_active() -> bool {
    POOL_ACTIVE.load(Ordering::Acquire)
}

fn assign_slot() -> i32 {
    let size = POOL_SIZE.load(Ordering::Acquire);
    if size == 0 {
        return -1;
    }
    loop {
        let slot = NEXT_SLOT.load(Ordering::Relaxed);
        if slot >= size {
            return -1;
        }
        if NEXT_SLOT
            .compare_exchange_weak(slot, slot + 1, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            return slot;
        }
    }
}

pub unsafe fn pool_ensure() -> bool {
    unsafe {
        let slot = SUBINTERP_SLOT.with(|s| s.get());
        if slot < 0 {
            let new_slot = assign_slot();
            if new_slot < 0 {
                return false;
            }
            SUBINTERP_SLOT.with(|s| s.set(new_slot));
            let interp = read_interp_state(new_slot as usize);
            if interp.is_null() {
                return false;
            }
            let ts = PY_THREAD_STATE_NEW(interp);
            if ts.is_null() {
                return false;
            }
            SUBINTERP_TS.with(|t| t.set(ts));
        }
        let ts = SUBINTERP_TS.with(|t| t.get());
        PY_EVAL_ACQUIRE_THREAD(ts);
        true
    }
}

pub unsafe fn pool_release() {
    unsafe {
        let ts = SUBINTERP_TS.with(|t| t.get());
        if !ts.is_null() {
            PY_EVAL_RELEASE_THREAD(ts);
        }
    }
}

pub unsafe fn pool_init() {
    unsafe {
        if noop_eq!(PY_EVAL_ACQUIRE_THREAD, python_noop::noop_acquire_thread)
            || noop_eq!(PY_EVAL_RELEASE_THREAD, python_noop::noop_release_thread)
            || noop_eq!(
                PY_NEW_INTERPRETER_FROM_CONFIG,
                python_noop::noop_new_interpreter
            )
            || noop_eq!(PY_END_INTERPRETER, python_noop::noop_end_interpreter)
            || noop_eq!(PY_THREAD_STATE_NEW, python_noop::noop_thread_state_new)
            || noop_eq!(PY_THREAD_STATE_SWAP, python_noop::noop_thread_state_swap)
            || noop_eq!(
                PY_INTERPRETER_STATE_GET,
                python_noop::noop_interpreter_state_get
            )
            || noop_eq!(PY_THREAD_STATE_CLEAR, python_noop::noop_thread_state_clear)
            || noop_eq!(
                PY_THREAD_STATE_DELETE,
                python_noop::noop_thread_state_delete
            )
        {
            return;
        }

        let size = std::env::var("OLIVE_PY_SUBINTERP_SIZE")
            .ok()
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or_else(|| {
                std::thread::available_parallelism()
                    .map(|n| n.get() as i32)
                    .unwrap_or(4)
            })
            .min(MAX_POOL)
            .max(1);

        let config = PyInterpreterConfig {
            use_main_obmalloc: 0,
            allow_fork: 0,
            allow_exec: 0,
            allow_threads: 1,
            allow_daemon_threads: 0,
            check_multi_interp_extensions: 1,
            gil: PY_INTERPRETER_CONFIG_OWN_GIL,
        };

        let main_ts = PY_THREAD_STATE_GET();
        if main_ts.is_null() {
            return;
        }

        for i in 0..size {
            let mut sub_ts: *mut c_void = std::ptr::null_mut();
            let status = PY_NEW_INTERPRETER_FROM_CONFIG(&mut sub_ts, &config);
            if status._type != 0 || sub_ts.is_null() {
                for j in 0..i {
                    let init_ts = read_init_ts(j as usize);
                    if !init_ts.is_null() {
                        PY_EVAL_RELEASE_THREAD(init_ts);
                        PY_EVAL_ACQUIRE_THREAD(main_ts);
                        write_init_ts(j as usize, std::ptr::null_mut());
                        write_interp_state(j as usize, std::ptr::null_mut());
                    }
                }
                return;
            }

            let interp = PY_INTERPRETER_STATE_GET();
            write_interp_state(i as usize, interp);
            write_init_ts(i as usize, sub_ts);

            PY_EVAL_RELEASE_THREAD(sub_ts);
            PY_THREAD_STATE_SWAP(main_ts);
        }

        POOL_SIZE.store(size, Ordering::Release);
        POOL_ACTIVE.store(true, Ordering::Release);
    }
}

pub unsafe fn pool_finalize() {
    unsafe {
        if !POOL_ACTIVE.load(Ordering::Acquire) {
            return;
        }
        POOL_ACTIVE.store(false, Ordering::Release);

        let size = POOL_SIZE.load(Ordering::Acquire);

        let main_ts = PY_THREAD_STATE_GET();
        if main_ts.is_null() {
            return;
        }

        for i in 0..size {
            let init_ts = read_init_ts(i as usize);
            if init_ts.is_null() {
                continue;
            }

            PY_EVAL_RELEASE_THREAD(main_ts);
            PY_EVAL_ACQUIRE_THREAD(init_ts);
            PY_END_INTERPRETER(init_ts);
            PY_THREAD_STATE_SWAP(main_ts);

            write_interp_state(i as usize, std::ptr::null_mut());
            write_init_ts(i as usize, std::ptr::null_mut());
        }

        POOL_SIZE.store(0, Ordering::Release);
        NEXT_SLOT.store(0, Ordering::Release);
    }
}
