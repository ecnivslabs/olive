use crate::mir::{Constant, MirFunction, Operand, Rvalue, StatementKind};
use crate::semantic::types::Type as OliveType;
use cranelift::prelude::types;

pub(super) fn collect_needed_imports(
    functions: &[MirFunction],
) -> std::collections::HashSet<&'static str> {
    let mut needed = std::collections::HashSet::new();
    for func in functions {
        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                match &stmt.kind {
                    StatementKind::Assign(_, rval) => scan_rvalue_imports(func, rval, &mut needed),
                    StatementKind::SetAttr(..) => {
                        needed.insert("__olive_obj_set");
                    }
                    StatementKind::SetIndex(..) => {
                        needed.insert("__olive_list_set");
                        needed.insert("__olive_obj_set");
                        needed.insert("__olive_set_index_any");
                    }
                    StatementKind::Drop(local) => {
                        let ty = &func.locals[local.0].ty;
                        if ty.is_move_type() {
                            match ty {
                                OliveType::Str => {
                                    needed.insert("__olive_free_str");
                                }
                                OliveType::List(_) | OliveType::Tuple(_) | OliveType::Set(_) => {
                                    needed.insert("__olive_free_list");
                                }
                                OliveType::Struct(_, _) => {
                                    needed.insert("__olive_free_struct");
                                    needed.insert("__olive_free_obj");
                                }
                                OliveType::Dict(_, _) => {
                                    needed.insert("__olive_free_obj");
                                }
                                OliveType::Enum(_, _) => {
                                    needed.insert("__olive_free_enum");
                                }
                                _ => {
                                    needed.insert("__olive_free");
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    needed
}

pub(super) fn scan_rvalue_imports(
    func_mir: &MirFunction,
    rval: &Rvalue,
    needed: &mut std::collections::HashSet<&'static str>,
) {
    match rval {
        Rvalue::Call {
            func: Operand::Constant(Constant::Function(name)),
            args,
        } => {
            if let Some(r) = resolve_builtin_import(func_mir, name, args) {
                needed.insert(r);
            }
        }
        Rvalue::Call { .. } => {}
        Rvalue::BinaryOp(op, lhs, _) => {
            use crate::parser::BinOp::*;
            match op {
                Add => {
                    if is_str_op(func_mir, lhs) {
                        needed.insert("__olive_str_concat");
                    } else if is_list_op(func_mir, lhs) {
                        needed.insert("__olive_list_concat");
                    }
                }
                Eq => {
                    if is_str_op(func_mir, lhs) {
                        needed.insert("__olive_str_eq");
                    }
                }
                Pow => {
                    if is_float_op(func_mir, lhs) {
                        needed.insert("__olive_pow_float");
                    } else {
                        needed.insert("__olive_pow");
                    }
                }
                In => {
                    needed.insert("__olive_in_list");
                    needed.insert("__olive_in_obj");
                }
                NotIn => {
                    needed.insert("__olive_in_list");
                    needed.insert("__olive_in_obj");
                }
                _ => {}
            }
        }
        Rvalue::GetAttr(..) => {
            needed.insert("__olive_obj_get");
        }
        Rvalue::GetTag(..) => {
            needed.insert("__olive_enum_tag");
        }
        Rvalue::GetTypeId(..) => {
            needed.insert("__olive_enum_type_id");
        }
        Rvalue::GetIndex(obj, _) => {
            needed.insert("__olive_list_get");
            needed.insert("__olive_obj_get");
            needed.insert("__olive_get_index_any");
            if let Operand::Copy(loc) | Operand::Move(loc) = obj {
                let ty = &func_mir.locals[loc.0].ty;
                if matches!(ty, OliveType::Str) {
                    needed.insert("__olive_str_get");
                } else if matches!(ty, OliveType::Enum(_, _) | OliveType::Union(_)) {
                    needed.insert("__olive_enum_get");
                }
            }
        }
        Rvalue::Aggregate(kind, _) => {
            use crate::mir::ir::AggregateKind;
            match kind {
                AggregateKind::Dict => {
                    needed.insert("__olive_obj_new");
                    needed.insert("__olive_obj_set");
                }
                AggregateKind::Set => {
                    needed.insert("__olive_list_new");
                    needed.insert("__olive_set_add");
                    needed.insert("__olive_set_new");
                }
                AggregateKind::EnumVariant(_, _) => {
                    needed.insert("__olive_enum_new");
                    needed.insert("__olive_enum_set");
                }
                _ => {
                    needed.insert("__olive_list_new");
                    needed.insert("__olive_list_append");
                    needed.insert("__olive_set_index_any");
                }
            }
        }
        _ => {}
    }
}

pub(super) fn resolve_builtin_import(
    func_mir: &MirFunction,
    name: &str,
    args: &[Operand],
) -> Option<&'static str> {
    if name.starts_with("__olive_") {
        return match name {
            "__olive_print_int" => Some("__olive_print_int"),
            "__olive_print_str" => Some("__olive_print_str"),
            "__olive_print_float" => Some("__olive_print_float"),
            "__olive_print_list" => Some("__olive_print_list"),
            "__olive_print_obj" => Some("__olive_print_obj"),
            "__olive_str" => Some("__olive_str"),
            "__olive_int" => Some("__olive_int"),
            "__olive_bool" => Some("__olive_bool"),
            "__olive_float" => Some("__olive_float"),
            "__olive_str_to_int" => Some("__olive_str_to_int"),
            "__olive_str_to_float" => Some("__olive_str_to_float"),
            "__olive_float_to_int" => Some("__olive_float_to_int"),
            "__olive_float_to_str" => Some("__olive_float_to_str"),
            "__olive_int_to_float" => Some("__olive_int_to_float"),
            "__olive_bool_from_float" => Some("__olive_bool_from_float"),
            "__olive_copy" => Some("__olive_copy"),
            "__olive_copy_float" => Some("__olive_copy_float"),
            "__olive_list_new" => Some("__olive_list_new"),
            "__olive_list_get" => Some("__olive_list_get"),
            "__olive_list_set" => Some("__olive_list_set"),
            "__olive_list_append" => Some("__olive_list_append"),
            "__olive_str_len" => Some("__olive_str_len"),
            "__olive_list_len" => Some("__olive_list_len"),
            "__olive_get_index_any" => Some("__olive_get_index_any"),
            "__olive_set_index_any" => Some("__olive_set_index_any"),
            "__olive_free_any" => Some("__olive_free_any"),
            "__olive_str_get" => Some("__olive_str_get"),
            "__olive_str_concat" => Some("__olive_str_concat"),
            "__olive_list_concat" => Some("__olive_list_concat"),
            "__olive_str_eq" => Some("__olive_str_eq"),
            "__olive_obj_new" => Some("__olive_obj_new"),
            "__olive_obj_get" => Some("__olive_obj_get"),
            "__olive_obj_set" => Some("__olive_obj_set"),
            "__olive_pow" => Some("__olive_pow"),
            "__olive_in_list" => Some("__olive_in_list"),
            "__olive_in_obj" => Some("__olive_in_obj"),
            "__olive_set_add" => Some("__olive_set_add"),
            "__olive_set_new" => Some("__olive_set_new"),
            "__olive_free" => Some("__olive_free"),
            "__olive_free_str" => Some("__olive_free_str"),
            "__olive_free_list" => Some("__olive_free_list"),
            "__olive_free_obj" => Some("__olive_free_obj"),
            "__olive_struct_alloc" => Some("__olive_struct_alloc"),
            "__olive_free_struct" => Some("__olive_free_struct"),
            "__olive_cache_get" => Some("__olive_cache_get"),
            "__olive_cache_has" => Some("__olive_cache_has"),
            "__olive_cache_set" => Some("__olive_cache_set"),
            "__olive_cache_has_tuple" => Some("__olive_cache_has_tuple"),
            "__olive_cache_get_tuple" => Some("__olive_cache_get_tuple"),
            "__olive_cache_set_tuple" => Some("__olive_cache_set_tuple"),
            "__olive_memo_get" => Some("__olive_memo_get"),
            "__olive_iter" => Some("__olive_iter"),
            "__olive_next" => Some("__olive_next"),
            "__olive_has_next" => Some("__olive_has_next"),
            "__olive_time_now" => Some("__olive_time_now"),
            "__olive_time_sleep" => Some("__olive_time_sleep"),
            "__olive_enum_new" => Some("__olive_enum_new"),
            "__olive_enum_tag" => Some("__olive_enum_tag"),
            "__olive_enum_type_id" => Some("__olive_enum_type_id"),
            "__olive_enum_get" => Some("__olive_enum_get"),
            "__olive_enum_set" => Some("__olive_enum_set"),
            "__olive_str_char" => Some("__olive_str_char"),
            "__olive_str_slice" => Some("__olive_str_slice"),
            "__olive_make_future" => Some("__olive_make_future"),
            "__olive_await" => Some("__olive_await"),
            "__olive_spawn_task" => Some("__olive_spawn_task"),
            "__olive_free_future" => Some("__olive_free_future"),
            "__olive_alloc" => Some("__olive_alloc"),
            "__olive_async_file_read" => Some("__olive_async_file_read"),
            "__olive_async_file_write" => Some("__olive_async_file_write"),
            "__olive_gather" => Some("__olive_gather"),
            "__olive_select" => Some("__olive_select"),
            "__olive_cancel_future" => Some("__olive_cancel_future"),
            "__olive_sm_poll" => Some("__olive_sm_poll"),
            "__olive_random_seed" => Some("__olive_random_seed"),
            "__olive_random_get" => Some("__olive_random_get"),
            "__olive_random_int" => Some("__olive_random_int"),
            "__olive_math_sin" => Some("__olive_math_sin"),
            "__olive_math_cos" => Some("__olive_math_cos"),
            "__olive_math_tan" => Some("__olive_math_tan"),
            "__olive_math_asin" => Some("__olive_math_asin"),
            "__olive_math_acos" => Some("__olive_math_acos"),
            "__olive_math_atan" => Some("__olive_math_atan"),
            "__olive_math_atan2" => Some("__olive_math_atan2"),
            "__olive_math_log" => Some("__olive_math_log"),
            "__olive_math_log10" => Some("__olive_math_log10"),
            "__olive_math_exp" => Some("__olive_math_exp"),
            "__olive_net_tcp_connect" => Some("__olive_net_tcp_connect"),
            "__olive_net_tcp_send" => Some("__olive_net_tcp_send"),
            "__olive_net_tcp_recv" => Some("__olive_net_tcp_recv"),
            "__olive_net_tcp_close" => Some("__olive_net_tcp_close"),
            "__olive_http_get" => Some("__olive_http_get"),
            "__olive_http_post" => Some("__olive_http_post"),
            "__olive_http_post_json" => Some("__olive_http_post_json"),
            "__olive_http_put" => Some("__olive_http_put"),
            "__olive_http_delete" => Some("__olive_http_delete"),
            "__olive_http_get_status" => Some("__olive_http_get_status"),
            "__olive_http_get_with_headers" => Some("__olive_http_get_with_headers"),
            "__olive_file_read" => Some("__olive_file_read"),
            "__olive_file_write" => Some("__olive_file_write"),
            "__olive_file_append" => Some("__olive_file_append"),
            "__olive_file_exists" => Some("__olive_file_exists"),
            "__olive_file_delete" => Some("__olive_file_delete"),
            "__olive_file_stat" => Some("__olive_file_stat"),
            "__olive_dir_create" => Some("__olive_dir_create"),
            "__olive_dir_list" => Some("__olive_dir_list"),
            "__olive_str_trim" => Some("__olive_str_trim"),
            "__olive_str_trim_start" => Some("__olive_str_trim_start"),
            "__olive_str_trim_end" => Some("__olive_str_trim_end"),
            "__olive_str_upper" => Some("__olive_str_upper"),
            "__olive_str_lower" => Some("__olive_str_lower"),
            "__olive_str_replace" => Some("__olive_str_replace"),
            "__olive_str_find" => Some("__olive_str_find"),
            "__olive_str_contains" => Some("__olive_str_contains"),
            "__olive_str_starts_with" => Some("__olive_str_starts_with"),
            "__olive_str_ends_with" => Some("__olive_str_ends_with"),
            "__olive_str_repeat" => Some("__olive_str_repeat"),
            "__olive_str_split" => Some("__olive_str_split"),
            "__olive_str_join" => Some("__olive_str_join"),
            "__olive_obj_keys" => Some("__olive_obj_keys"),
            "__olive_obj_values" => Some("__olive_obj_values"),
            "__olive_json_parse" => Some("__olive_json_parse"),
            "__olive_json_stringify" => Some("__olive_json_stringify"),
            "__olive_json_stringify_pretty" => Some("__olive_json_stringify_pretty"),
            "__olive_env_get" => Some("__olive_env_get"),
            "__olive_env_set" => Some("__olive_env_set"),
            "__olive_os_args" => Some("__olive_os_args"),
            "__olive_os_exit" => Some("__olive_os_exit"),
            "__olive_os_exec" => Some("__olive_os_exec"),
            "__olive_os_exec_status" => Some("__olive_os_exec_status"),
            "__olive_crypto_sha256" => Some("__olive_crypto_sha256"),
            "__olive_crypto_md5" => Some("__olive_crypto_md5"),
            "__olive_time_format" => Some("__olive_time_format"),
            "__olive_str_fmt" => Some("__olive_str_fmt"),
            "__olive_str_char_count" => Some("__olive_str_char_count"),
            "__olive_file_read_lines" => Some("__olive_file_read_lines"),
            "__olive_file_open" => Some("__olive_file_open"),
            "__olive_file_close" => Some("__olive_file_close"),
            "__olive_file_read_n" => Some("__olive_file_read_n"),
            "__olive_file_write_str" => Some("__olive_file_write_str"),
            "__olive_file_seek" => Some("__olive_file_seek"),
            "__olive_file_tell" => Some("__olive_file_tell"),
            "__olive_file_copy" => Some("__olive_file_copy"),
            "__olive_file_rename" => Some("__olive_file_rename"),
            "__olive_path_join" => Some("__olive_path_join"),
            "__olive_path_dirname" => Some("__olive_path_dirname"),
            "__olive_path_basename" => Some("__olive_path_basename"),
            "__olive_path_ext" => Some("__olive_path_ext"),
            "__olive_path_stem" => Some("__olive_path_stem"),
            "__olive_path_is_absolute" => Some("__olive_path_is_absolute"),
            "__olive_temp_dir" => Some("__olive_temp_dir"),
            "__olive_temp_file" => Some("__olive_temp_file"),
            "__olive_stdin_read" => Some("__olive_stdin_read"),
            "__olive_stdin_read_line" => Some("__olive_stdin_read_line"),
            "__olive_chan_new" => Some("__olive_chan_new"),
            "__olive_chan_send" => Some("__olive_chan_send"),
            "__olive_chan_recv" => Some("__olive_chan_recv"),
            "__olive_chan_try_recv" => Some("__olive_chan_try_recv"),
            "__olive_chan_len" => Some("__olive_chan_len"),
            "__olive_chan_close" => Some("__olive_chan_close"),
            "__olive_chan_free" => Some("__olive_chan_free"),
            "__olive_mutex_new" => Some("__olive_mutex_new"),
            "__olive_mutex_lock" => Some("__olive_mutex_lock"),
            "__olive_mutex_unlock" => Some("__olive_mutex_unlock"),
            "__olive_mutex_free" => Some("__olive_mutex_free"),
            "__olive_atomic_new" => Some("__olive_atomic_new"),
            "__olive_atomic_get" => Some("__olive_atomic_get"),
            "__olive_atomic_set" => Some("__olive_atomic_set"),
            "__olive_atomic_add" => Some("__olive_atomic_add"),
            "__olive_atomic_cas" => Some("__olive_atomic_cas"),
            "__olive_atomic_free" => Some("__olive_atomic_free"),
            "__olive_net_tcp_listen" => Some("__olive_net_tcp_listen"),
            "__olive_net_tcp_accept" => Some("__olive_net_tcp_accept"),
            "__olive_net_tcp_listener_addr" => Some("__olive_net_tcp_listener_addr"),
            "__olive_net_tcp_listener_close" => Some("__olive_net_tcp_listener_close"),
            "__olive_net_tcp_peer_addr" => Some("__olive_net_tcp_peer_addr"),
            "__olive_net_tcp_set_timeout" => Some("__olive_net_tcp_set_timeout"),
            "__olive_net_udp_open" => Some("__olive_net_udp_open"),
            "__olive_net_udp_send" => Some("__olive_net_udp_send"),
            "__olive_net_udp_recv" => Some("__olive_net_udp_recv"),
            "__olive_net_udp_set_timeout" => Some("__olive_net_udp_set_timeout"),
            "__olive_net_udp_close" => Some("__olive_net_udp_close"),
            "__olive_net_dns_lookup" => Some("__olive_net_dns_lookup"),
            "__olive_net_dns_lookup_all" => Some("__olive_net_dns_lookup_all"),
            "__olive_sys_hostname" => Some("__olive_sys_hostname"),
            "__olive_sys_pid" => Some("__olive_sys_pid"),
            "__olive_sys_cpu_count" => Some("__olive_sys_cpu_count"),
            "__olive_sys_platform" => Some("__olive_sys_platform"),
            "__olive_sys_arch" => Some("__olive_sys_arch"),
            "__olive_sys_memory_total" => Some("__olive_sys_memory_total"),
            "__olive_sys_memory_free" => Some("__olive_sys_memory_free"),
            "__olive_sys_uptime" => Some("__olive_sys_uptime"),
            "__olive_sys_username" => Some("__olive_sys_username"),
            "__olive_sys_home_dir" => Some("__olive_sys_home_dir"),
            "__olive_sys_cwd" => Some("__olive_sys_cwd"),
            "__olive_sys_chdir" => Some("__olive_sys_chdir"),
            "__olive_gzip_compress" => Some("__olive_gzip_compress"),
            "__olive_gzip_decompress" => Some("__olive_gzip_decompress"),
            "__olive_zstd_compress" => Some("__olive_zstd_compress"),
            "__olive_zstd_decompress" => Some("__olive_zstd_decompress"),
            "__olive_base64_encode" => Some("__olive_base64_encode"),
            "__olive_base64_decode" => Some("__olive_base64_decode"),
            "__olive_base64_encode_bytes" => Some("__olive_base64_encode_bytes"),
            "__olive_url_encode" => Some("__olive_url_encode"),
            "__olive_url_decode" => Some("__olive_url_decode"),
            "__olive_hex_encode" => Some("__olive_hex_encode"),
            "__olive_hex_decode" => Some("__olive_hex_decode"),
            "__olive_datetime_now" => Some("__olive_datetime_now"),
            "__olive_datetime_utcnow" => Some("__olive_datetime_utcnow"),
            "__olive_datetime_parse" => Some("__olive_datetime_parse"),
            "__olive_datetime_format" => Some("__olive_datetime_format"),
            "__olive_datetime_parts" => Some("__olive_datetime_parts"),
            "__olive_datetime_from_parts" => Some("__olive_datetime_from_parts"),
            "__olive_datetime_local_offset" => Some("__olive_datetime_local_offset"),
            "__olive_datetime_to_local" => Some("__olive_datetime_to_local"),
            "__olive_datetime_from_local" => Some("__olive_datetime_from_local"),
            "__olive_datetime_weekday" => Some("__olive_datetime_weekday"),
            "__olive_datetime_weekday_name" => Some("__olive_datetime_weekday_name"),
            "__olive_datetime_month_name" => Some("__olive_datetime_month_name"),
            "__olive_datetime_add_days" => Some("__olive_datetime_add_days"),
            "__olive_datetime_add_hours" => Some("__olive_datetime_add_hours"),
            "__olive_datetime_add_minutes" => Some("__olive_datetime_add_minutes"),
            "__olive_datetime_add_seconds" => Some("__olive_datetime_add_seconds"),
            "__olive_datetime_add_months" => Some("__olive_datetime_add_months"),
            "__olive_datetime_add_years" => Some("__olive_datetime_add_years"),
            "__olive_datetime_diff_days" => Some("__olive_datetime_diff_days"),
            "__olive_datetime_diff_seconds" => Some("__olive_datetime_diff_seconds"),
            "__olive_datetime_start_of_day" => Some("__olive_datetime_start_of_day"),
            "__olive_datetime_end_of_day" => Some("__olive_datetime_end_of_day"),
            "__olive_datetime_start_of_month" => Some("__olive_datetime_start_of_month"),
            "__olive_datetime_is_leap_year" => Some("__olive_datetime_is_leap_year"),
            "__olive_datetime_days_in_month" => Some("__olive_datetime_days_in_month"),
            "__olive_log_set_level" => Some("__olive_log_set_level"),
            "__olive_log_set_format" => Some("__olive_log_set_format"),
            "__olive_log_debug" => Some("__olive_log_debug"),
            "__olive_log_info" => Some("__olive_log_info"),
            "__olive_log_warn" => Some("__olive_log_warn"),
            "__olive_log_error" => Some("__olive_log_error"),
            "__olive_log_with_field" => Some("__olive_log_with_field"),
            "__olive_log_clear_fields" => Some("__olive_log_clear_fields"),
            "__olive_log_level_from_str" => Some("__olive_log_level_from_str"),
            "__olive_regex_match" => Some("__olive_regex_match"),
            "__olive_regex_find" => Some("__olive_regex_find"),
            "__olive_regex_find_all" => Some("__olive_regex_find_all"),
            "__olive_regex_replace" => Some("__olive_regex_replace"),
            "__olive_regex_replace_all" => Some("__olive_regex_replace_all"),
            "__olive_regex_captures" => Some("__olive_regex_captures"),
            "__olive_regex_split" => Some("__olive_regex_split"),
            "__olive_regex_is_valid" => Some("__olive_regex_is_valid"),
            "__olive_uuid_v4" => Some("__olive_uuid_v4"),
            "__olive_uuid_nil" => Some("__olive_uuid_nil"),
            "__olive_uuid_is_valid" => Some("__olive_uuid_is_valid"),
            "__olive_uuid_to_hex" => Some("__olive_uuid_to_hex"),
            "__olive_crypto_aes_encrypt" => Some("__olive_crypto_aes_encrypt"),
            "__olive_crypto_aes_decrypt" => Some("__olive_crypto_aes_decrypt"),
            "__olive_crypto_argon2_hash" => Some("__olive_crypto_argon2_hash"),
            "__olive_crypto_argon2_verify" => Some("__olive_crypto_argon2_verify"),
            "__olive_crypto_rsa_keygen" => Some("__olive_crypto_rsa_keygen"),
            "__olive_crypto_rsa_encrypt" => Some("__olive_crypto_rsa_encrypt"),
            "__olive_crypto_rsa_decrypt" => Some("__olive_crypto_rsa_decrypt"),
            "__olive_result_ok" => Some("__olive_result_ok"),
            "__olive_result_err" => Some("__olive_result_err"),
            "__olive_result_is_ok" => Some("__olive_result_is_ok"),
            "__olive_result_is_err" => Some("__olive_result_is_err"),
            "__olive_result_unwrap" => Some("__olive_result_unwrap"),
            "__olive_result_unwrap_err" => Some("__olive_result_unwrap_err"),
            "__olive_result_unwrap_or" => Some("__olive_result_unwrap_or"),
            "__olive_result_err_msg" => Some("__olive_result_err_msg"),
            "__olive_buf_new" => Some("__olive_buf_new"),
            "__olive_buf_from_str" => Some("__olive_buf_from_str"),
            "__olive_buf_len" => Some("__olive_buf_len"),
            "__olive_buf_push" => Some("__olive_buf_push"),
            "__olive_buf_get" => Some("__olive_buf_get"),
            "__olive_buf_set" => Some("__olive_buf_set"),
            "__olive_buf_to_str" => Some("__olive_buf_to_str"),
            "__olive_buf_to_hex" => Some("__olive_buf_to_hex"),
            "__olive_buf_concat" => Some("__olive_buf_concat"),
            "__olive_buf_slice" => Some("__olive_buf_slice"),
            "__olive_buf_free" => Some("__olive_buf_free"),
            "__olive_buf_read_u16_le" => Some("__olive_buf_read_u16_le"),
            "__olive_buf_read_u16_be" => Some("__olive_buf_read_u16_be"),
            "__olive_buf_read_u32_le" => Some("__olive_buf_read_u32_le"),
            "__olive_buf_read_u32_be" => Some("__olive_buf_read_u32_be"),
            "__olive_buf_read_u64_le" => Some("__olive_buf_read_u64_le"),
            "__olive_buf_read_u64_be" => Some("__olive_buf_read_u64_be"),
            "__olive_buf_write_u16_le" => Some("__olive_buf_write_u16_le"),
            "__olive_buf_write_u16_be" => Some("__olive_buf_write_u16_be"),
            "__olive_buf_write_u32_le" => Some("__olive_buf_write_u32_le"),
            "__olive_buf_write_u32_be" => Some("__olive_buf_write_u32_be"),
            "__olive_buf_write_u64_le" => Some("__olive_buf_write_u64_le"),
            "__olive_buf_write_u64_be" => Some("__olive_buf_write_u64_be"),
            "__olive_websocket_connect" => Some("__olive_websocket_connect"),
            "__olive_websocket_send" => Some("__olive_websocket_send"),
            "__olive_websocket_send_binary" => Some("__olive_websocket_send_binary"),
            "__olive_websocket_recv" => Some("__olive_websocket_recv"),
            "__olive_websocket_recv_binary" => Some("__olive_websocket_recv_binary"),
            "__olive_websocket_close" => Some("__olive_websocket_close"),
            "__olive_yaml_parse" => Some("__olive_yaml_parse"),
            "__olive_yaml_stringify" => Some("__olive_yaml_stringify"),
            "__olive_toml_parse" => Some("__olive_toml_parse"),
            "__olive_toml_stringify" => Some("__olive_toml_stringify"),
            "__olive_bufread_open" => Some("__olive_bufread_open"),
            "__olive_bufread_line" => Some("__olive_bufread_line"),
            "__olive_bufread_close" => Some("__olive_bufread_close"),
            "__olive_bufwrite_open" => Some("__olive_bufwrite_open"),
            "__olive_bufwrite_write" => Some("__olive_bufwrite_write"),
            "__olive_bufwrite_flush" => Some("__olive_bufwrite_flush"),
            "__olive_bufwrite_close" => Some("__olive_bufwrite_close"),
            "__olive_panic" => Some("__olive_panic"),
            "__olive_atexit" => Some("__olive_atexit"),
            "__olive_run_exit_hooks" => Some("__olive_run_exit_hooks"),
            "__olive_is_null" => Some("__olive_is_null"),
            "__olive_is_str" => Some("__olive_is_str"),
            "__olive_is_list" => Some("__olive_is_list"),
            "__olive_is_obj" => Some("__olive_is_obj"),
            "__olive_is_bytes" => Some("__olive_is_bytes"),
            "__olive_typeof_str" => Some("__olive_typeof_str"),
            "__olive_str_is_ascii" => Some("__olive_str_is_ascii"),
            "__olive_str_grapheme_count" => Some("__olive_str_grapheme_count"),
            "__olive_str_graphemes" => Some("__olive_str_graphemes"),
            "__olive_pool_size" => Some("__olive_pool_size"),
            "__olive_pool_run" => Some("__olive_pool_run"),
            "__olive_pool_run_sync" => Some("__olive_pool_run_sync"),
            _ => None,
        };
    }
    if name == "ffi_errno" {
        return Some("__olive_ffi_errno");
    }
    match name {
        "print" | "str" | "int" | "float" | "bool" | "iter" | "next" | "has_next" | "len"
        | "slice"
            if !args.is_empty() =>
        {
            let arg_type = match &args[0] {
                Operand::Constant(Constant::Str(_)) => OliveType::Str,
                Operand::Constant(Constant::Float(_)) => OliveType::Float,
                Operand::Copy(l) | Operand::Move(l) => func_mir.locals[l.0].ty.clone(),
                _ => OliveType::Int,
            };
            map_builtin_to_runtime(name, &arg_type)
        }

        "list_new" => Some("__olive_list_new"),
        _ => None,
    }
}

pub(super) fn map_builtin_to_runtime(name: &str, arg_ty: &OliveType) -> Option<&'static str> {
    let mut current_ty = arg_ty;
    while let OliveType::Ref(inner) | OliveType::MutRef(inner) = current_ty {
        current_ty = inner;
    }

    match name {
        "len" => match current_ty {
            OliveType::Str => Some("__olive_str_len"),
            OliveType::Dict(_, _) | OliveType::Struct(_, _) | OliveType::Any => {
                Some("__olive_obj_len")
            }
            _ => Some("__olive_list_len"),
        },
        "print" => match current_ty {
            OliveType::Str => Some("__olive_print_str"),
            OliveType::Float => Some("__olive_print_float"),
            OliveType::List(_) | OliveType::Tuple(_) | OliveType::Set(_) => {
                Some("__olive_print_list")
            }
            OliveType::Dict(_, _) | OliveType::Struct(_, _) => Some("__olive_print_obj"),
            _ => Some("__olive_print_int"),
        },
        "str" => match current_ty {
            OliveType::Str => Some("__olive_copy"),
            OliveType::Float => Some("__olive_float_to_str"),
            _ => Some("__olive_str"),
        },
        "int" => match current_ty {
            OliveType::Float => Some("__olive_float_to_int"),
            OliveType::Str => Some("__olive_str_to_int"),
            _ => Some("__olive_int"),
        },
        "float" => match current_ty {
            OliveType::Float => Some("__olive_copy_float"),
            OliveType::Int => Some("__olive_int_to_float"),
            OliveType::Str => Some("__olive_str_to_float"),
            _ => Some("__olive_float"),
        },
        "bool" => {
            if *current_ty == OliveType::Float {
                Some("__olive_bool_from_float")
            } else {
                Some("__olive_bool")
            }
        }
        "iter" => Some("__olive_iter"),
        "next" => Some("__olive_next"),
        "has_next" => Some("__olive_has_next"),
        "slice" => Some("__olive_str_slice"),
        _ => None,
    }
}

pub(super) fn is_u64_op(func_mir: &MirFunction, op: &Operand) -> bool {
    match op {
        Operand::Copy(loc) | Operand::Move(loc) => {
            matches!(func_mir.locals[loc.0].ty, OliveType::U64)
        }
        _ => false,
    }
}

pub(super) fn is_str_op(func_mir: &MirFunction, op: &Operand) -> bool {
    match op {
        Operand::Constant(Constant::Str(_)) => true,
        Operand::Copy(loc) | Operand::Move(loc) => func_mir.locals[loc.0].ty == OliveType::Str,
        _ => false,
    }
}

pub(super) fn is_float_op(func_mir: &MirFunction, op: &Operand) -> bool {
    match op {
        Operand::Constant(Constant::Float(_)) => true,
        Operand::Copy(loc) | Operand::Move(loc) => func_mir.locals[loc.0].ty == OliveType::Float,
        _ => false,
    }
}

pub(super) fn is_list_op(func_mir: &MirFunction, op: &Operand) -> bool {
    match op {
        Operand::Copy(loc) | Operand::Move(loc) => {
            let ty = &func_mir.locals[loc.0].ty;
            matches!(
                ty,
                OliveType::List(_) | OliveType::Tuple(_) | OliveType::Set(_)
            )
        }
        _ => false,
    }
}

pub(super) fn cl_type(ty: &OliveType) -> cranelift::prelude::Type {
    match ty {
        OliveType::Int | OliveType::Bool => types::I64,
        OliveType::Float => types::F64,
        OliveType::Vector(inner, width) => match &**inner {
            OliveType::Int => types::I64.by(*width as u32).expect("invalid vector width"),
            OliveType::Float => types::F64.by(*width as u32).expect("invalid vector width"),
            _ => types::I64,
        },
        _ => types::I64,
    }
}
