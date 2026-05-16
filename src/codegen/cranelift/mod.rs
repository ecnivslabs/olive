mod async_sm;
mod imports;
mod translate;

use crate::mir::{Constant, Local, MirFunction, Operand, Rvalue, StatementKind};
use cranelift::codegen::ir::ArgumentPurpose;
use cranelift::prelude::*;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use rustc_hash::FxHashMap as HashMap;
use std::process;

pub(super) const KIND_SM_FUTURE: i64 = 5;

pub(super) static SYMBOL_MAP: &[(&str, &[u8])] = &[
    ("__olive_time_now", b"olive_time_now\0"),
    ("__olive_time_monotonic", b"olive_time_monotonic\0"),
    ("__olive_time_sleep", b"olive_time_sleep\0"),
    ("__olive_pow", b"olive_pow\0"),
    ("__olive_pow_float", b"olive_pow_float\0"),
    ("__olive_math_sin", b"olive_math_sin\0"),
    ("__olive_math_cos", b"olive_math_cos\0"),
    ("__olive_math_tan", b"olive_math_tan\0"),
    ("__olive_math_asin", b"olive_math_asin\0"),
    ("__olive_math_acos", b"olive_math_acos\0"),
    ("__olive_math_atan", b"olive_math_atan\0"),
    ("__olive_math_atan2", b"olive_math_atan2\0"),
    ("__olive_math_log", b"olive_math_log\0"),
    ("__olive_math_log10", b"olive_math_log10\0"),
    ("__olive_math_exp", b"olive_math_exp\0"),
    ("__olive_random_seed", b"olive_random_seed\0"),
    ("__olive_random_get", b"olive_random_get\0"),
    ("__olive_random_int", b"olive_random_int\0"),
    ("__olive_print_int", b"olive_print\0"),
    ("__olive_print_float", b"olive_print_float\0"),
    ("__olive_print_str", b"olive_print_str\0"),
    ("__olive_print_list", b"olive_print_list\0"),
    ("__olive_print_obj", b"olive_print_obj\0"),
    ("__olive_str", b"olive_str\0"),
    ("__olive_int", b"olive_int\0"),
    ("__olive_float", b"olive_float\0"),
    ("__olive_bool", b"olive_bool\0"),
    ("__olive_bool_from_float", b"olive_bool_from_float\0"),
    ("__olive_float_to_str", b"olive_float_to_str\0"),
    ("__olive_float_to_int", b"olive_float_to_int\0"),
    ("__olive_int_to_float", b"olive_int_to_float\0"),
    ("__olive_str_to_int", b"olive_str_to_int\0"),
    ("__olive_str_to_float", b"olive_str_to_float\0"),
    ("__olive_str_concat", b"olive_str_concat\0"),
    ("__olive_str_eq", b"olive_str_eq\0"),
    ("__olive_str_len", b"olive_str_len\0"),
    ("__olive_str_get", b"olive_str_get\0"),
    ("__olive_copy", b"olive_copy\0"),
    ("__olive_copy_float", b"olive_copy_float\0"),
    ("__olive_list_new", b"olive_list_new\0"),
    ("__olive_list_set", b"olive_list_set\0"),
    ("__olive_list_get", b"olive_list_get\0"),
    ("__olive_list_len", b"olive_list_len\0"),
    ("__olive_list_append", b"olive_list_append\0"),
    ("__olive_obj_new", b"olive_obj_new\0"),
    ("__olive_obj_set", b"olive_obj_set\0"),
    ("__olive_obj_get", b"olive_obj_get\0"),
    ("__olive_obj_len", b"olive_obj_len\0"),
    ("__olive_enum_new", b"olive_enum_new\0"),
    ("__olive_enum_tag", b"olive_enum_tag\0"),
    ("__olive_enum_type_id", b"olive_enum_type_id\0"),
    ("__olive_enum_get", b"olive_enum_get\0"),
    ("__olive_enum_set", b"olive_enum_set\0"),
    ("__olive_free_enum", b"olive_free_enum\0"),
    ("__olive_free_str", b"olive_free_str\0"),
    ("__olive_free_list", b"olive_free_list\0"),
    ("__olive_free_obj", b"olive_free_obj\0"),
    ("__olive_struct_alloc", b"olive_struct_alloc\0"),
    ("__olive_free_struct", b"olive_free_struct\0"),
    ("__olive_free_c_struct", b"olive_free_c_struct\0"),
    ("__olive_iter", b"olive_iter\0"),
    ("__olive_has_next", b"olive_has_next\0"),
    ("__olive_next", b"olive_next\0"),
    ("__olive_alloc", b"olive_alloc\0"),
    ("__olive_vararg_call", b"olive_vararg_call\0"),
    ("__olive_cache_has", b"olive_cache_has\0"),
    ("__olive_cache_get", b"olive_cache_get\0"),
    ("__olive_cache_set", b"olive_cache_set\0"),
    ("__olive_memo_get", b"olive_memo_get\0"),
    ("__olive_cache_has_tuple", b"olive_cache_has_tuple\0"),
    ("__olive_cache_get_tuple", b"olive_cache_get_tuple\0"),
    ("__olive_cache_set_tuple", b"olive_cache_set_tuple\0"),
    ("__olive_get_index_any", b"olive_get_index_any\0"),
    ("__olive_set_index_any", b"olive_set_index_any\0"),
    ("__olive_free_any", b"olive_free_any\0"),
    ("__olive_free", b"olive_free_any\0"),
    ("__olive_file_read", b"olive_file_read\0"),
    ("__olive_file_write", b"olive_file_write\0"),
    ("__olive_make_future", b"olive_make_future\0"),
    ("__olive_await", b"olive_await_future\0"),
    ("__olive_spawn_task", b"olive_spawn_task\0"),
    ("__olive_free_future", b"olive_free_future\0"),
    ("__olive_gather", b"olive_gather\0"),
    ("__olive_select", b"olive_select\0"),
    ("__olive_cancel_future", b"olive_cancel_future\0"),
    ("__olive_sm_poll", b"olive_sm_poll\0"),
    ("__olive_async_file_read", b"olive_async_file_read\0"),
    ("__olive_async_file_write", b"olive_async_file_write\0"),
    ("__olive_net_tcp_connect", b"olive_net_tcp_connect\0"),
    ("__olive_net_tcp_send", b"olive_net_tcp_send\0"),
    ("__olive_net_tcp_recv", b"olive_net_tcp_recv\0"),
    ("__olive_net_tcp_close", b"olive_net_tcp_close\0"),
    ("__olive_http_get", b"olive_http_get\0"),
    ("__olive_http_post", b"olive_http_post\0"),
    ("__olive_in_list", b"olive_in_list\0"),
    ("__olive_in_obj", b"olive_in_obj\0"),
    ("__olive_set_add", b"olive_set_add\0"),
    ("__olive_set_new", b"olive_set_new\0"),
    ("__olive_str_char", b"olive_str_char\0"),
    ("__olive_str_slice", b"olive_str_slice\0"),
    ("__olive_list_concat", b"olive_list_concat\0"),
    ("__olive_ffi_errno", b"olive_ffi_errno\0"),
];
pub(super) const POLL_PENDING: i64 = i64::MIN;

const ASYNC_RUNTIME_SYMS: &[&str] = &[
    "__olive_make_future",
    "__olive_await",
    "__olive_spawn_task",
    "__olive_alloc",
    "__olive_free_future",
    "__olive_sm_poll",
];

pub(super) struct SmAwaitPoint {
    pub(super) bb_idx: usize,
    pub(super) stmt_idx: usize,
    pub(super) result_local: Local,
    pub(super) sub_future_local: Local,
}

pub(super) struct FfiFnEntry {
    pub(super) jit_name: String,
    pub(super) c_name: String,
    pub(super) params: Vec<String>,
    pub(super) ret: Option<String>,
    pub(super) is_vararg: bool,
    pub(super) n_fixed: usize,
    pub(super) call_conv: Option<String>,
    pub(super) use_sret: bool,
}

pub struct CraneliftCodegen<'a, M: Module> {
    pub(super) functions: &'a [MirFunction],
    pub(super) module: M,
    pub(super) func_ids: HashMap<String, FuncId>,
    pub(super) string_ids: HashMap<String, DataId>,
    pub(super) struct_fields: HashMap<String, Vec<String>>,
    pub(super) _libs: Vec<libloading::Library>,
    pub(super) native_aliases: std::collections::HashSet<String>,
    pub(super) ffi_entries: Vec<FfiFnEntry>,
    pub(super) ffi_vararg_ptrs: HashMap<String, *const u8>,
    pub(super) ffi_vararg_ids: std::collections::HashSet<String>,
    pub(super) c_struct_offsets: HashMap<String, Vec<(String, i32, String, Option<(u8, u8)>)>>,
    pub(super) c_struct_sizes: HashMap<String, i64>,
    pub(super) c_struct_names: std::collections::HashSet<String>,
    pub(super) c_struct_destructors: HashMap<String, String>,
    pub(super) aot: bool,
    pub(super) extern_var_ptrs: HashMap<String, (i64, String, String)>,
}

fn c_prim_layout(ty: &str) -> (i32, i32) {
    match ty {
        "f64" | "i64" | "u64" | "ptr" => (8, 8),
        "f32" | "i32" | "u32" => (4, 4),
        "i16" | "u16" => (2, 2),
        "i8" | "u8" | "bool" => (1, 1),
        _ if ty.starts_with('[') => {
            if let Some(semi) = ty.find(';') {
                let elem = &ty[1..semi];
                let n: i32 = ty[semi + 1..ty.len() - 1].parse().unwrap_or(1);
                let (elem_size, elem_align) = c_prim_layout(elem);
                (elem_size * n, elem_align)
            } else {
                (8, 8)
            }
        }
        _ => (8, 8),
    }
}

fn c_abi_layout(
    fields: &[crate::parser::ast::FfiStructField],
    is_union: bool,
) -> (Vec<(String, i32, String, Option<(u8, u8)>)>, i64) {
    if is_union {
        let mut max_size = 0i32;
        let mut max_align = 1i32;
        let mut layout = Vec::new();
        for field in fields {
            let ty = type_expr_to_name(&field.ty);
            let (size, align) = c_prim_layout(&ty);
            max_align = max_align.max(align);
            max_size = max_size.max(size);
            layout.push((field.name.clone(), 0, ty.clone(), None));
        }
        let total = if max_align > 0 {
            let r = max_size % max_align;
            if r == 0 {
                max_size
            } else {
                max_size + max_align - r
            }
        } else {
            max_size
        };
        return (layout, total as i64);
    }
    let mut offset = 0i32;
    let mut layout = Vec::new();
    let mut max_align = 1i32;
    let mut current_bit_offset = 0i32;
    let mut last_bitfield_size = 0i32;

    for field in fields {
        let ty = type_expr_to_name(&field.ty);
        let (size, align) = c_prim_layout(&ty);
        max_align = max_align.max(align);

        if let Some(bits) = field.bits {
            if current_bit_offset == 0
                || (current_bit_offset + (bits as i32) > last_bitfield_size * 8)
                || size != last_bitfield_size
            {
                let padding = (align - (offset % align)) % align;
                offset += padding;
                layout.push((field.name.clone(), offset, ty.clone(), Some((0u8, bits))));
                last_bitfield_size = size;
                current_bit_offset = bits as i32;
                offset += size;
            } else {
                let word_offset = offset - last_bitfield_size;
                let bit_off = current_bit_offset as u8;
                layout.push((
                    field.name.clone(),
                    word_offset,
                    ty.clone(),
                    Some((bit_off, bits)),
                ));
                current_bit_offset += bits as i32;
            }
        } else {
            current_bit_offset = 0;
            last_bitfield_size = 0;
            let padding = (align - (offset % align)) % align;
            offset += padding;
            layout.push((field.name.clone(), offset, ty.clone(), None));
            offset += size;
        }
    }
    let total = if max_align > 0 {
        let r = offset % max_align;
        if r == 0 {
            offset
        } else {
            offset + max_align - r
        }
    } else {
        offset
    };
    (layout, total as i64)
}

fn type_expr_to_name(t: &crate::parser::ast::TypeExpr) -> String {
    match &t.kind {
        crate::parser::ast::TypeExprKind::Name(n) => n.clone(),
        crate::parser::ast::TypeExprKind::Ref(inner)
        | crate::parser::ast::TypeExprKind::MutRef(inner) => type_expr_to_name(inner),
        crate::parser::ast::TypeExprKind::Ptr(_) => "ptr".to_string(),
        crate::parser::ast::TypeExprKind::FixedArray(inner, n) => {
            format!("[{};{}]", type_expr_to_name(inner), n)
        }
        _ => "int".to_string(),
    }
}

pub(super) fn ffi_cl_type(name: &str) -> cranelift::prelude::Type {
    use cranelift::prelude::types;
    match name {
        "float" | "f64" => types::F64,
        "f32" => types::F32,
        "i32" | "u32" => types::I32,
        "i16" | "u16" => types::I16,
        "i8" | "u8" | "bool" => types::I8,
        "ptr" => types::I64,
        _ if name.starts_with('[') => types::I64,
        _ => types::I64,
    }
}

impl<'a> CraneliftCodegen<'a, JITModule> {
    pub fn new_jit(
        functions: &'a [MirFunction],
        struct_fields: HashMap<String, Vec<String>>,
        native_lib_paths: &[(
            String,
            String,
            Vec<crate::parser::ast::FfiFnSig>,
            Vec<crate::parser::ast::FfiStructDef>,
            Vec<crate::parser::ast::FfiVarDef>,
        )],
    ) -> Self {
        let mut flag_builder = settings::builder();
        flag_builder.set("use_colocated_libcalls", "false").unwrap();
        flag_builder.set("is_pic", "false").unwrap();
        flag_builder.set("opt_level", "speed").unwrap();
        flag_builder.set("enable_alias_analysis", "true").unwrap();
        flag_builder.set("enable_verifier", "false").unwrap();
        let isa_builder = cranelift_native::builder().unwrap_or_else(|msg| {
            eprintln!("error: host architecture not supported: {msg}");
            process::exit(1);
        });
        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .unwrap_or_else(|msg| {
                eprintln!("error: host architecture not supported: {msg}");
                process::exit(1);
            });

        let mut builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());

        let needed = imports::collect_needed_imports(functions);
        let has_async = functions.iter().any(|f| f.is_async);

        let mut libs: Vec<libloading::Library> = Vec::new();
        let mut native_aliases = std::collections::HashSet::new();
        let mut ffi_entries: Vec<FfiFnEntry> = Vec::new();
        let mut ffi_vararg_ptrs: HashMap<String, *const u8> = HashMap::default();
        let mut c_struct_offsets: HashMap<String, Vec<(String, i32, String, Option<(u8, u8)>)>> =
            HashMap::default();
        let mut c_struct_sizes: HashMap<String, i64> = HashMap::default();
        let mut c_struct_names: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let mut c_struct_destructors: HashMap<String, String> = HashMap::default();
        let has_c_structs = native_lib_paths
            .iter()
            .any(|(_, _, _, structs, _)| !structs.is_empty());
        let mut extern_var_ptrs: HashMap<String, (i64, String, String)> = HashMap::default();

        #[cfg(all(olive_std_linked, target_os = "linux"))]
        {
            unsafe extern "C" {
                fn dlsym(
                    handle: *mut std::ffi::c_void,
                    symbol: *const std::ffi::c_char,
                ) -> *mut std::ffi::c_void;
            }
            for &(jit_name, c_name) in SYMBOL_MAP {
                let is_async_needed = has_async && ASYNC_RUNTIME_SYMS.contains(&jit_name);
                let needed_for_c = (jit_name == "__olive_alloc"
                    || jit_name == "__olive_free_c_struct")
                    && has_c_structs;
                if needed.contains(jit_name) || is_async_needed || needed_for_c {
                    let ptr = unsafe { dlsym(std::ptr::null_mut(), c_name.as_ptr() as *const _) };
                    if !ptr.is_null() {
                        builder.symbol(jit_name, ptr as *const u8);
                    }
                }
            }
        }

        #[cfg(not(all(olive_std_linked, target_os = "linux")))]
        let lib = unsafe {
            let name = libloading::library_filename("olive_std");
            let mut paths = vec![
                std::path::PathBuf::from("target/release").join(&name),
                std::path::PathBuf::from("target/debug").join(&name),
            ];

            if let Ok(exe_path) = std::env::current_exe() {
                if let Some(exe_dir) = exe_path.parent() {
                    paths.push(exe_dir.join(&name));
                    if let Some(parent) = exe_dir.parent() {
                        paths.push(parent.join("lib").join(&name));
                    }
                }
            }

            paths.push(std::path::PathBuf::from("/usr/local/lib").join(&name));
            paths.push(std::path::PathBuf::from("/usr/lib").join(&name));
            paths.push(std::path::PathBuf::from("/lib").join(&name));

            let mut loaded_lib = None;
            for path in paths {
                if let Ok(l) = libloading::Library::new(&path) {
                    loaded_lib = Some(l);
                    break;
                }
            }
            loaded_lib
        };

        #[cfg(not(all(olive_std_linked, target_os = "linux")))]
        if let Some(lib) = lib {
            unsafe {
                for &(jit_name, c_name) in SYMBOL_MAP {
                    let is_async_needed = has_async && ASYNC_RUNTIME_SYMS.contains(&jit_name);
                    let needed_for_c = (jit_name == "__olive_alloc"
                        || jit_name == "__olive_free_c_struct")
                        && has_c_structs;
                    if needed.contains(jit_name) || is_async_needed || needed_for_c {
                        if let Ok(f) = lib.get::<unsafe extern "C" fn()>(c_name) {
                            builder.symbol(jit_name, *f as *const u8);
                        }
                    }
                }
            }
            libs.push(lib);
        }

        for (alias, path, ffi_sigs, ffi_structs, ffi_vars) in native_lib_paths {
            for ffi_struct in ffi_structs {
                let type_name = format!("{}::{}", alias, ffi_struct.name);
                let (layout, total_size) = c_abi_layout(&ffi_struct.fields, ffi_struct.is_union);
                c_struct_offsets.insert(type_name.clone(), layout);
                c_struct_sizes.insert(type_name.clone(), total_size);
                c_struct_names.insert(type_name.clone());
                if let Some(dtor) = &ffi_struct.destructor {
                    let dtor_jit = format!("{}::{}", alias, dtor);
                    c_struct_destructors.insert(type_name, dtor_jit);
                }
            }
            if let Ok(lib) = unsafe { libloading::Library::new(path) } {
                native_aliases.insert(alias.clone());
                for var in ffi_vars {
                    let sym_bytes = format!("{}\0", var.name);
                    if let Ok(sym) =
                        unsafe { lib.get::<*const std::ffi::c_void>(sym_bytes.as_bytes()) }
                    {
                        let addr = *sym as i64;
                        let ty_str = type_expr_to_name(&var.ty);
                        let jit_name = format!("{}::{}", alias, var.name);
                        extern_var_ptrs.insert(jit_name, (addr, ty_str, var.name.clone()));
                    }
                }
                if ffi_sigs.is_empty() {
                    let prefix = format!("{}::", alias);
                    for func in functions {
                        for bb in &func.basic_blocks {
                            for stmt in &bb.statements {
                                if let crate::mir::StatementKind::Assign(
                                    _,
                                    crate::mir::Rvalue::Call {
                                        func:
                                            crate::mir::Operand::Constant(
                                                crate::mir::Constant::Function(name),
                                            ),
                                        ..
                                    },
                                ) = &stmt.kind
                                    && name.starts_with(&prefix)
                                    && !c_struct_names.contains(name.as_str())
                                {
                                    let c_sym = format!("{}\0", &name[prefix.len()..]);
                                    if let Ok(f) = unsafe {
                                        lib.get::<unsafe extern "C" fn()>(c_sym.as_bytes())
                                    } {
                                        builder.symbol(name, *f as *const u8);
                                    }
                                }
                            }
                        }
                    }
                } else {
                    for sig in ffi_sigs {
                        let jit_name = format!("{}::{}", alias, sig.name);
                        let c_sym = format!("{}\0", sig.name);
                        if let Ok(f) =
                            unsafe { lib.get::<unsafe extern "C" fn()>(c_sym.as_bytes()) }
                        {
                            if sig.is_vararg {
                                ffi_vararg_ptrs.insert(jit_name.clone(), *f as *const u8);
                            } else {
                                builder.symbol(&jit_name, *f as *const u8);
                            }
                        }
                        let mut use_sret = false;
                        if let Some(ret_type) = &sig.ret {
                            let ret_name = type_expr_to_name(ret_type);
                            if let Some(&size) = c_struct_sizes.get(&ret_name) {
                                if size > 16 {
                                    use_sret = true;
                                }
                            }
                        }
                        ffi_entries.push(FfiFnEntry {
                            jit_name,
                            c_name: sig.name.clone(),
                            params: sig
                                .params
                                .iter()
                                .map(|p| type_expr_to_name(&p.ty))
                                .collect(),
                            ret: sig.ret.as_ref().map(type_expr_to_name),
                            is_vararg: sig.is_vararg,
                            n_fixed: sig.params.len(),
                            call_conv: sig.call_conv.clone(),
                            use_sret,
                        });
                    }
                }
                libs.push(lib);
            } else {
                eprintln!("warning: could not load native library '{}'", path);
            }
        }

        let module = JITModule::new(builder);

        Self {
            functions,
            module,
            func_ids: HashMap::default(),
            string_ids: HashMap::default(),
            struct_fields,
            _libs: libs,
            native_aliases,
            ffi_entries,
            ffi_vararg_ptrs,
            ffi_vararg_ids: std::collections::HashSet::new(),
            c_struct_offsets,
            c_struct_sizes,
            c_struct_names,
            c_struct_destructors,
            aot: false,
            extern_var_ptrs,
        }
    }

    pub fn finalize(&mut self) {
        self.module.finalize_definitions().unwrap_or_else(|e| {
            eprintln!("error: JIT finalization failed: {e}");
            process::exit(1);
        });
    }

    pub fn get_function(&mut self, name: &str) -> Option<*const u8> {
        let func_id = self.func_ids.get(name)?;
        Some(self.module.get_finalized_function(*func_id))
    }
}

impl<'a> CraneliftCodegen<'a, ObjectModule> {
    pub fn new_aot(
        functions: &'a [MirFunction],
        struct_fields: HashMap<String, Vec<String>>,
        native_lib_paths: &[(
            String,
            String,
            Vec<crate::parser::ast::FfiFnSig>,
            Vec<crate::parser::ast::FfiStructDef>,
            Vec<crate::parser::ast::FfiVarDef>,
        )],
    ) -> Self {
        let mut flag_builder = settings::builder();
        flag_builder.set("use_colocated_libcalls", "false").unwrap();
        flag_builder.set("is_pic", "true").unwrap();
        flag_builder.set("opt_level", "speed").unwrap();
        flag_builder.set("enable_alias_analysis", "true").unwrap();
        flag_builder.set("enable_verifier", "false").unwrap();
        let isa_builder = cranelift_native::builder().unwrap_or_else(|msg| {
            eprintln!("error: host architecture not supported: {msg}");
            process::exit(1);
        });
        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .unwrap_or_else(|msg| {
                eprintln!("error: host architecture not supported: {msg}");
                process::exit(1);
            });

        let obj_builder =
            ObjectBuilder::new(isa, "olive", cranelift_module::default_libcall_names())
                .unwrap_or_else(|e| {
                    eprintln!("error: failed to create object builder: {e}");
                    process::exit(1);
                });
        let module = ObjectModule::new(obj_builder);

        let mut ffi_entries: Vec<FfiFnEntry> = Vec::new();
        let mut c_struct_offsets: HashMap<String, Vec<(String, i32, String, Option<(u8, u8)>)>> =
            HashMap::default();
        let mut c_struct_sizes: HashMap<String, i64> = HashMap::default();
        let mut c_struct_names: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let mut c_struct_destructors: HashMap<String, String> = HashMap::default();

        let mut extern_var_ptrs: HashMap<String, (i64, String, String)> = HashMap::default();

        for (alias, _path, ffi_sigs, ffi_structs, ffi_vars) in native_lib_paths {
            for ffi_struct in ffi_structs {
                let type_name = format!("{}::{}", alias, ffi_struct.name);
                let (layout, total_size) = c_abi_layout(&ffi_struct.fields, ffi_struct.is_union);
                c_struct_offsets.insert(type_name.clone(), layout);
                c_struct_sizes.insert(type_name.clone(), total_size);
                c_struct_names.insert(type_name.clone());
                if let Some(dtor) = &ffi_struct.destructor {
                    let dtor_jit = format!("{}::{}", alias, dtor);
                    c_struct_destructors.insert(type_name, dtor_jit);
                }
            }
            for var in ffi_vars {
                let ty_str = type_expr_to_name(&var.ty);
                let jit_name = format!("{}::{}", alias, var.name);
                extern_var_ptrs.insert(jit_name, (0, ty_str, var.name.clone()));
            }
            for sig in ffi_sigs {
                let mut use_sret = false;
                if let Some(ret_name) = &sig.ret {
                    let ret_ty_name = type_expr_to_name(ret_name);
                    if let Some(&size) = c_struct_sizes.get(&ret_ty_name) {
                        if size > 16 {
                            use_sret = true;
                        }
                    }
                }
                ffi_entries.push(FfiFnEntry {
                    jit_name: format!("{}::{}", alias, sig.name),
                    c_name: sig.name.clone(),
                    params: sig
                        .params
                        .iter()
                        .map(|p| type_expr_to_name(&p.ty))
                        .collect(),
                    ret: sig.ret.as_ref().map(type_expr_to_name),
                    is_vararg: sig.is_vararg,
                    n_fixed: sig.params.len(),
                    call_conv: sig.call_conv.clone(),
                    use_sret,
                });
            }
        }

        Self {
            functions,
            module,
            func_ids: HashMap::default(),
            string_ids: HashMap::default(),
            struct_fields,
            _libs: Vec::new(),
            native_aliases: std::collections::HashSet::new(),
            ffi_entries,
            ffi_vararg_ptrs: HashMap::default(),
            ffi_vararg_ids: std::collections::HashSet::new(),
            c_struct_offsets,
            c_struct_sizes,
            c_struct_names,
            c_struct_destructors,
            aot: true,
            extern_var_ptrs,
        }
    }

    pub fn emit_object(self) -> Vec<u8> {
        self.module.finish().emit().unwrap()
    }
}

impl<'a, M: Module> CraneliftCodegen<'a, M> {
    pub fn generate(&mut self) {
        let needed = imports::collect_needed_imports(self.functions);

        let mk_sig = |params: &[cranelift::prelude::Type], returns: &[cranelift::prelude::Type]| {
            let mut sig = self.module.make_signature();
            for &p in params {
                sig.params.push(AbiParam::new(p));
            }
            for &r in returns {
                sig.returns.push(AbiParam::new(r));
            }
            sig
        };

        let sig_i64_i64 = mk_sig(&[types::I64], &[types::I64]);
        let sig_f64_i64 = mk_sig(&[types::F64], &[types::I64]);
        let sig_i64_f64 = mk_sig(&[types::I64], &[types::F64]);
        let sig_f64_f64 = mk_sig(&[types::F64], &[types::F64]);
        let sig_void_i64 = mk_sig(&[], &[types::I64]);
        let sig_void_f64 = mk_sig(&[], &[types::F64]);
        let sig_f64_void = mk_sig(&[types::F64], &[]);
        let sig_i64_void = mk_sig(&[types::I64], &[]);
        let sig_i64_i64_i64 = mk_sig(&[types::I64, types::I64], &[types::I64]);
        let sig_i64_i64_void = mk_sig(&[types::I64, types::I64], &[]);
        let sig_f64_f64_f64 = mk_sig(&[types::F64, types::F64], &[types::F64]);

        let sig_i64_i64_i64_void = mk_sig(&[types::I64, types::I64, types::I64], &[]);
        let sig_i64_5_i64 = mk_sig(
            &[types::I64, types::I64, types::I64, types::I64, types::I64],
            &[types::I64],
        );

        let import_table: &[(&str, &cranelift::prelude::Signature)] = &[
            ("__olive_print_int", &sig_i64_i64),
            ("__olive_print_str", &sig_i64_i64),
            ("__olive_print_list", &sig_i64_i64),
            ("__olive_print_obj", &sig_i64_i64),
            ("__olive_str", &sig_i64_i64),
            ("__olive_int", &sig_i64_i64),
            ("__olive_bool", &sig_i64_i64),
            ("__olive_str_to_int", &sig_i64_i64),
            ("__olive_copy", &sig_i64_i64),
            ("__olive_list_new", &sig_i64_i64),
            ("__olive_str_len", &sig_i64_i64),
            ("__olive_list_len", &sig_i64_i64),
            ("__olive_print_float", &sig_f64_i64),
            ("__olive_float_to_str", &sig_f64_i64),
            ("__olive_float_to_int", &sig_f64_i64),
            ("__olive_bool_from_float", &sig_f64_i64),
            ("__olive_str_to_float", &sig_i64_f64),
            ("__olive_int_to_float", &sig_i64_f64),
            ("__olive_float", &sig_i64_f64),
            ("__olive_copy_float", &sig_f64_f64),
            ("__olive_obj_new", &sig_void_i64),
            ("__olive_str_concat", &sig_i64_i64_i64),
            ("__olive_list_concat", &sig_i64_i64_i64),
            ("__olive_str_eq", &sig_i64_i64_i64),
            ("__olive_list_get", &sig_i64_i64_i64),
            ("__olive_get_index_any", &sig_i64_i64_i64),
            ("__olive_str_get", &sig_i64_i64_i64),
            ("__olive_cache_get", &sig_i64_i64_i64),
            ("__olive_cache_has", &sig_i64_i64_i64),
            ("__olive_obj_get", &sig_i64_i64_i64),
            ("__olive_memo_get", &sig_i64_i64_i64),
            ("__olive_cache_has_tuple", &sig_i64_i64_i64),
            ("__olive_cache_get_tuple", &sig_i64_i64_i64),
            ("__olive_list_set", &sig_i64_i64_i64_void),
            ("__olive_obj_set", &sig_i64_i64_i64),
            ("__olive_set_index_any", &sig_i64_i64_i64_void),
            ("__olive_cache_set", &sig_i64_i64_i64),
            ("__olive_cache_set_tuple", &sig_i64_i64_i64),
            ("__olive_free", &sig_i64_void),
            ("__olive_free_str", &sig_i64_void),
            ("__olive_free_list", &sig_i64_void),
            ("__olive_free_obj", &sig_i64_void),
            ("__olive_free_any", &sig_i64_void),
            ("__olive_pow", &sig_i64_i64_i64),
            ("__olive_pow_float", &sig_f64_f64_f64),
            ("__olive_in_list", &sig_i64_i64_i64),
            ("__olive_in_obj", &sig_i64_i64_i64),
            ("__olive_list_append", &sig_i64_i64_void),
            ("__olive_set_add", &sig_i64_i64_void),
            ("__olive_set_new", &sig_i64_i64),
            ("__olive_iter", &sig_i64_i64),
            ("__olive_next", &sig_i64_i64),
            ("__olive_has_next", &sig_i64_i64),
            ("__olive_time_now", &sig_void_f64),
            ("__olive_time_sleep", &sig_f64_void),
            ("__olive_enum_new", &sig_i64_i64_i64),
            ("__olive_enum_tag", &sig_i64_i64),
            ("__olive_enum_type_id", &sig_i64_i64),
            ("__olive_enum_get", &sig_i64_i64_i64),
            ("__olive_enum_set", &sig_i64_i64_i64_void),
            ("__olive_free_enum", &sig_i64_void),
            ("__olive_str_char", &sig_i64_i64_i64),
            ("__olive_str_slice", &sig_i64_i64_i64),
            ("__olive_obj_len", &sig_i64_i64),
            ("__olive_make_future", &sig_i64_i64),
            ("__olive_await", &sig_i64_i64),
            ("__olive_spawn_task", &sig_i64_i64),
            ("__olive_free_future", &sig_i64_i64),
            ("__olive_alloc", &sig_i64_i64),
            ("__olive_async_file_read", &sig_i64_i64),
            ("__olive_async_file_write", &sig_i64_i64_i64),
            ("__olive_gather", &sig_i64_i64),
            ("__olive_select", &sig_i64_i64),
            ("__olive_cancel_future", &sig_i64_i64),
            ("__olive_sm_poll", &sig_i64_i64),
            ("__olive_random_seed", &sig_i64_void),
            ("__olive_random_get", &sig_void_f64),
            ("__olive_random_int", &sig_i64_i64_i64),
            ("__olive_math_sin", &sig_f64_f64),
            ("__olive_math_cos", &sig_f64_f64),
            ("__olive_math_tan", &sig_f64_f64),
            ("__olive_math_asin", &sig_f64_f64),
            ("__olive_math_acos", &sig_f64_f64),
            ("__olive_math_atan", &sig_f64_f64),
            ("__olive_math_atan2", &sig_f64_f64_f64),
            ("__olive_math_log", &sig_f64_f64),
            ("__olive_math_log10", &sig_f64_f64),
            ("__olive_math_exp", &sig_f64_f64),
            ("__olive_net_tcp_connect", &sig_i64_i64),
            ("__olive_net_tcp_send", &sig_i64_i64_i64),
            ("__olive_net_tcp_recv", &sig_i64_i64_i64),
            ("__olive_net_tcp_close", &sig_i64_void),
            ("__olive_http_get", &sig_i64_i64),
            ("__olive_http_post", &sig_i64_i64_i64),
            ("__olive_struct_alloc", &sig_i64_i64),
            ("__olive_free_struct", &sig_i64_void),
            ("__olive_free_c_struct", &sig_i64_i64_void),
            ("__olive_vararg_call", &sig_i64_5_i64),
            ("__olive_ffi_errno", &sig_void_i64),
        ];

        let has_async = self.functions.iter().any(|f| f.is_async);
        let has_c_structs = !self.c_struct_sizes.is_empty();
        for &(name, sig) in import_table {
            let always_needed = ASYNC_RUNTIME_SYMS.contains(&name);
            let needed_for_c =
                (name == "__olive_alloc" || name == "__olive_free_c_struct") && has_c_structs;
            if !(needed.contains(name) || always_needed && has_async || needed_for_c) {
                continue;
            }
            let decl_name = if self.aot {
                SYMBOL_MAP
                    .iter()
                    .find(|&&(k, _)| k == name)
                    .map(|&(_, v)| std::str::from_utf8(&v[..v.len() - 1]).unwrap())
                    .unwrap_or(name)
            } else {
                name
            };
            let id = self
                .module
                .declare_function(decl_name, Linkage::Import, sig)
                .unwrap();
            self.func_ids.insert(name.to_string(), id);
        }

        for entry in &self.ffi_entries {
            if entry.is_vararg && !self.aot {
                continue;
            }
            if self.func_ids.contains_key(&entry.jit_name) {
                continue;
            }
            let mut sig = self.module.make_signature();
            sig.call_conv = match entry.call_conv.as_deref() {
                #[cfg(target_os = "windows")]
                Some("stdcall") | Some("fastcall") => {
                    cranelift::prelude::isa::CallConv::WindowsFastcall
                }
                _ => self.module.isa().default_call_conv(),
            };
            for param_name in &entry.params {
                if let Some(layout) = self.c_struct_offsets.get(param_name) {
                    for (_, _, ty_name, _) in layout {
                        sig.params.push(AbiParam::new(ffi_cl_type(ty_name)));
                    }
                } else {
                    sig.params.push(AbiParam::new(ffi_cl_type(param_name)));
                }
            }
            if entry.use_sret {
                sig.params.insert(
                    0,
                    AbiParam::special(
                        self.module.isa().pointer_type(),
                        ArgumentPurpose::StructReturn,
                    ),
                );
            } else if let Some(ret_name) = &entry.ret {
                if ret_name != "void" {
                    sig.returns.push(AbiParam::new(ffi_cl_type(ret_name)));
                }
            } else {
                sig.returns.push(AbiParam::new(types::I64));
            }
            let decl_name = if self.aot {
                &entry.c_name
            } else {
                &entry.jit_name
            };
            if let Ok(id) = self
                .module
                .declare_function(decl_name, Linkage::Import, &sig)
            {
                self.func_ids.insert(entry.jit_name.clone(), id);
                if entry.is_vararg {
                    self.ffi_vararg_ids.insert(entry.jit_name.clone());
                }
            }
        }

        if !self.native_aliases.is_empty() {
            for func in self.functions {
                for bb in &func.basic_blocks {
                    for stmt in &bb.statements {
                        if let StatementKind::Assign(
                            _,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(name)),
                                args,
                            },
                        ) = &stmt.kind
                        {
                            let is_native = self
                                .native_aliases
                                .iter()
                                .any(|alias| name.starts_with(&format!("{}::", alias)));
                            let is_vararg = self.ffi_vararg_ptrs.contains_key(name.as_str());
                            if is_native && !self.func_ids.contains_key(name.as_str()) && !is_vararg
                            {
                                let mut sig = self.module.make_signature();
                                for arg in args {
                                    let ty = match arg {
                                        Operand::Constant(Constant::Float(_)) => types::F64,
                                        Operand::Copy(l) | Operand::Move(l) => {
                                            imports::cl_type(&func.locals[l.0].ty)
                                        }
                                        _ => types::I64,
                                    };
                                    sig.params.push(AbiParam::new(ty));
                                }
                                sig.returns.push(AbiParam::new(types::I64));
                                if let Ok(id) =
                                    self.module.declare_function(name, Linkage::Import, &sig)
                                {
                                    self.func_ids.insert(name.clone(), id);
                                }
                            }
                        }
                    }
                }
            }
        }

        for func in self.functions {
            let mut sig = self.module.make_signature();
            for i in 0..func.arg_count {
                let ty = &func.locals[i + 1].ty;
                sig.params.push(AbiParam::new(imports::cl_type(ty)));
            }
            let ret_ty = &func.locals[0].ty;
            sig.returns.push(AbiParam::new(imports::cl_type(ret_ty)));

            if func.is_async {
                let can_sm = Self::analyze_async_sm(func).is_some();
                if can_sm {
                    let poll_name = format!("{}__sm_poll", func.name);
                    let mut poll_sig = self.module.make_signature();
                    poll_sig.params.push(AbiParam::new(types::I64));
                    poll_sig.returns.push(AbiParam::new(types::I64));
                    let poll_id = self
                        .module
                        .declare_function(&poll_name, Linkage::Local, &poll_sig)
                        .unwrap();
                    self.func_ids.insert(poll_name, poll_id);
                } else {
                    let body_name = format!("{}__async_body", func.name);
                    let body_id = self
                        .module
                        .declare_function(&body_name, Linkage::Local, &sig)
                        .unwrap();
                    self.func_ids.insert(body_name, body_id);
                }
                let wrapper_id = self
                    .module
                    .declare_function(&func.name, Linkage::Export, &sig)
                    .unwrap();
                self.func_ids.insert(func.name.clone(), wrapper_id);
            } else {
                let func_id = self
                    .module
                    .declare_function(&func.name, Linkage::Export, &sig)
                    .unwrap();
                self.func_ids.insert(func.name.clone(), func_id);
            }
        }

        for func in self.functions {
            self.collect_strings(func);
        }

        let func_count = self.functions.len();
        for i in 0..func_count {
            let func = self.functions[i].clone();
            if func.is_async {
                if let Some(await_points) = Self::analyze_async_sm(&func) {
                    self.translate_async_sm_poll(&func, &await_points);
                    self.generate_sm_wrapper(&func);
                } else {
                    let mut body_func = func.clone();
                    body_func.name = format!("{}__async_body", func.name);
                    body_func.is_async = false;
                    self.translate_function(&body_func);
                    self.generate_async_wrapper(&func);
                }
            } else {
                self.translate_function(&func);
            }
        }

        // Emit synthetic getter functions for extern global vars
        let var_entries: Vec<(String, i64, String, String)> = self
            .extern_var_ptrs
            .iter()
            .map(|(name, (addr, ty, c_name))| (name.clone(), *addr, ty.clone(), c_name.clone()))
            .collect();
        for (name, addr, ty_str, c_name) in var_entries {
            if !self.func_ids.contains_key(&name) {
                if self.aot {
                    self.emit_aot_extern_var_getter(&name, &ty_str, &c_name);
                } else {
                    self.emit_extern_var_getter(&name, addr, &ty_str);
                }
            }
        }

        if self.aot {
            self.emit_aot_main();
        }
    }

    fn emit_extern_var_getter(&mut self, name: &str, addr: i64, ty_str: &str) {
        use cranelift::prelude::FunctionBuilderContext;
        let cl_ty = ffi_cl_type(ty_str);
        let mut sig = self.module.make_signature();
        sig.returns.push(AbiParam::new(types::I64));
        let Ok(func_id) = self.module.declare_function(name, Linkage::Local, &sig) else {
            return;
        };
        self.func_ids.insert(name.to_string(), func_id);
        let mut ctx = self.module.make_context();
        ctx.func.signature = sig;
        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let block = builder.create_block();
        builder.switch_to_block(block);
        builder.seal_block(block);
        let addr_val = builder.ins().iconst(types::I64, addr);
        let raw = builder
            .ins()
            .load(cl_ty, cranelift::prelude::MemFlags::trusted(), addr_val, 0);
        let val = if cl_ty != types::I64 {
            if cl_ty.is_float() {
                builder
                    .ins()
                    .bitcast(types::I64, cranelift::prelude::MemFlags::new(), raw)
            } else {
                builder.ins().uextend(types::I64, raw)
            }
        } else {
            raw
        };
        builder.ins().return_(&[val]);
        builder.finalize();
        if self.module.define_function(func_id, &mut ctx).is_err() {
            eprintln!("warning: failed to emit getter for extern var '{}'", name);
        }
    }

    fn emit_aot_extern_var_getter(&mut self, name: &str, ty_str: &str, c_name: &str) {
        use cranelift::prelude::FunctionBuilderContext;
        let cl_ty = ffi_cl_type(ty_str);
        let mut sig = self.module.make_signature();
        sig.returns.push(AbiParam::new(types::I64));
        let Ok(func_id) = self.module.declare_function(name, Linkage::Local, &sig) else {
            return;
        };
        self.func_ids.insert(name.to_string(), func_id);

        let data_id = match self
            .module
            .declare_data(c_name, Linkage::Import, false, false)
        {
            Ok(id) => id,
            Err(_) => return,
        };

        let mut ctx = self.module.make_context();
        ctx.func.signature = sig;
        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let block = builder.create_block();
        builder.switch_to_block(block);
        builder.seal_block(block);

        let sym_val = self.module.declare_data_in_func(data_id, builder.func);
        let addr_val = builder.ins().symbol_value(types::I64, sym_val);

        let raw = builder
            .ins()
            .load(cl_ty, cranelift::prelude::MemFlags::trusted(), addr_val, 0);

        let val = if cl_ty != types::I64 {
            if cl_ty.is_float() {
                builder
                    .ins()
                    .bitcast(types::I64, cranelift::prelude::MemFlags::new(), raw)
            } else {
                builder.ins().uextend(types::I64, raw)
            }
        } else {
            raw
        };
        builder.ins().return_(&[val]);
        builder.finalize();
        if self.module.define_function(func_id, &mut ctx).is_err() {
            eprintln!("warning: failed to emit getter for extern var '{}'", name);
        }
    }

    fn emit_aot_main(&mut self) {
        let Some(&olive_main_id) = self.func_ids.get("__main__") else {
            return;
        };
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I32));
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I32));
        let Ok(func_id) = self.module.declare_function("main", Linkage::Export, &sig) else {
            return;
        };
        let mut ctx = self.module.make_context();
        ctx.func.signature = sig;
        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let block = builder.create_block();
        builder.append_block_params_for_function_params(block);
        builder.switch_to_block(block);
        builder.seal_block(block);
        let local_fn = self
            .module
            .declare_func_in_func(olive_main_id, builder.func);
        builder.ins().call(local_fn, &[]);
        let zero = builder.ins().iconst(types::I32, 0);
        builder.ins().return_(&[zero]);
        builder.finalize();
        self.module.define_function(func_id, &mut ctx).unwrap();
        self.func_ids.insert("main".to_string(), func_id);
    }

    fn intern_attr_string(&mut self, attr: &str) {
        if self.string_ids.contains_key(attr) {
            return;
        }
        let mut data_ctx = DataDescription::new();
        let mut bytes = attr.as_bytes().to_vec();
        bytes.push(0);
        if bytes.len() % 2 != 0 {
            bytes.push(0);
        }
        data_ctx.define(bytes.into_boxed_slice());
        let name = format!("str_{}", self.string_ids.len());
        let id = self
            .module
            .declare_data(&name, Linkage::Export, false, false)
            .unwrap();
        self.module.define_data(id, &data_ctx).unwrap();
        self.string_ids.insert(attr.to_string(), id);
    }

    fn collect_strings(&mut self, func: &MirFunction) {
        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                match &stmt.kind {
                    StatementKind::Assign(_, rval) => {
                        self.collect_strings_in_rvalue(rval);
                    }
                    StatementKind::SetAttr(_, attr, val_op) => {
                        self.intern_attr_string(attr);
                        self.collect_strings_in_operand(val_op);
                    }
                    _ => {}
                }
            }
        }
    }

    fn collect_strings_in_rvalue(&mut self, rval: &Rvalue) {
        match rval {
            Rvalue::Use(op) | Rvalue::UnaryOp(_, op) => {
                self.collect_strings_in_operand(op);
            }
            Rvalue::GetAttr(op, attr) => {
                self.collect_strings_in_operand(op);
                self.intern_attr_string(attr);
            }
            Rvalue::BinaryOp(_, l, r) | Rvalue::GetIndex(l, r) => {
                self.collect_strings_in_operand(l);
                self.collect_strings_in_operand(r);
            }
            Rvalue::Call { func, args } => {
                self.collect_strings_in_operand(func);
                for arg in args {
                    self.collect_strings_in_operand(arg);
                }
            }
            Rvalue::Aggregate(_, ops) => {
                for op in ops {
                    self.collect_strings_in_operand(op);
                }
            }
            _ => {}
        }
    }

    fn collect_strings_in_operand(&mut self, op: &Operand) {
        if let Operand::Constant(Constant::Str(s)) = op
            && !self.string_ids.contains_key(s)
        {
            let mut data_ctx = DataDescription::new();
            let mut bytes = s.as_bytes().to_vec();
            bytes.push(0);
            if bytes.len() % 2 != 0 {
                bytes.push(0);
            }
            data_ctx.define(bytes.into_boxed_slice());

            let name = format!("str_{}", self.string_ids.len());
            let id = self
                .module
                .declare_data(&name, Linkage::Export, false, false)
                .unwrap();
            self.module.define_data(id, &data_ctx).unwrap();
            self.string_ids.insert(s.clone(), id);
        }
    }
}
