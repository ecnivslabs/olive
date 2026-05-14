mod async_sm;
mod imports;
mod translate;

use crate::mir::{Constant, Local, MirFunction, Operand, Rvalue, StatementKind};
use cranelift::prelude::*;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use rustc_hash::FxHashMap as HashMap;

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
    ("__olive_iter", b"olive_iter\0"),
    ("__olive_has_next", b"olive_has_next\0"),
    ("__olive_next", b"olive_next\0"),
    ("__olive_alloc", b"olive_alloc\0"),
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
];
pub(super) const POLL_PENDING: i64 = i64::MIN;

pub(super) struct SmAwaitPoint {
    pub(super) bb_idx: usize,
    pub(super) stmt_idx: usize,
    pub(super) result_local: Local,
    pub(super) sub_future_local: Local,
}

pub struct CraneliftCodegen<'a, M: Module> {
    pub(super) functions: &'a [MirFunction],
    pub(super) module: M,
    pub(super) func_ids: HashMap<String, FuncId>,
    pub(super) string_ids: HashMap<String, DataId>,
    pub(super) struct_fields: HashMap<String, Vec<String>>,
    pub(super) _libs: Vec<libloading::Library>,
    pub(super) native_aliases: std::collections::HashSet<String>,
    pub(super) aot: bool,
}

impl<'a> CraneliftCodegen<'a, JITModule> {
    pub fn new_jit(
        functions: &'a [MirFunction],
        struct_fields: HashMap<String, Vec<String>>,
        native_lib_paths: &[(String, String)],
    ) -> Self {
        let mut flag_builder = settings::builder();
        flag_builder.set("use_colocated_libcalls", "false").unwrap();
        flag_builder.set("is_pic", "false").unwrap();
        flag_builder.set("opt_level", "speed").unwrap();
        flag_builder.set("enable_alias_analysis", "true").unwrap();
        flag_builder.set("enable_verifier", "false").unwrap();
        let isa_builder = cranelift_native::builder().unwrap_or_else(|msg| {
            panic!("host machine is not supported: {}", msg);
        });
        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .map_err(|msg| panic!("host machine is not supported: {}", msg))
            .unwrap();

        let mut builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());

        let needed = imports::collect_needed_imports(functions);
        let has_async = functions.iter().any(|f| f.is_async);

        let mut libs: Vec<libloading::Library> = Vec::new();
        let mut native_aliases = std::collections::HashSet::new();

        #[cfg(all(olive_std_linked, target_os = "linux"))]
        {
            unsafe extern "C" {
                fn dlsym(handle: *mut std::ffi::c_void, symbol: *const std::ffi::c_char) -> *mut std::ffi::c_void;
            }
            for &(jit_name, c_name) in SYMBOL_MAP {
                let is_async_needed = has_async
                    && matches!(
                        jit_name,
                        "__olive_make_future"
                            | "__olive_await"
                            | "__olive_spawn_task"
                            | "__olive_alloc"
                            | "__olive_free_future"
                            | "__olive_sm_poll"
                    );
                if needed.contains(jit_name) || is_async_needed {
                    let ptr = unsafe { dlsym(std::ptr::null_mut(), c_name.as_ptr() as *const _) };
                    if !ptr.is_null() {
                        builder.symbol(jit_name, ptr as *const u8);
                    }
                }
            }
        }

        #[cfg(not(all(olive_std_linked, target_os = "linux")))]
        if let Ok(lib) = unsafe {
            let name = libloading::library_filename("olive_std");
            libloading::Library::new(std::path::Path::new("target/debug").join(&name))
                .or_else(|_| libloading::Library::new(std::path::Path::new("target/release").join(&name)))
        } {
            unsafe {
                for &(jit_name, c_name) in SYMBOL_MAP {
                    let is_async_needed = has_async
                        && matches!(
                            jit_name,
                            "__olive_make_future"
                                | "__olive_await"
                                | "__olive_spawn_task"
                                | "__olive_alloc"
                                | "__olive_free_future"
                                | "__olive_sm_poll"
                        );
                    if needed.contains(jit_name) || is_async_needed {
                        if let Ok(f) = lib.get::<unsafe extern "C" fn()>(c_name) {
                            builder.symbol(jit_name, *f as *const u8);
                        }
                    }
                }
            }
            libs.push(lib);
        }

        for (alias, path) in native_lib_paths {
            if let Ok(lib) = unsafe { libloading::Library::new(path) } {
                native_aliases.insert(alias.clone());
                let prefix = format!("{}::", alias);
                for func in functions {
                    for bb in &func.basic_blocks {
                        for stmt in &bb.statements {
                            if let crate::mir::StatementKind::Assign(
                                _,
                                crate::mir::Rvalue::Call {
                                    func: crate::mir::Operand::Constant(
                                        crate::mir::Constant::Function(name),
                                    ),
                                    ..
                                },
                            ) = &stmt.kind
                            {
                                if name.starts_with(&prefix) {
                                    let mut sym_bytes: Vec<u8> =
                                        name.as_bytes().to_vec();
                                    sym_bytes.push(0);
                                    if let Ok(f) =
                                        unsafe { lib.get::<unsafe extern "C" fn()>(&*sym_bytes) }
                                    {
                                        builder.symbol(name, *f as *const u8);
                                    }
                                }
                            }
                        }
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
            aot: false,
        }
    }

    pub fn finalize(&mut self) {
        self.module.finalize_definitions().unwrap();
    }

    pub fn get_function(&mut self, name: &str) -> Option<*const u8> {
        let func_id = self.func_ids.get(name)?;
        Some(self.module.get_finalized_function(*func_id))
    }
}

impl<'a> CraneliftCodegen<'a, ObjectModule> {
    pub fn new_aot(functions: &'a [MirFunction], struct_fields: HashMap<String, Vec<String>>) -> Self {
        let mut flag_builder = settings::builder();
        flag_builder.set("use_colocated_libcalls", "false").unwrap();
        flag_builder.set("is_pic", "true").unwrap();
        flag_builder.set("opt_level", "speed").unwrap();
        flag_builder.set("enable_alias_analysis", "true").unwrap();
        flag_builder.set("enable_verifier", "false").unwrap();
        let isa_builder = cranelift_native::builder().unwrap_or_else(|msg| {
            panic!("host machine is not supported: {}", msg);
        });
        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .map_err(|msg| panic!("host machine is not supported: {}", msg))
            .unwrap();

        let obj_builder = ObjectBuilder::new(isa, "olive", cranelift_module::default_libcall_names()).unwrap();
        let module = ObjectModule::new(obj_builder);

        Self {
            functions,
            module,
            func_ids: HashMap::default(),
            string_ids: HashMap::default(),
            struct_fields,
            _libs: Vec::new(),
            native_aliases: std::collections::HashSet::new(),
            aot: true,
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
        let sig_i64_i64_i64_i64 = mk_sig(&[types::I64, types::I64, types::I64], &[types::I64]);
        let sig_i64_i64_i64_void = mk_sig(&[types::I64, types::I64, types::I64], &[]);

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
            ("__olive_list_set", &sig_i64_i64_i64_i64),
            ("__olive_obj_set", &sig_i64_i64_i64_i64),
            ("__olive_set_index_any", &sig_i64_i64_i64_i64),
            ("__olive_cache_set", &sig_i64_i64_i64_i64),
            ("__olive_cache_set_tuple", &sig_i64_i64_i64_i64),
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
            ("__olive_enum_new", &sig_i64_i64_i64_i64),
            ("__olive_enum_tag", &sig_i64_i64),
            ("__olive_enum_type_id", &sig_i64_i64),
            ("__olive_enum_get", &sig_i64_i64_i64),
            ("__olive_enum_set", &sig_i64_i64_i64_void),
            ("__olive_free_enum", &sig_i64_void),
            ("__olive_str_char", &sig_i64_i64_i64),
            ("__olive_str_slice", &sig_i64_i64_i64_i64),
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
        ];

        let has_async = self.functions.iter().any(|f| f.is_async);

        for &(name, sig) in import_table {
            let always_needed = matches!(
                name,
                "__olive_make_future"
                    | "__olive_await"
                    | "__olive_spawn_task"
                    | "__olive_alloc"
                    | "__olive_free_future"
                    | "__olive_sm_poll"
            );
            if !(needed.contains(name) || always_needed && has_async) {
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
                            if is_native && !self.func_ids.contains_key(name.as_str()) {
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
                                if let Ok(id) = self.module.declare_function(
                                    name,
                                    Linkage::Import,
                                    &sig,
                                ) {
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
    }

    fn collect_strings(&mut self, func: &MirFunction) {
        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                if let StatementKind::Assign(_, rval) = &stmt.kind {
                    self.collect_strings_in_rvalue(rval);
                }
            }
        }
    }

    fn collect_strings_in_rvalue(&mut self, rval: &Rvalue) {
        match rval {
            Rvalue::Use(op) | Rvalue::UnaryOp(_, op) | Rvalue::GetAttr(op, _) => {
                self.collect_strings_in_operand(op);
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
