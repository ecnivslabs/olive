use super::CraneliftCodegen;
use super::imports;
use crate::mir::{Constant, MirFunction, Operand, Rvalue, StatementKind};
use cranelift::codegen::ir::ArgumentPurpose;
use cranelift::prelude::*;
use cranelift_module::{DataDescription, Linkage, Module};

impl<M: Module> CraneliftCodegen<M> {
    pub fn generate(&mut self) {
        let needed = imports::collect_needed_imports(&self.functions);

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

        let sig_3i64_i64 = mk_sig(&[types::I64, types::I64, types::I64], &[types::I64]);
        let sig_6i64_f64 = mk_sig(&[types::I64; 6], &[types::I64]);
        let sig_f64_f64 = mk_sig(&[types::F64], &[types::F64]);
        let sig_f64_f64_f64 = mk_sig(&[types::F64, types::F64], &[types::F64]);
        let sig_f64_f64_i64 = mk_sig(&[types::F64, types::F64], &[types::I64]);
        let sig_f64_i64 = mk_sig(&[types::F64], &[types::I64]);
        let sig_f64_i64_f64 = mk_sig(&[types::F64, types::I64], &[types::F64]);
        let sig_f64_i64_i64 = mk_sig(&[types::F64, types::I64], &[types::I64]);
        let sig_f64_void = mk_sig(&[types::F64], &[]);
        let sig_i64_5_i64 = mk_sig(
            &[types::I64, types::I64, types::I64, types::I64, types::I64],
            &[types::I64],
        );
        let sig_i64_f64 = mk_sig(&[types::I64], &[types::F64]);
        let sig_i64_f64_i64 = mk_sig(&[types::I64, types::F64], &[types::I64]);
        let sig_i64_i64 = mk_sig(&[types::I64], &[types::I64]);
        let sig_i64_i64_i64 = mk_sig(&[types::I64, types::I64], &[types::I64]);
        let sig_i64_i64_i64_void = mk_sig(&[types::I64, types::I64, types::I64], &[]);
        let sig_i64_i64_void = mk_sig(&[types::I64, types::I64], &[]);
        let sig_i64_void = mk_sig(&[types::I64], &[]);
        let sig_void_f64 = mk_sig(&[], &[types::F64]);
        let sig_void_i64 = mk_sig(&[], &[types::I64]);
        let sig_void_void = mk_sig(&[], &[]);
        let import_table: &[(&str, &cranelift::prelude::Signature)] = &[
            ("__olive_alloc", &sig_i64_i64),
            ("__olive_async_file_read", &sig_i64_i64),
            ("__olive_async_file_write", &sig_i64_i64_i64),
            ("__olive_atexit", &sig_i64_void),
            ("__olive_atomic_add", &sig_i64_i64_i64),
            ("__olive_atomic_cas", &sig_3i64_i64),
            ("__olive_atomic_free", &sig_i64_void),
            ("__olive_atomic_get", &sig_i64_i64),
            ("__olive_atomic_new", &sig_i64_i64),
            ("__olive_atomic_set", &sig_i64_i64_void),
            ("__olive_await", &sig_i64_i64),
            ("__olive_await_future", &sig_i64_i64),
            ("__olive_base64_decode", &sig_i64_i64),
            ("__olive_base64_encode", &sig_i64_i64),
            ("__olive_base64_encode_bytes", &sig_i64_i64_i64),
            ("__olive_bool", &sig_i64_i64),
            ("__olive_bool_from_float", &sig_f64_i64),
            ("__olive_buf_concat", &sig_i64_i64_i64),
            ("__olive_buf_free", &sig_i64_void),
            ("__olive_buf_from_str", &sig_i64_i64),
            ("__olive_buf_get", &sig_i64_i64_i64),
            ("__olive_buf_len", &sig_i64_i64),
            ("__olive_buf_new", &sig_i64_i64),
            ("__olive_buf_push", &sig_i64_i64_void),
            ("__olive_buf_read_u16_be", &sig_i64_i64_i64),
            ("__olive_buf_read_u16_le", &sig_i64_i64_i64),
            ("__olive_buf_read_u32_be", &sig_i64_i64_i64),
            ("__olive_buf_read_u32_le", &sig_i64_i64_i64),
            ("__olive_buf_read_u64_be", &sig_i64_i64_i64),
            ("__olive_buf_read_u64_le", &sig_i64_i64_i64),
            ("__olive_buf_set", &sig_i64_i64_i64_void),
            ("__olive_buf_slice", &sig_3i64_i64),
            ("__olive_buf_to_hex", &sig_i64_i64),
            ("__olive_buf_to_str", &sig_i64_i64),
            ("__olive_buf_write_u16_be", &sig_i64_i64_i64_void),
            ("__olive_buf_write_u16_le", &sig_i64_i64_i64_void),
            ("__olive_buf_write_u32_be", &sig_i64_i64_i64_void),
            ("__olive_buf_write_u32_le", &sig_i64_i64_i64_void),
            ("__olive_buf_write_u64_be", &sig_i64_i64_i64_void),
            ("__olive_buf_write_u64_le", &sig_i64_i64_i64_void),
            ("__olive_bufread_close", &sig_i64_void),
            ("__olive_bufread_line", &sig_i64_i64),
            ("__olive_bufread_open", &sig_i64_i64),
            ("__olive_bufwrite_close", &sig_i64_void),
            ("__olive_bufwrite_flush", &sig_i64_i64),
            ("__olive_bufwrite_open", &sig_i64_i64),
            ("__olive_bufwrite_write", &sig_i64_i64_i64),
            ("__olive_cache_get", &sig_i64_i64_i64),
            ("__olive_cache_get_tuple", &sig_i64_i64_i64),
            ("__olive_cache_has", &sig_i64_i64_i64),
            ("__olive_cache_has_tuple", &sig_i64_i64_i64),
            ("__olive_cache_set", &sig_3i64_i64),
            ("__olive_cache_set_tuple", &sig_3i64_i64),
            ("__olive_cancel_future", &sig_i64_i64),
            ("__olive_chan_close", &sig_i64_void),
            ("__olive_chan_free", &sig_i64_void),
            ("__olive_chan_len", &sig_i64_i64),
            ("__olive_chan_new", &sig_void_i64),
            ("__olive_chan_recv", &sig_i64_i64),
            ("__olive_chan_send", &sig_i64_i64_i64),
            ("__olive_chan_try_recv", &sig_i64_i64),
            ("__olive_copy", &sig_i64_i64),
            ("__olive_copy_float", &sig_f64_f64),
            ("__olive_crypto_aes_decrypt", &sig_i64_i64_i64),
            ("__olive_crypto_aes_encrypt", &sig_i64_i64_i64),
            ("__olive_crypto_argon2_hash", &sig_i64_i64),
            ("__olive_crypto_argon2_verify", &sig_i64_i64_i64),
            ("__olive_crypto_md5", &sig_i64_i64),
            ("__olive_crypto_rsa_decrypt", &sig_i64_i64_i64),
            ("__olive_crypto_rsa_encrypt", &sig_i64_i64_i64),
            ("__olive_crypto_rsa_keygen", &sig_void_i64),
            ("__olive_crypto_sha256", &sig_i64_i64),
            ("__olive_datetime_add_days", &sig_f64_i64_f64),
            ("__olive_datetime_add_hours", &sig_f64_i64_f64),
            ("__olive_datetime_add_minutes", &sig_f64_i64_f64),
            ("__olive_datetime_add_months", &sig_f64_i64_f64),
            ("__olive_datetime_add_seconds", &sig_f64_i64_f64),
            ("__olive_datetime_add_years", &sig_f64_i64_f64),
            ("__olive_datetime_days_in_month", &sig_i64_i64_i64),
            ("__olive_datetime_diff_days", &sig_f64_f64_i64),
            ("__olive_datetime_diff_seconds", &sig_f64_f64_i64),
            ("__olive_datetime_end_of_day", &sig_f64_f64),
            ("__olive_datetime_format", &sig_f64_i64_i64),
            ("__olive_datetime_from_local", &sig_f64_f64),
            ("__olive_datetime_from_parts", &sig_6i64_f64),
            ("__olive_datetime_is_leap_year", &sig_i64_i64),
            ("__olive_datetime_local_offset", &sig_void_i64),
            ("__olive_datetime_month_name", &sig_f64_i64),
            ("__olive_datetime_now", &sig_void_f64),
            ("__olive_datetime_parse", &sig_i64_f64),
            ("__olive_datetime_parts", &sig_f64_i64),
            ("__olive_datetime_start_of_day", &sig_f64_f64),
            ("__olive_datetime_start_of_month", &sig_f64_f64),
            ("__olive_datetime_to_local", &sig_f64_f64),
            ("__olive_datetime_utcnow", &sig_void_f64),
            ("__olive_datetime_weekday", &sig_f64_i64),
            ("__olive_datetime_weekday_name", &sig_f64_i64),
            ("__olive_dict_keys_ffi", &sig_i64_i64),
            ("__olive_dir_create", &sig_i64_i64),
            ("__olive_dir_list", &sig_i64_i64),
            ("__olive_enum_get", &sig_i64_i64_i64),
            ("__olive_enum_new", &sig_3i64_i64),
            ("__olive_enum_set", &sig_i64_i64_i64_void),
            ("__olive_enum_tag", &sig_i64_i64),
            ("__olive_enum_type_id", &sig_i64_i64),
            ("__olive_env_get", &sig_i64_i64),
            ("__olive_env_set", &sig_i64_i64_i64),
            ("__olive_ffi_errno", &sig_void_i64),
            ("__olive_file_append", &sig_i64_i64_i64),
            ("__olive_file_close", &sig_i64_void),
            ("__olive_file_copy", &sig_i64_i64_i64),
            ("__olive_file_delete", &sig_i64_i64),
            ("__olive_file_exists", &sig_i64_i64),
            ("__olive_file_open", &sig_i64_i64_i64),
            ("__olive_file_read", &sig_i64_i64),
            ("__olive_file_read_lines", &sig_i64_i64),
            ("__olive_file_read_n", &sig_i64_i64_i64),
            ("__olive_file_rename", &sig_i64_i64_i64),
            ("__olive_file_seek", &sig_3i64_i64),
            ("__olive_file_stat", &sig_i64_i64),
            ("__olive_file_tell", &sig_i64_i64),
            ("__olive_file_write", &sig_i64_i64_i64),
            ("__olive_file_write_str", &sig_i64_i64_i64),
            ("__olive_float", &sig_i64_f64),
            ("__olive_float_to_int", &sig_f64_i64),
            ("__olive_float_to_str", &sig_f64_i64),
            ("__olive_free", &sig_i64_void),
            ("__olive_free_any", &sig_i64_void),
            ("__olive_free_c_struct", &sig_i64_i64_void),
            ("__olive_free_enum", &sig_i64_void),
            ("__olive_free_future", &sig_i64_i64),
            ("__olive_free_list", &sig_i64_void),
            ("__olive_free_obj", &sig_i64_void),
            ("__olive_free_result", &sig_i64_void),
            ("__olive_free_str", &sig_i64_void),
            ("__olive_free_struct", &sig_i64_void),
            ("__olive_gather", &sig_i64_i64),
            ("__olive_gather_poll", &sig_i64_i64),
            ("__olive_get_index_any", &sig_i64_i64_i64),
            ("__olive_gzip_compress", &sig_i64_i64),
            ("__olive_gzip_decompress", &sig_i64_i64),
            ("__olive_has_next", &sig_i64_i64),
            ("__olive_hex_decode", &sig_i64_i64),
            ("__olive_hex_encode", &sig_i64_i64),
            ("__olive_http_delete", &sig_i64_i64),
            ("__olive_http_get", &sig_i64_i64),
            ("__olive_http_get_status", &sig_i64_i64),
            ("__olive_http_get_with_headers", &sig_i64_i64_i64),
            ("__olive_http_post", &sig_i64_i64_i64),
            ("__olive_http_post_json", &sig_i64_i64_i64),
            ("__olive_http_put", &sig_i64_i64_i64),
            ("__olive_in_list", &sig_i64_i64_i64),
            ("__olive_in_obj", &sig_i64_i64_i64),
            ("__olive_int", &sig_i64_i64),
            ("__olive_int_to_float", &sig_i64_f64),
            ("__olive_is_bytes", &sig_i64_i64),
            ("__olive_is_list", &sig_i64_i64),
            ("__olive_is_null", &sig_i64_i64),
            ("__olive_is_obj", &sig_i64_i64),
            ("__olive_is_str", &sig_i64_i64),
            ("__olive_iter", &sig_i64_i64),
            ("__olive_json_parse", &sig_i64_i64),
            ("__olive_json_stringify", &sig_i64_i64),
            ("__olive_json_stringify_pretty", &sig_i64_i64),
            ("__olive_list_append", &sig_i64_i64_void),
            ("__olive_list_concat", &sig_i64_i64_i64),
            ("__olive_list_get", &sig_i64_i64_i64),
            ("__olive_list_insert", &sig_i64_i64_i64_void),
            ("__olive_list_len", &sig_i64_i64),
            ("__olive_list_new", &sig_i64_i64),
            ("__olive_list_remove", &sig_i64_i64_i64),
            ("__olive_list_set", &sig_i64_i64_i64_void),
            ("__olive_log_clear_fields", &sig_void_void),
            ("__olive_log_debug", &sig_i64_void),
            ("__olive_log_error", &sig_i64_void),
            ("__olive_log_info", &sig_i64_void),
            ("__olive_log_level_from_str", &sig_i64_i64),
            ("__olive_log_set_format", &sig_i64_void),
            ("__olive_log_set_level", &sig_i64_void),
            ("__olive_log_warn", &sig_i64_void),
            ("__olive_log_with_field", &sig_i64_i64_void),
            ("__olive_make_future", &sig_i64_i64),
            ("__olive_math_acos", &sig_f64_f64),
            ("__olive_math_asin", &sig_f64_f64),
            ("__olive_math_atan", &sig_f64_f64),
            ("__olive_math_atan2", &sig_f64_f64_f64),
            ("__olive_math_cos", &sig_f64_f64),
            ("__olive_math_exp", &sig_f64_f64),
            ("__olive_math_log", &sig_f64_f64),
            ("__olive_math_log10", &sig_f64_f64),
            ("__olive_math_sin", &sig_f64_f64),
            ("__olive_math_tan", &sig_f64_f64),
            ("__olive_memo_get", &sig_i64_i64_i64),
            ("__olive_mutex_free", &sig_i64_void),
            ("__olive_mutex_lock", &sig_i64_i64),
            ("__olive_mutex_new", &sig_i64_i64),
            ("__olive_mutex_unlock", &sig_i64_i64_void),
            ("__olive_net_dns_lookup", &sig_i64_i64),
            ("__olive_net_dns_lookup_all", &sig_i64_i64),
            ("__olive_net_tcp_accept", &sig_i64_i64),
            ("__olive_net_tcp_close", &sig_i64_void),
            ("__olive_net_tcp_connect", &sig_i64_i64),
            ("__olive_net_tcp_listen", &sig_i64_i64),
            ("__olive_net_tcp_listener_addr", &sig_i64_i64),
            ("__olive_net_tcp_listener_close", &sig_i64_void),
            ("__olive_net_tcp_peer_addr", &sig_i64_i64),
            ("__olive_net_tcp_recv", &sig_i64_i64_i64),
            ("__olive_net_tcp_send", &sig_i64_i64_i64),
            ("__olive_net_tcp_set_timeout", &sig_i64_f64_i64),
            ("__olive_net_udp_close", &sig_i64_void),
            ("__olive_net_udp_open", &sig_i64_i64),
            ("__olive_net_udp_recv", &sig_i64_i64_i64),
            ("__olive_net_udp_send", &sig_3i64_i64),
            ("__olive_net_udp_set_timeout", &sig_i64_f64_i64),
            ("__olive_next", &sig_i64_i64),
            ("__olive_obj_get", &sig_i64_i64_i64),
            ("__olive_obj_keys", &sig_i64_i64),
            ("__olive_obj_len", &sig_i64_i64),
            ("__olive_obj_new", &sig_void_i64),
            ("__olive_obj_remove", &sig_i64_i64_i64),
            ("__olive_obj_set", &sig_3i64_i64),
            ("__olive_obj_values", &sig_i64_i64),
            ("__olive_os_args", &sig_void_i64),
            ("__olive_os_exec", &sig_i64_i64),
            ("__olive_os_exec_status", &sig_i64_i64),
            ("__olive_os_exit", &sig_i64_void),
            ("__olive_panic", &sig_i64_i64),
            ("__olive_path_basename", &sig_i64_i64),
            ("__olive_path_dirname", &sig_i64_i64),
            ("__olive_path_ext", &sig_i64_i64),
            ("__olive_path_is_absolute", &sig_i64_i64),
            ("__olive_path_join", &sig_i64_i64_i64),
            ("__olive_path_stem", &sig_i64_i64),
            ("__olive_pool_run", &sig_i64_i64_i64),
            ("__olive_pool_run_sync", &sig_i64_i64_i64),
            ("__olive_pool_size", &sig_void_i64),
            ("__olive_pow", &sig_i64_i64_i64),
            ("__olive_pow_float", &sig_f64_f64_f64),
            ("__olive_print", &sig_i64_i64),
            ("__olive_print_float", &sig_f64_i64),
            ("__olive_print_int", &sig_i64_i64),
            ("__olive_print_list", &sig_i64_i64),
            ("__olive_print_list_float", &sig_i64_i64),
            ("__olive_print_obj", &sig_i64_i64),
            ("__olive_print_enum", &sig_i64_i64),
            ("__olive_print_any", &sig_i64_i64),
            ("__olive_print_str", &sig_i64_i64),
            ("__olive_str", &sig_i64_i64),
            ("__olive_py_call", &sig_i64_i64_i64),
            ("__olive_py_call_kw", &sig_3i64_i64),
            ("__olive_py_call_kw_safe", &sig_3i64_i64),
            ("__olive_py_call_safe", &sig_i64_i64_i64),
            ("__olive_py_check_alive", &sig_i64_i64),
            ("__olive_py_conv_to_olive", &sig_i64_i64),
            ("__olive_py_conv_to_py", &sig_i64_i64),
            ("__olive_py_decref", &sig_i64_void),
            ("__olive_py_bitor", &sig_i64_i64_i64),
            ("__olive_py_eq", &sig_i64_i64_i64),
            ("__olive_py_add", &sig_i64_i64_i64),
            ("__olive_py_sub", &sig_i64_i64_i64),
            ("__olive_py_mul", &sig_i64_i64_i64),
            ("__olive_py_div", &sig_i64_i64_i64),
            ("__olive_py_mod", &sig_i64_i64_i64),
            ("__olive_py_pow", &sig_i64_i64_i64),
            ("__olive_py_finalize", &sig_void_void),
            ("__olive_py_from_float", &sig_f64_i64),
            ("__olive_py_from_float_bits", &sig_i64_i64),
            ("__olive_py_from_int", &sig_i64_i64),
            ("__olive_py_from_list", &sig_i64_i64),
            ("__olive_py_from_str", &sig_i64_i64),
            ("__olive_py_getattr", &sig_i64_i64_i64),
            ("__olive_py_getattr_safe", &sig_i64_i64_i64),
            ("__olive_py_getitem", &sig_i64_i64_i64),
            ("__olive_py_getitem_safe", &sig_i64_i64_i64),
            ("__olive_py_import", &sig_i64_i64),
            ("__olive_py_import_safe", &sig_i64_i64),
            ("__olive_py_initialize", &sig_void_void),
            ("__olive_py_is_none", &sig_i64_i64),
            ("__olive_py_is_valid_proxy", &sig_i64_i64),
            ("__olive_py_len", &sig_i64_i64),
            ("__olive_py_none", &sig_void_i64),
            ("__olive_py_setattr", &sig_3i64_i64),
            ("__olive_py_setattr_safe", &sig_3i64_i64),
            ("__olive_py_setitem", &sig_i64_i64_i64_void),
            ("__olive_py_setitem_safe", &sig_3i64_i64),
            ("__olive_py_to_dict", &sig_i64_i64),
            ("__olive_py_to_float", &sig_i64_f64),
            ("__olive_py_to_int", &sig_i64_i64),
            ("__olive_py_to_list", &sig_i64_i64),
            ("__olive_py_to_str", &sig_i64_i64),
            ("__olive_random_get", &sig_void_f64),
            ("__olive_random_int", &sig_i64_i64_i64),
            ("__olive_random_seed", &sig_i64_void),
            ("__olive_regex_captures", &sig_i64_i64_i64),
            ("__olive_regex_find", &sig_i64_i64_i64),
            ("__olive_regex_find_all", &sig_i64_i64_i64),
            ("__olive_regex_is_valid", &sig_i64_i64),
            ("__olive_regex_match", &sig_i64_i64_i64),
            ("__olive_regex_replace", &sig_3i64_i64),
            ("__olive_regex_replace_all", &sig_3i64_i64),
            ("__olive_regex_split", &sig_i64_i64_i64),
            ("__olive_result_err", &sig_i64_i64),
            ("__olive_result_err_msg", &sig_i64_i64),
            ("__olive_result_is_err", &sig_i64_i64),
            ("__olive_result_is_ok", &sig_i64_i64),
            ("__olive_result_ok", &sig_i64_i64),
            ("__olive_result_unwrap", &sig_i64_i64),
            ("__olive_result_unwrap_err", &sig_i64_i64),
            ("__olive_result_unwrap_or", &sig_i64_i64_i64),
            ("__olive_run_exit_hooks", &sig_void_void),
            ("__olive_select", &sig_i64_i64),
            ("__olive_select_poll", &sig_i64_i64),
            ("__olive_set_add", &sig_i64_i64_void),
            ("__olive_set_index_any", &sig_i64_i64_i64_void),
            ("__olive_set_new", &sig_i64_i64),
            ("__olive_sm_poll", &sig_i64_i64),
            ("__olive_spawn_task", &sig_i64_i64),
            ("__olive_stdin_read", &sig_void_i64),
            ("__olive_stdin_read_line", &sig_void_i64),
            ("__olive_str", &sig_i64_i64),
            ("__olive_str_char", &sig_i64_i64_i64),
            ("__olive_str_char_count", &sig_i64_i64),
            ("__olive_str_concat", &sig_i64_i64_i64),
            ("__olive_str_contains", &sig_i64_i64_i64),
            ("__olive_str_ends_with", &sig_i64_i64_i64),
            ("__olive_str_eq", &sig_i64_i64_i64),
            ("__olive_str_find", &sig_i64_i64_i64),
            ("__olive_str_fmt", &sig_i64_i64_i64),
            ("__olive_str_get", &sig_i64_i64_i64),
            ("__olive_str_grapheme_count", &sig_i64_i64),
            ("__olive_str_graphemes", &sig_i64_i64),
            ("__olive_str_is_ascii", &sig_i64_i64),
            ("__olive_str_join", &sig_i64_i64_i64),
            ("__olive_str_len", &sig_i64_i64),
            ("__olive_str_lower", &sig_i64_i64),
            ("__olive_str_repeat", &sig_i64_i64_i64),
            ("__olive_str_replace", &sig_3i64_i64),
            ("__olive_str_slice", &sig_3i64_i64),
            ("__olive_str_split", &sig_i64_i64_i64),
            ("__olive_str_starts_with", &sig_i64_i64_i64),
            ("__olive_str_to_float", &sig_i64_f64),
            ("__olive_str_to_int", &sig_i64_i64),
            ("__olive_str_trim", &sig_i64_i64),
            ("__olive_str_trim_end", &sig_i64_i64),
            ("__olive_str_trim_start", &sig_i64_i64),
            ("__olive_str_upper", &sig_i64_i64),
            ("__olive_struct_alloc", &sig_i64_i64),
            ("__olive_sys_arch", &sig_void_i64),
            ("__olive_sys_chdir", &sig_i64_i64),
            ("__olive_sys_cpu_count", &sig_void_i64),
            ("__olive_sys_cwd", &sig_void_i64),
            ("__olive_sys_home_dir", &sig_void_i64),
            ("__olive_sys_hostname", &sig_void_i64),
            ("__olive_sys_memory_free", &sig_void_i64),
            ("__olive_sys_memory_total", &sig_void_i64),
            ("__olive_sys_pid", &sig_void_i64),
            ("__olive_sys_platform", &sig_void_i64),
            ("__olive_sys_uptime", &sig_void_f64),
            ("__olive_sys_username", &sig_void_i64),
            ("__olive_temp_dir", &sig_void_i64),
            ("__olive_temp_file", &sig_void_i64),
            ("__olive_time_format", &sig_f64_i64_i64),
            ("__olive_time_monotonic", &sig_void_f64),
            ("__olive_time_now", &sig_void_f64),
            ("__olive_time_sleep", &sig_f64_void),
            ("__olive_toml_parse", &sig_i64_i64),
            ("__olive_toml_stringify", &sig_i64_i64),
            ("__olive_typeof_str", &sig_i64_i64),
            ("__olive_url_decode", &sig_i64_i64),
            ("__olive_url_encode", &sig_i64_i64),
            ("__olive_uuid_is_valid", &sig_i64_i64),
            ("__olive_uuid_nil", &sig_void_i64),
            ("__olive_uuid_to_hex", &sig_i64_i64),
            ("__olive_uuid_v4", &sig_void_i64),
            ("__olive_vararg_call", &sig_i64_5_i64),
            ("__olive_websocket_close", &sig_i64_void),
            ("__olive_websocket_connect", &sig_i64_i64),
            ("__olive_websocket_recv", &sig_i64_i64),
            ("__olive_websocket_recv_binary", &sig_i64_i64),
            ("__olive_websocket_send", &sig_i64_i64_i64),
            ("__olive_websocket_send_binary", &sig_i64_i64_i64),
            ("__olive_yaml_parse", &sig_i64_i64),
            ("__olive_yaml_stringify", &sig_i64_i64),
            ("__olive_zstd_compress", &sig_i64_i64),
            ("__olive_zstd_decompress", &sig_i64_i64),
        ];

        let has_async = self.functions.iter().any(|f| f.is_async);
        let has_c_structs = !self.c_struct_sizes.is_empty();
        for &(name, sig) in import_table {
            let always_needed = super::ASYNC_RUNTIME_SYMS.contains(&name);
            let needed_for_c_or_traits = (name == "__olive_alloc")
                && (has_c_structs || !self.vtables.is_empty())
                || (name == "__olive_free_c_struct" && has_c_structs);
            if !(needed.contains(name) || always_needed && has_async || needed_for_c_or_traits) {
                continue;
            }
            let decl_name = if self.aot {
                super::SYMBOL_MAP
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
                #[cfg(not(target_os = "windows"))]
                Some("stdcall") | Some("fastcall") => {
                    eprintln!(
                        "Warning: calling convention '{}' ignored on non-Windows target",
                        entry.call_conv.as_deref().unwrap()
                    );
                    self.module.isa().default_call_conv()
                }
                _ => self.module.isa().default_call_conv(),
            };
            let is_windows = cfg!(target_os = "windows");
            for param_name in &entry.params {
                if let Some(layout) = self.c_struct_offsets.get(param_name) {
                    let size = self.c_struct_sizes.get(param_name).cloned().unwrap_or(8);
                    if is_windows {
                        if size == 1 || size == 2 || size == 4 || size == 8 {
                            let ty = match size {
                                1 => types::I8,
                                2 => types::I16,
                                4 => types::I32,
                                _ => types::I64,
                            };
                            sig.params.push(AbiParam::new(ty));
                        } else {
                            sig.params
                                .push(AbiParam::new(self.module.isa().pointer_type()));
                        }
                    } else {
                        if size <= 8 {
                            let has_float = layout.iter().any(|(_, _, ty_name, _)| {
                                ty_name == "float" || ty_name == "f32" || ty_name == "f64"
                            });
                            let ty = if has_float {
                                if size <= 4 { types::F32 } else { types::F64 }
                            } else {
                                if size <= 1 {
                                    types::I8
                                } else if size <= 2 {
                                    types::I16
                                } else if size <= 4 {
                                    types::I32
                                } else {
                                    types::I64
                                }
                            };
                            sig.params.push(AbiParam::new(ty));
                        } else if size <= 16 {
                            let first_has_float = layout.iter().any(|(_, offset, ty_name, _)| {
                                *offset < 8
                                    && (ty_name == "float" || ty_name == "f32" || ty_name == "f64")
                            });
                            let second_has_float = layout.iter().any(|(_, offset, ty_name, _)| {
                                *offset >= 8
                                    && (ty_name == "float" || ty_name == "f32" || ty_name == "f64")
                            });

                            let first_ty = if first_has_float {
                                types::F64
                            } else {
                                types::I64
                            };
                            let second_ty = if second_has_float {
                                types::F64
                            } else {
                                types::I64
                            };

                            sig.params.push(AbiParam::new(first_ty));
                            sig.params.push(AbiParam::new(second_ty));
                        } else {
                            sig.params
                                .push(AbiParam::new(self.module.isa().pointer_type()));
                        }
                    }
                } else {
                    sig.params
                        .push(AbiParam::new(super::ffi_cl_type(param_name)));
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
                    if let Some(layout) = self.c_struct_offsets.get(ret_name) {
                        let size = self.c_struct_sizes.get(ret_name).cloned().unwrap_or(8);
                        if is_windows {
                            if size == 1 || size == 2 || size == 4 || size == 8 {
                                let ty = match size {
                                    1 => types::I8,
                                    2 => types::I16,
                                    4 => types::I32,
                                    _ => types::I64,
                                };
                                sig.returns.push(AbiParam::new(ty));
                            } else {
                                sig.returns.push(AbiParam::new(types::I64));
                            }
                        } else {
                            if size <= 8 {
                                let has_float = layout.iter().any(|(_, _, ty_name, _)| {
                                    ty_name == "float" || ty_name == "f32" || ty_name == "f64"
                                });
                                let ty = if has_float {
                                    if size <= 4 { types::F32 } else { types::F64 }
                                } else {
                                    if size <= 1 {
                                        types::I8
                                    } else if size <= 2 {
                                        types::I16
                                    } else if size <= 4 {
                                        types::I32
                                    } else {
                                        types::I64
                                    }
                                };
                                sig.returns.push(AbiParam::new(ty));
                            } else if size <= 16 {
                                let first_has_float =
                                    layout.iter().any(|(_, offset, ty_name, _)| {
                                        *offset < 8
                                            && (ty_name == "float"
                                                || ty_name == "f32"
                                                || ty_name == "f64")
                                    });
                                let second_has_float =
                                    layout.iter().any(|(_, offset, ty_name, _)| {
                                        *offset >= 8
                                            && (ty_name == "float"
                                                || ty_name == "f32"
                                                || ty_name == "f64")
                                    });

                                let first_ty = if first_has_float {
                                    types::F64
                                } else {
                                    types::I64
                                };
                                let second_ty = if second_has_float {
                                    types::F64
                                } else {
                                    types::I64
                                };

                                sig.returns.push(AbiParam::new(first_ty));
                                sig.returns.push(AbiParam::new(second_ty));
                            } else {
                                sig.returns.push(AbiParam::new(types::I64));
                            }
                        }
                    } else {
                        sig.returns
                            .push(AbiParam::new(super::ffi_cl_type(ret_name)));
                    }
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
            for func in &self.functions {
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

        for func in &self.functions {
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
                let decl_name = if func.name == "main" {
                    "__olive_user_main"
                } else {
                    &func.name
                };
                let wrapper_id = self
                    .module
                    .declare_function(decl_name, Linkage::Export, &sig)
                    .unwrap();
                self.func_ids.insert(func.name.clone(), wrapper_id);
            } else {
                let decl_name = if func.name == "main" {
                    "__olive_user_main"
                } else {
                    &func.name
                };
                let func_id = self
                    .module
                    .declare_function(decl_name, Linkage::Export, &sig)
                    .unwrap();
                self.func_ids.insert(func.name.clone(), func_id);
            }
        }

        let funcs_for_strings = self.functions.clone();
        for func in &funcs_for_strings {
            self.collect_strings(func);
        }

        self.generate_global_vars();
        self.generate_vtables();

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
        let cl_ty = super::ffi_cl_type(ty_str);
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
        let cl_ty = super::ffi_cl_type(ty_str);
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

    pub(super) fn intern_attr_string(&mut self, attr: &str) {
        if self.string_ids.contains_key(attr) {
            return;
        }
        let mut data_ctx = DataDescription::new();
        let mut bytes = attr.as_bytes().to_vec();
        bytes.push(0);
        if !bytes.len().is_multiple_of(2) {
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

    pub(super) fn collect_strings(&mut self, func: &MirFunction) {
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
                    StatementKind::SetIndex(obj_op, idx_op, val_op) => {
                        self.collect_strings_in_operand(obj_op);
                        self.collect_strings_in_operand(idx_op);
                        self.collect_strings_in_operand(val_op);
                    }
                    _ => {}
                }
            }
        }
    }

    fn collect_strings_in_rvalue(&mut self, rval: &crate::mir::Rvalue) {
        use crate::mir::Rvalue;
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

    fn collect_strings_in_operand(&mut self, op: &crate::mir::Operand) {
        use crate::mir::{Constant, Operand};
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
    fn generate_global_vars(&mut self) {
        let vars = self.global_vars.clone();
        for var_name in vars {
            let mut data_ctx = DataDescription::new();
            data_ctx.define_zeroinit(8);
            let id = self
                .module
                .declare_data(&var_name, Linkage::Export, true, false)
                .unwrap();
            self.module.define_data(id, &data_ctx).unwrap();
        }
    }
    fn generate_vtables(&mut self) {
        let vtables = self.vtables.clone();
        for (vtable_name, methods) in vtables {
            let mut data_ctx = DataDescription::new();
            let bytes = vec![0u8; methods.len() * 8];
            data_ctx.define(bytes.into_boxed_slice());

            for (i, method) in methods.iter().enumerate() {
                if let Some(&func_id) = self.func_ids.get(method) {
                    let local_func = self.module.declare_func_in_data(func_id, &mut data_ctx);
                    data_ctx.write_function_addr((i * 8) as u32, local_func);
                }
            }

            let id = self
                .module
                .declare_data(&vtable_name, Linkage::Export, true, false)
                .unwrap();
            self.module.define_data(id, &data_ctx).unwrap();
        }
    }
}
