mod async_sm;
pub(crate) mod debug_variant;
mod imports;
mod jit_loader;
pub(crate) mod profile;
mod setup;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_extended;
pub(crate) mod tier_up;
mod translate;
mod translate_aggregate;
mod translate_binop;
mod translate_call;
mod translate_rvalue;

use crate::mir::{Local, MirFunction};
use cranelift::prelude::*;
use cranelift_jit::{ArenaMemoryProvider, JITBuilder, JITModule};
use cranelift_module::{DataId, FuncId, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use rustc_hash::FxHashMap as HashMap;
use std::process;

/// Reserves one region for the JIT module's whole life, not per-retier (see `new_jit`).
const JIT_ARENA_SIZE: usize = 128 * 1024 * 1024;

/// Best-effort: reservation failure falls back to the default per-finalize
/// provider (the relocation-panic risk `JIT_ARENA_SIZE` exists to avoid), so
/// it's surfaced instead of silently reintroducing that risk. Returns
/// whether the reservation succeeded, so tests can force the failure path
/// without capturing stderr.
fn reserve_jit_arena(builder: &mut JITBuilder, size: usize) -> bool {
    match ArenaMemoryProvider::new_with_size(size) {
        Ok(arena) => {
            builder.memory_provider(Box::new(arena));
            true
        }
        Err(e) => {
            eprintln!(
                "warning: JIT arena reservation failed ({e}); tier-up retiering may be unstable"
            );
            false
        }
    }
}

pub(super) const KIND_SM_FUTURE: i64 = 5;

/// Site graduated all-int; kept in sync by hand with `std_lib`'s `ANY_SITE_SAMPLE_WINDOW`.
pub(crate) const ANY_SITE_GRADUATED: u8 = 8;

/// Ops with a guarded fast path; `kind_history.rs` and `translate_binop.rs` must agree.
pub(super) fn is_specializable_any_binop(op: &crate::parser::BinOp) -> bool {
    use crate::parser::BinOp::*;
    matches!(
        op,
        Add | Sub | Mul | Div | Mod | Lt | LtEq | Gt | GtEq | Eq | NotEq
    )
}

pub(super) type FfiStructFieldLayout = (String, i32, String, Option<(u8, u8)>);
pub(super) type FfiLibInfo = (
    String,
    String,
    Vec<crate::parser::ast::FfiFnSig>,
    Vec<crate::parser::ast::FfiStructDef>,
    Vec<crate::parser::ast::FfiVarDef>,
);

pub(super) static SYMBOL_MAP: &[(&str, &[u8])] = &[
    ("__olive_alloc", b"olive_alloc\0"),
    ("__olive_async_file_read", b"olive_async_file_read\0"),
    ("__olive_async_file_write", b"olive_async_file_write\0"),
    ("__olive_atexit", b"olive_atexit\0"),
    ("__olive_atomic_add", b"olive_atomic_add\0"),
    ("__olive_atomic_cas", b"olive_atomic_cas\0"),
    ("__olive_atomic_free", b"olive_atomic_free\0"),
    ("__olive_atomic_get", b"olive_atomic_get\0"),
    ("__olive_atomic_new", b"olive_atomic_new\0"),
    ("__olive_atomic_set", b"olive_atomic_set\0"),
    ("__olive_await", b"olive_await_future\0"),
    ("__olive_base64_decode", b"olive_base64_decode\0"),
    ("__olive_base64_encode", b"olive_base64_encode\0"),
    (
        "__olive_base64_encode_bytes",
        b"olive_base64_encode_bytes\0",
    ),
    ("__olive_bool", b"olive_bool\0"),
    ("__olive_bool_from_float", b"olive_bool_from_float\0"),
    ("__olive_buf_concat", b"olive_buf_concat\0"),
    ("__olive_buf_free", b"olive_buf_free\0"),
    ("__olive_buf_from_str", b"olive_buf_from_str\0"),
    ("__olive_buf_get", b"olive_buf_get\0"),
    ("__olive_buf_len", b"olive_buf_len\0"),
    ("__olive_buf_new", b"olive_buf_new\0"),
    ("__olive_buf_new_zeroed", b"olive_buf_new_zeroed\0"),
    ("__olive_buf_push", b"olive_buf_push\0"),
    ("__olive_buf_push_u16_le", b"olive_buf_push_u16_le\0"),
    ("__olive_buf_push_u32_le", b"olive_buf_push_u32_le\0"),
    ("__olive_buf_read_u16_be", b"olive_buf_read_u16_be\0"),
    ("__olive_buf_read_u16_le", b"olive_buf_read_u16_le\0"),
    ("__olive_buf_read_u32_be", b"olive_buf_read_u32_be\0"),
    ("__olive_buf_read_u32_le", b"olive_buf_read_u32_le\0"),
    ("__olive_buf_read_u64_be", b"olive_buf_read_u64_be\0"),
    ("__olive_buf_read_u64_le", b"olive_buf_read_u64_le\0"),
    ("__olive_buf_set", b"olive_buf_set\0"),
    ("__olive_buf_slice", b"olive_buf_slice\0"),
    ("__olive_buf_getslice", b"olive_buf_getslice\0"),
    ("__olive_buf_to_hex", b"olive_buf_to_hex\0"),
    ("__olive_buf_to_str", b"olive_buf_to_str\0"),
    ("__olive_buf_write_u16_be", b"olive_buf_write_u16_be\0"),
    ("__olive_buf_write_u16_le", b"olive_buf_write_u16_le\0"),
    ("__olive_buf_write_u32_be", b"olive_buf_write_u32_be\0"),
    ("__olive_buf_write_u32_le", b"olive_buf_write_u32_le\0"),
    ("__olive_buf_write_u64_be", b"olive_buf_write_u64_be\0"),
    ("__olive_buf_write_u64_le", b"olive_buf_write_u64_le\0"),
    ("__olive_bufread_close", b"olive_bufread_close\0"),
    ("__olive_bufread_line", b"olive_bufread_line\0"),
    ("__olive_bufread_open", b"olive_bufread_open\0"),
    ("__olive_bufwrite_close", b"olive_bufwrite_close\0"),
    ("__olive_bufwrite_flush", b"olive_bufwrite_flush\0"),
    ("__olive_bufwrite_open", b"olive_bufwrite_open\0"),
    ("__olive_bufwrite_write", b"olive_bufwrite_write\0"),
    ("__olive_cache_get", b"olive_cache_get\0"),
    ("__olive_cache_get_tuple", b"olive_cache_get_tuple\0"),
    ("__olive_cache_has", b"olive_cache_has\0"),
    ("__olive_cache_has_tuple", b"olive_cache_has_tuple\0"),
    ("__olive_cache_set", b"olive_cache_set\0"),
    ("__olive_cache_set_tuple", b"olive_cache_set_tuple\0"),
    ("__olive_cancel_future", b"olive_cancel_future\0"),
    ("__olive_chan_close", b"olive_chan_close\0"),
    ("__olive_chan_free", b"olive_chan_free\0"),
    ("__olive_chan_len", b"olive_chan_len\0"),
    ("__olive_chan_new", b"olive_chan_new\0"),
    ("__olive_chan_recv", b"olive_chan_recv\0"),
    ("__olive_chan_send", b"olive_chan_send\0"),
    ("__olive_chan_try_recv", b"olive_chan_try_recv\0"),
    ("__olive_copy", b"olive_copy\0"),
    ("__olive_copy_float", b"olive_copy_float\0"),
    ("__olive_crypto_aes_decrypt", b"olive_crypto_aes_decrypt\0"),
    ("__olive_crypto_aes_encrypt", b"olive_crypto_aes_encrypt\0"),
    ("__olive_crypto_argon2_hash", b"olive_crypto_argon2_hash\0"),
    (
        "__olive_crypto_argon2_verify",
        b"olive_crypto_argon2_verify\0",
    ),
    ("__olive_crypto_md5", b"olive_crypto_md5\0"),
    ("__olive_crypto_rsa_decrypt", b"olive_crypto_rsa_decrypt\0"),
    ("__olive_crypto_rsa_encrypt", b"olive_crypto_rsa_encrypt\0"),
    ("__olive_crypto_rsa_keygen", b"olive_crypto_rsa_keygen\0"),
    ("__olive_crypto_sha256", b"olive_crypto_sha256\0"),
    ("__olive_datetime_add_days", b"olive_datetime_add_days\0"),
    ("__olive_datetime_add_hours", b"olive_datetime_add_hours\0"),
    (
        "__olive_datetime_add_minutes",
        b"olive_datetime_add_minutes\0",
    ),
    (
        "__olive_datetime_add_months",
        b"olive_datetime_add_months\0",
    ),
    (
        "__olive_datetime_add_seconds",
        b"olive_datetime_add_seconds\0",
    ),
    ("__olive_datetime_add_years", b"olive_datetime_add_years\0"),
    (
        "__olive_datetime_days_in_month",
        b"olive_datetime_days_in_month\0",
    ),
    ("__olive_datetime_diff_days", b"olive_datetime_diff_days\0"),
    (
        "__olive_datetime_diff_seconds",
        b"olive_datetime_diff_seconds\0",
    ),
    (
        "__olive_datetime_end_of_day",
        b"olive_datetime_end_of_day\0",
    ),
    ("__olive_datetime_format", b"olive_datetime_format\0"),
    (
        "__olive_datetime_from_local",
        b"olive_datetime_from_local\0",
    ),
    (
        "__olive_datetime_from_parts",
        b"olive_datetime_from_parts\0",
    ),
    (
        "__olive_datetime_is_leap_year",
        b"olive_datetime_is_leap_year\0",
    ),
    (
        "__olive_datetime_local_offset",
        b"olive_datetime_local_offset\0",
    ),
    (
        "__olive_datetime_month_name",
        b"olive_datetime_month_name\0",
    ),
    ("__olive_datetime_now", b"olive_datetime_now\0"),
    ("__olive_datetime_parse", b"olive_datetime_parse\0"),
    ("__olive_datetime_parts", b"olive_datetime_parts\0"),
    (
        "__olive_datetime_start_of_day",
        b"olive_datetime_start_of_day\0",
    ),
    (
        "__olive_datetime_start_of_month",
        b"olive_datetime_start_of_month\0",
    ),
    ("__olive_datetime_to_local", b"olive_datetime_to_local\0"),
    ("__olive_datetime_utcnow", b"olive_datetime_utcnow\0"),
    ("__olive_datetime_weekday", b"olive_datetime_weekday\0"),
    (
        "__olive_datetime_weekday_name",
        b"olive_datetime_weekday_name\0",
    ),
    ("__olive_dir_create", b"olive_dir_create\0"),
    ("__olive_dir_list", b"olive_dir_list\0"),
    ("__olive_enum_get", b"olive_enum_get\0"),
    ("__olive_enum_new", b"olive_enum_new\0"),
    ("__olive_enum_set", b"olive_enum_set\0"),
    ("__olive_enum_tag", b"olive_enum_tag\0"),
    ("__olive_enum_type_id", b"olive_enum_type_id\0"),
    ("__olive_env_get", b"olive_env_get\0"),
    ("__olive_env_set", b"olive_env_set\0"),
    ("__olive_ffi_errno", b"olive_ffi_errno\0"),
    ("__olive_ffi_snapshot_errno", b"olive_ffi_snapshot_errno\0"),
    ("__olive_ffi_clear_errno", b"olive_ffi_clear_errno\0"),
    ("__olive_ffi_errmsg", b"olive_ffi_errmsg\0"),
    ("__olive_file_append", b"olive_file_append\0"),
    ("__olive_file_close", b"olive_file_close\0"),
    ("__olive_file_copy", b"olive_file_copy\0"),
    ("__olive_file_delete", b"olive_file_delete\0"),
    ("__olive_file_exists", b"olive_file_exists\0"),
    ("__olive_file_open", b"olive_file_open\0"),
    ("__olive_file_read", b"olive_file_read\0"),
    ("__olive_file_read_lines", b"olive_file_read_lines\0"),
    ("__olive_file_read_n", b"olive_file_read_n\0"),
    ("__olive_file_rename", b"olive_file_rename\0"),
    ("__olive_file_seek", b"olive_file_seek\0"),
    ("__olive_file_stat", b"olive_file_stat\0"),
    ("__olive_file_tell", b"olive_file_tell\0"),
    ("__olive_file_write", b"olive_file_write\0"),
    ("__olive_file_write_str", b"olive_file_write_str\0"),
    ("__olive_float", b"olive_float\0"),
    ("__olive_fatptr_alloc", b"olive_fatptr_alloc\0"),
    ("__olive_float_to_int", b"olive_float_to_int\0"),
    ("__olive_float_to_str", b"olive_float_to_str\0"),
    ("__olive_free", b"olive_free_any\0"),
    ("__olive_free_fatptr", b"olive_free_fatptr\0"),
    ("__olive_free_any", b"olive_free_any\0"),
    ("__olive_free_union_member", b"olive_free_union_member\0"),
    ("__olive_free_c_struct", b"olive_free_c_struct\0"),
    ("__olive_free_enum", b"olive_free_enum\0"),
    ("__olive_free_future", b"olive_free_future\0"),
    ("__olive_free_iter", b"olive_free_iter\0"),
    ("__olive_free_list", b"olive_free_list\0"),
    ("__olive_free_obj", b"olive_free_obj\0"),
    ("__olive_free_str", b"olive_free_str\0"),
    ("__olive_free_struct", b"olive_free_struct\0"),
    ("__olive_free_typed", b"olive_free_typed\0"),
    ("__olive_copy_typed", b"olive_copy_typed\0"),
    ("__olive_relocate_typed", b"olive_relocate_typed\0"),
    ("__olive_eq_typed", b"olive_eq_typed\0"),
    ("__olive_obj_set_typed", b"olive_obj_set_typed\0"),
    ("__olive_obj_get_typed", b"olive_obj_get_typed\0"),
    (
        "__olive_obj_get_checked_typed",
        b"olive_obj_get_checked_typed\0",
    ),
    (
        "__olive_obj_get_default_typed",
        b"olive_obj_get_default_typed\0",
    ),
    ("__olive_set_add_typed", b"olive_set_add_typed\0"),
    ("__olive_set_contains_typed", b"olive_set_contains_typed\0"),
    ("__olive_set_remove_typed", b"olive_set_remove_typed\0"),
    ("__olive_set_remove_checked", b"olive_set_remove_checked\0"),
    (
        "__olive_set_remove_checked_typed",
        b"olive_set_remove_checked_typed\0",
    ),
    ("__olive_set_clear", b"olive_set_clear\0"),
    ("__olive_list_count_typed", b"olive_list_count_typed\0"),
    ("__olive_list_index_typed", b"olive_list_index_typed\0"),
    ("__olive_list_clear", b"olive_list_clear\0"),
    ("__olive_obj_pop_checked", b"olive_obj_pop_checked\0"),
    (
        "__olive_obj_pop_checked_typed",
        b"olive_obj_pop_checked_typed\0",
    ),
    ("__olive_obj_pop_default", b"olive_obj_pop_default\0"),
    (
        "__olive_obj_pop_default_typed",
        b"olive_obj_pop_default_typed\0",
    ),
    ("__olive_obj_setdefault", b"olive_obj_setdefault\0"),
    (
        "__olive_obj_setdefault_typed",
        b"olive_obj_setdefault_typed\0",
    ),
    ("__olive_obj_update", b"olive_obj_update\0"),
    ("__olive_obj_update_typed", b"olive_obj_update_typed\0"),
    ("__olive_obj_clear", b"olive_obj_clear\0"),
    ("__olive_in_obj_typed", b"olive_in_obj_typed\0"),
    ("__olive_in_list_typed", b"olive_in_list_typed\0"),
    ("__olive_stale_ref_fail", b"olive_stale_ref_fail\0"),
    ("__olive_str_gen_of", b"olive_str_gen_of\0"),
    ("__olive_str_gen_stale", b"olive_str_gen_stale\0"),
    ("__olive_struct_gen_of", b"olive_struct_gen_of\0"),
    ("__olive_struct_gen_stale", b"olive_struct_gen_stale\0"),
    ("__olive_gather", b"olive_gather\0"),
    ("__olive_get_index_any", b"olive_get_index_any\0"),
    ("__olive_gzip_compress", b"olive_gzip_compress\0"),
    ("__olive_gzip_decompress", b"olive_gzip_decompress\0"),
    ("__olive_has_next", b"olive_has_next\0"),
    ("__olive_hex_decode", b"olive_hex_decode\0"),
    ("__olive_hex_encode", b"olive_hex_encode\0"),
    ("__olive_http_delete", b"olive_http_delete\0"),
    ("__olive_http_get", b"olive_http_get\0"),
    ("__olive_http_get_status", b"olive_http_get_status\0"),
    (
        "__olive_http_get_with_headers",
        b"olive_http_get_with_headers\0",
    ),
    ("__olive_http_post", b"olive_http_post\0"),
    ("__olive_http_post_json", b"olive_http_post_json\0"),
    ("__olive_http_put", b"olive_http_put\0"),
    ("__olive_in_list", b"olive_in_list\0"),
    ("__olive_in_obj", b"olive_in_obj\0"),
    ("__olive_int", b"olive_int\0"),
    ("__olive_int_to_float", b"olive_int_to_float\0"),
    ("__olive_int_abs", b"olive_int_abs\0"),
    ("__olive_input", b"olive_input\0"),
    ("__olive_is_bytes", b"olive_is_bytes\0"),
    ("__olive_is_list", b"olive_is_list\0"),
    ("__olive_is_null", b"olive_is_null\0"),
    ("__olive_is_obj", b"olive_is_obj\0"),
    ("__olive_is_str", b"olive_is_str\0"),
    ("__olive_iter", b"olive_iter\0"),
    ("__olive_json_parse", b"olive_json_parse\0"),
    ("__olive_json_stringify", b"olive_json_stringify\0"),
    (
        "__olive_json_stringify_pretty",
        b"olive_json_stringify_pretty\0",
    ),
    ("__olive_list_any_bool", b"olive_list_any_bool\0"),
    ("__olive_list_all_bool", b"olive_list_all_bool\0"),
    ("__olive_list_any_any", b"olive_list_any_any\0"),
    ("__olive_list_all_any", b"olive_list_all_any\0"),
    ("__olive_list_append", b"olive_list_append\0"),
    ("__olive_list_concat", b"olive_list_concat\0"),
    ("__olive_list_concat_typed", b"olive_list_concat_typed\0"),
    (
        "__olive_list_getslice_typed",
        b"olive_list_getslice_typed\0",
    ),
    ("__olive_list_extend_typed", b"olive_list_extend_typed\0"),
    ("__olive_list_getslice", b"olive_list_getslice\0"),
    ("__olive_list_sum_int", b"olive_list_sum_int\0"),
    ("__olive_list_sum_float", b"olive_list_sum_float\0"),
    ("__olive_list_min_int", b"olive_list_min_int\0"),
    ("__olive_list_min_float", b"olive_list_min_float\0"),
    ("__olive_list_max_int", b"olive_list_max_int\0"),
    ("__olive_list_max_float", b"olive_list_max_float\0"),
    ("__olive_list_extend", b"olive_list_extend\0"),
    ("__olive_list_insert", b"olive_list_insert\0"),
    ("__olive_list_remove", b"olive_list_remove\0"),
    ("__olive_list_pop", b"olive_list_pop\0"),
    ("__olive_list_reverse", b"olive_list_reverse\0"),
    ("__olive_list_sort_int", b"olive_list_sort_int\0"),
    ("__olive_list_sort_float", b"olive_list_sort_float\0"),
    ("__olive_list_sort_str", b"olive_list_sort_str\0"),
    ("__olive_list_sort_by_keys", b"olive_list_sort_by_keys\0"),
    ("__olive_list_apply_order", b"olive_list_apply_order\0"),
    ("__olive_list_get", b"olive_list_get\0"),
    ("__olive_list_len", b"olive_list_len\0"),
    ("__olive_list_new", b"olive_list_new\0"),
    ("__olive_range_list", b"olive_range_list\0"),
    ("__olive_list_set", b"olive_list_set\0"),
    ("__olive_log_clear_fields", b"olive_log_clear_fields\0"),
    ("__olive_log_debug", b"olive_log_debug\0"),
    ("__olive_log_error", b"olive_log_error\0"),
    ("__olive_log_info", b"olive_log_info\0"),
    ("__olive_log_level_from_str", b"olive_log_level_from_str\0"),
    ("__olive_log_set_format", b"olive_log_set_format\0"),
    ("__olive_log_set_level", b"olive_log_set_level\0"),
    ("__olive_log_warn", b"olive_log_warn\0"),
    ("__olive_log_with_field", b"olive_log_with_field\0"),
    ("__olive_make_future", b"olive_make_future\0"),
    ("__olive_math_abs", b"olive_math_abs\0"),
    ("__olive_math_acos", b"olive_math_acos\0"),
    ("__olive_math_asin", b"olive_math_asin\0"),
    ("__olive_math_atan", b"olive_math_atan\0"),
    ("__olive_math_atan2", b"olive_math_atan2\0"),
    ("__olive_math_cos", b"olive_math_cos\0"),
    ("__olive_math_exp", b"olive_math_exp\0"),
    ("__olive_math_log", b"olive_math_log\0"),
    ("__olive_math_log10", b"olive_math_log10\0"),
    ("__olive_math_round_to_int", b"olive_math_round_to_int\0"),
    (
        "__olive_math_round_with_digits",
        b"olive_math_round_with_digits\0",
    ),
    ("__olive_math_sin", b"olive_math_sin\0"),
    ("__olive_math_tan", b"olive_math_tan\0"),
    ("__olive_memo_get", b"olive_memo_get\0"),
    ("__olive_mutex_free", b"olive_mutex_free\0"),
    ("__olive_mutex_lock", b"olive_mutex_lock\0"),
    ("__olive_mutex_new", b"olive_mutex_new\0"),
    ("__olive_mutex_unlock", b"olive_mutex_unlock\0"),
    ("__olive_net_dns_lookup", b"olive_net_dns_lookup\0"),
    ("__olive_net_dns_lookup_all", b"olive_net_dns_lookup_all\0"),
    ("__olive_net_tcp_accept", b"olive_net_tcp_accept\0"),
    ("__olive_net_tcp_close", b"olive_net_tcp_close\0"),
    ("__olive_net_tcp_connect", b"olive_net_tcp_connect\0"),
    ("__olive_net_tcp_listen", b"olive_net_tcp_listen\0"),
    (
        "__olive_net_tcp_listener_addr",
        b"olive_net_tcp_listener_addr\0",
    ),
    (
        "__olive_net_tcp_listener_close",
        b"olive_net_tcp_listener_close\0",
    ),
    ("__olive_net_tcp_peer_addr", b"olive_net_tcp_peer_addr\0"),
    ("__olive_net_tcp_recv", b"olive_net_tcp_recv\0"),
    ("__olive_net_tcp_send", b"olive_net_tcp_send\0"),
    (
        "__olive_net_tcp_set_timeout",
        b"olive_net_tcp_set_timeout\0",
    ),
    ("__olive_net_udp_close", b"olive_net_udp_close\0"),
    ("__olive_net_udp_open", b"olive_net_udp_open\0"),
    ("__olive_net_udp_recv", b"olive_net_udp_recv\0"),
    ("__olive_net_udp_send", b"olive_net_udp_send\0"),
    (
        "__olive_net_udp_set_timeout",
        b"olive_net_udp_set_timeout\0",
    ),
    ("__olive_next", b"olive_next\0"),
    ("__olive_obj_get", b"olive_obj_get\0"),
    ("__olive_obj_get_checked", b"olive_obj_get_checked\0"),
    ("__olive_obj_get_default", b"olive_obj_get_default\0"),
    ("__olive_obj_keys", b"olive_obj_keys\0"),
    ("__olive_obj_items", b"olive_obj_items\0"),
    ("__olive_obj_len", b"olive_obj_len\0"),
    ("__olive_obj_new", b"olive_obj_new\0"),
    ("__olive_obj_remove", b"olive_obj_remove\0"),
    ("__olive_obj_set", b"olive_obj_set\0"),
    ("__olive_obj_values", b"olive_obj_values\0"),
    ("__olive_os_args", b"olive_os_args\0"),
    ("__olive_os_exec", b"olive_os_exec\0"),
    ("__olive_os_exec_status", b"olive_os_exec_status\0"),
    ("__olive_os_exit", b"olive_os_exit\0"),
    ("__olive_panic", b"olive_panic\0"),
    ("__olive_bounds_fail", b"olive_bounds_fail\0"),
    ("__olive_nil_index_fail", b"olive_nil_index_fail\0"),
    ("__olive_div_zero_fail", b"olive_div_zero_fail\0"),
    ("__olive_overflow_fail", b"olive_overflow_fail\0"),
    ("__olive_check_nonzero_step", b"olive_check_nonzero_step\0"),
    ("__olive_assert_fail", b"olive_assert_fail\0"),
    ("__olive_check_list_min_len", b"olive_check_list_min_len\0"),
    ("__olive_str_get_checked", b"olive_str_get_checked\0"),
    ("__olive_path_basename", b"olive_path_basename\0"),
    ("__olive_path_dirname", b"olive_path_dirname\0"),
    ("__olive_path_ext", b"olive_path_ext\0"),
    ("__olive_path_is_absolute", b"olive_path_is_absolute\0"),
    ("__olive_path_join", b"olive_path_join\0"),
    ("__olive_path_stem", b"olive_path_stem\0"),
    ("__olive_pool_run", b"olive_pool_run\0"),
    ("__olive_pool_run_sync", b"olive_pool_run_sync\0"),
    ("__olive_pool_size", b"olive_pool_size\0"),
    ("__olive_pow", b"olive_pow\0"),
    ("__olive_pow_float", b"olive_pow_float\0"),
    ("__olive_print_bool", b"olive_print_bool\0"),
    ("__olive_print_float", b"olive_print_float\0"),
    ("__olive_print_int", b"olive_print\0"),
    ("__olive_print_u64", b"olive_print_u64\0"),
    ("__olive_format_u64", b"olive_format_u64\0"),
    ("__olive_str_u64", b"olive_str_u64\0"),
    ("__olive_print_list", b"olive_print_list\0"),
    ("__olive_print_list_float", b"olive_print_list_float\0"),
    ("__olive_print_obj", b"olive_print_obj\0"),
    ("__olive_print_enum", b"olive_print_enum\0"),
    ("__olive_print_any", b"olive_print_any\0"),
    ("__olive_print_str", b"olive_print_str\0"),
    ("__olive_print_py", b"olive_print_py\0"),
    ("__olive_print_typed", b"olive_print_typed\0"),
    ("__olive_write_any", b"olive_write_any\0"),
    ("__olive_write_int", b"olive_write_int\0"),
    ("__olive_write_u64", b"olive_write_u64\0"),
    ("__olive_write_bool", b"olive_write_bool\0"),
    ("__olive_write_float", b"olive_write_float\0"),
    ("__olive_write_str", b"olive_write_str\0"),
    ("__olive_write_typed", b"olive_write_typed\0"),
    ("__olive_write_char", b"olive_write_char\0"),
    ("__olive_write_nl", b"olive_write_nl\0"),
    ("__olive_format_typed", b"olive_format_typed\0"),
    ("__olive_format_int", b"olive_format_int\0"),
    ("__olive_format_float", b"olive_format_float\0"),
    ("__olive_format_str", b"olive_format_str\0"),
    ("__olive_format_bool", b"olive_format_bool\0"),
    ("__olive_format_any", b"olive_format_any\0"),
    ("__olive_box_int", b"olive_box_int\0"),
    ("__olive_box_float", b"olive_box_float\0"),
    ("__olive_box_bool", b"olive_box_bool\0"),
    ("__olive_box_null", b"olive_box_null\0"),
    ("__olive_any_is_null", b"olive_any_is_null\0"),
    ("__olive_unbox_float", b"olive_unbox_float\0"),
    ("__olive_unbox_int", b"olive_unbox_int\0"),
    ("__olive_any_truthy", b"olive_any_truthy\0"),
    ("__olive_any_to_str", b"olive_any_to_str\0"),
    ("__olive_none_to_str", b"olive_none_to_str\0"),
    ("__olive_bool_to_str", b"olive_bool_to_str\0"),
    ("__olive_str", b"olive_str\0"),
    ("__olive_py_call", b"olive_py_call\0"),
    ("__olive_py_call_kw", b"olive_py_call_kw\0"),
    ("__olive_py_call_kw_safe", b"olive_py_call_kw_safe\0"),
    ("__olive_py_call_safe", b"olive_py_call_safe\0"),
    ("__olive_py_call_t", b"olive_py_call_t\0"),
    ("__olive_py_call_t_safe", b"olive_py_call_t_safe\0"),
    ("__olive_py_call_kw_v", b"olive_py_call_kw_v\0"),
    ("__olive_py_call_kw_v_safe", b"olive_py_call_kw_v_safe\0"),
    (
        "__olive_py_call_method_kw_v",
        b"olive_py_call_method_kw_v\0",
    ),
    (
        "__olive_py_call_method_kw_v_safe",
        b"olive_py_call_method_kw_v_safe\0",
    ),
    ("__olive_py_call_kw_v_p0_k1", b"olive_py_call_kw_v_p0_k1\0"),
    (
        "__olive_py_call_kw_v_p0_k1_safe",
        b"olive_py_call_kw_v_p0_k1_safe\0",
    ),
    ("__olive_py_call_kw_v_p0_k2", b"olive_py_call_kw_v_p0_k2\0"),
    (
        "__olive_py_call_kw_v_p0_k2_safe",
        b"olive_py_call_kw_v_p0_k2_safe\0",
    ),
    ("__olive_py_call_kw_v_p0_k3", b"olive_py_call_kw_v_p0_k3\0"),
    (
        "__olive_py_call_kw_v_p0_k3_safe",
        b"olive_py_call_kw_v_p0_k3_safe\0",
    ),
    ("__olive_py_call_kw_v_p0_k4", b"olive_py_call_kw_v_p0_k4\0"),
    (
        "__olive_py_call_kw_v_p0_k4_safe",
        b"olive_py_call_kw_v_p0_k4_safe\0",
    ),
    ("__olive_py_call_kw_v_p1_k1", b"olive_py_call_kw_v_p1_k1\0"),
    (
        "__olive_py_call_kw_v_p1_k1_safe",
        b"olive_py_call_kw_v_p1_k1_safe\0",
    ),
    ("__olive_py_call_kw_v_p1_k2", b"olive_py_call_kw_v_p1_k2\0"),
    (
        "__olive_py_call_kw_v_p1_k2_safe",
        b"olive_py_call_kw_v_p1_k2_safe\0",
    ),
    ("__olive_py_call_kw_v_p1_k3", b"olive_py_call_kw_v_p1_k3\0"),
    (
        "__olive_py_call_kw_v_p1_k3_safe",
        b"olive_py_call_kw_v_p1_k3_safe\0",
    ),
    ("__olive_py_call_kw_v_p2_k1", b"olive_py_call_kw_v_p2_k1\0"),
    (
        "__olive_py_call_kw_v_p2_k1_safe",
        b"olive_py_call_kw_v_p2_k1_safe\0",
    ),
    ("__olive_py_call_kw_v_p2_k2", b"olive_py_call_kw_v_p2_k2\0"),
    (
        "__olive_py_call_kw_v_p2_k2_safe",
        b"olive_py_call_kw_v_p2_k2_safe\0",
    ),
    ("__olive_py_call_kw_v_p3_k1", b"olive_py_call_kw_v_p3_k1\0"),
    (
        "__olive_py_call_kw_v_p3_k1_safe",
        b"olive_py_call_kw_v_p3_k1_safe\0",
    ),
    (
        "__olive_py_call_method_kw_v_p0_k1",
        b"olive_py_call_method_kw_v_p0_k1\0",
    ),
    (
        "__olive_py_call_method_kw_v_p0_k1_safe",
        b"olive_py_call_method_kw_v_p0_k1_safe\0",
    ),
    (
        "__olive_py_call_method_kw_v_p0_k2",
        b"olive_py_call_method_kw_v_p0_k2\0",
    ),
    (
        "__olive_py_call_method_kw_v_p0_k2_safe",
        b"olive_py_call_method_kw_v_p0_k2_safe\0",
    ),
    (
        "__olive_py_call_method_kw_v_p0_k3",
        b"olive_py_call_method_kw_v_p0_k3\0",
    ),
    (
        "__olive_py_call_method_kw_v_p0_k3_safe",
        b"olive_py_call_method_kw_v_p0_k3_safe\0",
    ),
    (
        "__olive_py_call_method_kw_v_p0_k4",
        b"olive_py_call_method_kw_v_p0_k4\0",
    ),
    (
        "__olive_py_call_method_kw_v_p0_k4_safe",
        b"olive_py_call_method_kw_v_p0_k4_safe\0",
    ),
    (
        "__olive_py_call_method_kw_v_p1_k1",
        b"olive_py_call_method_kw_v_p1_k1\0",
    ),
    (
        "__olive_py_call_method_kw_v_p1_k1_safe",
        b"olive_py_call_method_kw_v_p1_k1_safe\0",
    ),
    (
        "__olive_py_call_method_kw_v_p1_k2",
        b"olive_py_call_method_kw_v_p1_k2\0",
    ),
    (
        "__olive_py_call_method_kw_v_p1_k2_safe",
        b"olive_py_call_method_kw_v_p1_k2_safe\0",
    ),
    (
        "__olive_py_call_method_kw_v_p1_k3",
        b"olive_py_call_method_kw_v_p1_k3\0",
    ),
    (
        "__olive_py_call_method_kw_v_p1_k3_safe",
        b"olive_py_call_method_kw_v_p1_k3_safe\0",
    ),
    (
        "__olive_py_call_method_kw_v_p2_k1",
        b"olive_py_call_method_kw_v_p2_k1\0",
    ),
    (
        "__olive_py_call_method_kw_v_p2_k1_safe",
        b"olive_py_call_method_kw_v_p2_k1_safe\0",
    ),
    (
        "__olive_py_call_method_kw_v_p2_k2",
        b"olive_py_call_method_kw_v_p2_k2\0",
    ),
    (
        "__olive_py_call_method_kw_v_p2_k2_safe",
        b"olive_py_call_method_kw_v_p2_k2_safe\0",
    ),
    (
        "__olive_py_call_method_kw_v_p3_k1",
        b"olive_py_call_method_kw_v_p3_k1\0",
    ),
    (
        "__olive_py_call_method_kw_v_p3_k1_safe",
        b"olive_py_call_method_kw_v_p3_k1_safe\0",
    ),
    ("__olive_py_call0", b"olive_py_call0\0"),
    ("__olive_py_call0_safe", b"olive_py_call0_safe\0"),
    ("__olive_py_call1", b"olive_py_call1\0"),
    ("__olive_py_call1_safe", b"olive_py_call1_safe\0"),
    ("__olive_py_call2", b"olive_py_call2\0"),
    ("__olive_py_call2_safe", b"olive_py_call2_safe\0"),
    ("__olive_py_call3", b"olive_py_call3\0"),
    ("__olive_py_call3_safe", b"olive_py_call3_safe\0"),
    ("__olive_py_call4", b"olive_py_call4\0"),
    ("__olive_py_call4_safe", b"olive_py_call4_safe\0"),
    ("__olive_py_call_method0", b"olive_py_call_method0\0"),
    (
        "__olive_py_call_method0_safe",
        b"olive_py_call_method0_safe\0",
    ),
    ("__olive_py_call_method1", b"olive_py_call_method1\0"),
    (
        "__olive_py_call_method1_safe",
        b"olive_py_call_method1_safe\0",
    ),
    ("__olive_py_call_method2", b"olive_py_call_method2\0"),
    (
        "__olive_py_call_method2_safe",
        b"olive_py_call_method2_safe\0",
    ),
    ("__olive_py_call_method3", b"olive_py_call_method3\0"),
    (
        "__olive_py_call_method3_safe",
        b"olive_py_call_method3_safe\0",
    ),
    ("__olive_py_call_method4", b"olive_py_call_method4\0"),
    (
        "__olive_py_call_method4_safe",
        b"olive_py_call_method4_safe\0",
    ),
    ("__olive_py_conv_to_py", b"olive_py_conv_to_py\0"),
    ("__olive_py_decref", b"olive_py_decref\0"),
    ("__olive_py_eq", b"olive_py_eq\0"),
    ("__olive_py_lt", b"olive_py_lt\0"),
    ("__olive_py_le", b"olive_py_le\0"),
    ("__olive_py_gt", b"olive_py_gt\0"),
    ("__olive_py_ge", b"olive_py_ge\0"),
    ("__olive_py_ne", b"olive_py_ne\0"),
    ("__olive_py_add", b"olive_py_add\0"),
    ("__olive_py_sub", b"olive_py_sub\0"),
    ("__olive_py_mul", b"olive_py_mul\0"),
    ("__olive_py_div", b"olive_py_div\0"),
    ("__olive_py_mod", b"olive_py_mod\0"),
    ("__olive_py_pow", b"olive_py_pow\0"),
    ("__olive_py_finalize", b"olive_py_finalize\0"),
    ("__olive_py_gil_begin", b"olive_py_gil_begin\0"),
    ("__olive_py_gil_end", b"olive_py_gil_end\0"),
    ("__olive_py_gil_checkpoint", b"olive_py_gil_checkpoint\0"),
    ("__olive_py_from_float", b"olive_py_from_float\0"),
    ("__olive_py_from_int", b"olive_py_from_int\0"),
    ("__olive_py_from_str", b"olive_py_from_str\0"),
    ("__olive_py_getattr", b"olive_py_getattr\0"),
    ("__olive_py_getattr_ret", b"olive_py_getattr_ret\0"),
    ("__olive_py_getattr_safe", b"olive_py_getattr_safe\0"),
    ("__olive_py_getitem", b"olive_py_getitem\0"),
    ("__olive_py_getitem_int", b"olive_py_getitem_int\0"),
    ("__olive_py_getitem_safe", b"olive_py_getitem_safe\0"),
    ("__olive_py_getslice", b"olive_py_getslice\0"),
    ("__olive_py_import", b"olive_py_import\0"),
    ("__olive_py_import_safe", b"olive_py_import_safe\0"),
    ("__olive_py_initialize", b"olive_py_initialize\0"),
    ("__olive_py_bitor", b"olive_py_bitor\0"),
    ("__olive_py_is_none", b"olive_py_is_none\0"),
    ("__olive_py_is_handle", b"olive_py_is_handle\0"),
    ("__olive_py_len", b"olive_py_len\0"),
    ("__olive_py_none", b"olive_py_none\0"),
    ("__olive_py_set_loc", b"olive_py_set_loc\0"),
    ("__olive_set_fault_loc", b"olive_set_fault_loc\0"),
    ("__olive_shadow_push", b"olive_shadow_push\0"),
    ("__olive_shadow_pop", b"olive_shadow_pop\0"),
    ("__olive_py_setattr", b"olive_py_setattr\0"),
    ("__olive_py_setattr_safe", b"olive_py_setattr_safe\0"),
    ("__olive_py_setitem", b"olive_py_setitem\0"),
    ("__olive_py_setitem_int", b"olive_py_setitem_int\0"),
    ("__olive_py_setitem_safe", b"olive_py_setitem_safe\0"),
    ("__olive_py_to_any_dict", b"olive_py_to_any_dict\0"),
    ("__olive_py_to_any_list", b"olive_py_to_any_list\0"),
    ("__olive_py_to_bytes", b"olive_py_to_bytes\0"),
    ("__olive_py_to_dict", b"olive_py_to_dict\0"),
    ("__olive_py_to_float", b"olive_py_to_float\0"),
    ("__olive_py_to_int", b"olive_py_to_int\0"),
    ("__olive_py_copy_ref", b"olive_py_copy_ref\0"),
    ("__olive_py_to_list", b"olive_py_to_list\0"),
    ("__olive_py_to_str", b"olive_py_to_str\0"),
    ("__olive_py_to_any", b"olive_py_to_any\0"),
    ("__olive_to_pyobject", b"olive_to_pyobject\0"),
    ("__olive_py_make_callable", b"olive_py_make_callable\0"),
    ("__olive_py_make_export", b"olive_py_make_export\0"),
    ("__olive_py_create_module", b"olive_py_create_module\0"),
    (
        "__olive_py_module_add_object",
        b"olive_py_module_add_object\0",
    ),
    ("__olive_random_get", b"olive_random_get\0"),
    ("__olive_random_int", b"olive_random_int\0"),
    ("__olive_random_seed", b"olive_random_seed\0"),
    ("__olive_regex_captures", b"olive_regex_captures\0"),
    ("__olive_regex_find", b"olive_regex_find\0"),
    ("__olive_regex_find_all", b"olive_regex_find_all\0"),
    ("__olive_regex_is_valid", b"olive_regex_is_valid\0"),
    ("__olive_regex_match", b"olive_regex_match\0"),
    ("__olive_regex_replace", b"olive_regex_replace\0"),
    ("__olive_regex_replace_all", b"olive_regex_replace_all\0"),
    ("__olive_regex_split", b"olive_regex_split\0"),
    ("__olive_result_err", b"olive_result_err\0"),
    ("__olive_result_err_msg", b"olive_result_err_msg\0"),
    ("__olive_result_is_err", b"olive_result_is_err\0"),
    ("__olive_result_is_ok", b"olive_result_is_ok\0"),
    ("__olive_result_ok", b"olive_result_ok\0"),
    ("__olive_result_unwrap", b"olive_result_unwrap\0"),
    ("__olive_result_unwrap_err", b"olive_result_unwrap_err\0"),
    ("__olive_result_unwrap_or", b"olive_result_unwrap_or\0"),
    ("__olive_run_exit_hooks", b"olive_run_exit_hooks\0"),
    ("__olive_select", b"olive_select\0"),
    ("__olive_set_add", b"olive_set_add\0"),
    ("__olive_set_contains", b"olive_set_contains\0"),
    ("__olive_set_index_any", b"olive_set_index_any\0"),
    ("__olive_set_new", b"olive_set_new\0"),
    ("__olive_set_remove", b"olive_set_remove\0"),
    ("__olive_set_union", b"olive_set_union\0"),
    ("__olive_set_intersection", b"olive_set_intersection\0"),
    ("__olive_set_diff", b"olive_set_diff\0"),
    ("__olive_set_sym_diff", b"olive_set_sym_diff\0"),
    ("__olive_sm_poll", b"olive_sm_poll\0"),
    ("__olive_spawn_task", b"olive_spawn_task\0"),
    ("__olive_stdin_read", b"olive_stdin_read\0"),
    ("__olive_stdin_read_line", b"olive_stdin_read_line\0"),
    ("__olive_str", b"olive_str\0"),
    ("__olive_str_char", b"olive_str_char\0"),
    ("__olive_str_char_count", b"olive_str_char_count\0"),
    ("__olive_str_concat", b"olive_str_concat\0"),
    ("__olive_list_repeat", b"olive_list_repeat\0"),
    ("__olive_list_repeat_typed", b"olive_list_repeat_typed\0"),
    ("__olive_any_add", b"olive_any_add\0"),
    ("__olive_any_add_profiled", b"olive_any_add_profiled\0"),
    ("__olive_any_sub", b"olive_any_sub\0"),
    ("__olive_any_sub_profiled", b"olive_any_sub_profiled\0"),
    ("__olive_any_mul", b"olive_any_mul\0"),
    ("__olive_any_mul_profiled", b"olive_any_mul_profiled\0"),
    ("__olive_any_div", b"olive_any_div\0"),
    ("__olive_any_div_profiled", b"olive_any_div_profiled\0"),
    ("__olive_any_mod", b"olive_any_mod\0"),
    ("__olive_any_mod_profiled", b"olive_any_mod_profiled\0"),
    ("__olive_any_lt", b"olive_any_lt\0"),
    ("__olive_any_lt_profiled", b"olive_any_lt_profiled\0"),
    ("__olive_any_le", b"olive_any_le\0"),
    ("__olive_any_le_profiled", b"olive_any_le_profiled\0"),
    ("__olive_any_gt", b"olive_any_gt\0"),
    ("__olive_any_gt_profiled", b"olive_any_gt_profiled\0"),
    ("__olive_any_ge", b"olive_any_ge\0"),
    ("__olive_any_ge_profiled", b"olive_any_ge_profiled\0"),
    ("__olive_any_eq", b"olive_any_eq\0"),
    ("__olive_any_eq_profiled", b"olive_any_eq_profiled\0"),
    ("__olive_any_ne", b"olive_any_ne\0"),
    ("__olive_any_ne_profiled", b"olive_any_ne_profiled\0"),
    ("__olive_any_eq_strict", b"olive_any_eq_strict\0"),
    ("__olive_any_ne_strict", b"olive_any_ne_strict\0"),
    ("__olive_struct_box", b"olive_struct_box\0"),
    ("__olive_str_capitalize", b"olive_str_capitalize\0"),
    ("__olive_str_center", b"olive_str_center\0"),
    ("__olive_str_contains", b"olive_str_contains\0"),
    ("__olive_str_count", b"olive_str_count\0"),
    ("__olive_str_ends_with", b"olive_str_ends_with\0"),
    ("__olive_str_eq", b"olive_str_eq\0"),
    ("__olive_str_find", b"olive_str_find\0"),
    ("__olive_str_fmt", b"olive_str_fmt\0"),
    ("__olive_str_get", b"olive_str_get\0"),
    ("__olive_str_grapheme_count", b"olive_str_grapheme_count\0"),
    ("__olive_str_graphemes", b"olive_str_graphemes\0"),
    ("__olive_str_is_ascii", b"olive_str_is_ascii\0"),
    ("__olive_str_isalpha", b"olive_str_isalpha\0"),
    ("__olive_str_isdigit", b"olive_str_isdigit\0"),
    ("__olive_str_islower", b"olive_str_islower\0"),
    ("__olive_str_isspace", b"olive_str_isspace\0"),
    ("__olive_str_isupper", b"olive_str_isupper\0"),
    ("__olive_str_join", b"olive_str_join\0"),
    ("__olive_str_len", b"olive_str_len\0"),
    ("__olive_str_ljust", b"olive_str_ljust\0"),
    ("__olive_str_lower", b"olive_str_lower\0"),
    ("__olive_str_partition", b"olive_str_partition\0"),
    ("__olive_str_removeprefix", b"olive_str_removeprefix\0"),
    ("__olive_str_removesuffix", b"olive_str_removesuffix\0"),
    ("__olive_str_repeat", b"olive_str_repeat\0"),
    ("__olive_str_replace", b"olive_str_replace\0"),
    ("__olive_str_rfind", b"olive_str_rfind\0"),
    ("__olive_str_rjust", b"olive_str_rjust\0"),
    ("__olive_str_slice", b"olive_str_slice\0"),
    ("__olive_str_splitlines", b"olive_str_splitlines\0"),
    ("__olive_str_title", b"olive_str_title\0"),
    ("__olive_str_trim_chars", b"olive_str_trim_chars\0"),
    ("__olive_str_trim_end_chars", b"olive_str_trim_end_chars\0"),
    (
        "__olive_str_trim_start_chars",
        b"olive_str_trim_start_chars\0",
    ),
    ("__olive_str_zfill", b"olive_str_zfill\0"),
    ("__olive_str_getslice", b"olive_str_getslice\0"),
    ("__olive_str_split", b"olive_str_split\0"),
    ("__olive_str_starts_with", b"olive_str_starts_with\0"),
    ("__olive_str_to_float", b"olive_str_to_float\0"),
    ("__olive_str_to_int", b"olive_str_to_int\0"),
    ("__olive_str_to_float_opt", b"olive_str_to_float_opt\0"),
    ("__olive_str_to_int_opt", b"olive_str_to_int_opt\0"),
    ("__olive_str_trim", b"olive_str_trim\0"),
    ("__olive_str_trim_end", b"olive_str_trim_end\0"),
    ("__olive_str_trim_start", b"olive_str_trim_start\0"),
    ("__olive_str_upper", b"olive_str_upper\0"),
    ("__olive_struct_alloc", b"olive_struct_alloc\0"),
    ("__olive_sys_arch", b"olive_sys_arch\0"),
    ("__olive_sys_chdir", b"olive_sys_chdir\0"),
    ("__olive_sys_cpu_count", b"olive_sys_cpu_count\0"),
    ("__olive_sys_cwd", b"olive_sys_cwd\0"),
    ("__olive_sys_home_dir", b"olive_sys_home_dir\0"),
    ("__olive_sys_hostname", b"olive_sys_hostname\0"),
    ("__olive_sys_memory_free", b"olive_sys_memory_free\0"),
    ("__olive_sys_memory_total", b"olive_sys_memory_total\0"),
    ("__olive_sys_pid", b"olive_sys_pid\0"),
    ("__olive_sys_platform", b"olive_sys_platform\0"),
    ("__olive_sys_uptime", b"olive_sys_uptime\0"),
    ("__olive_sys_username", b"olive_sys_username\0"),
    ("__olive_temp_dir", b"olive_temp_dir\0"),
    ("__olive_temp_file", b"olive_temp_file\0"),
    ("__olive_time_format", b"olive_time_format\0"),
    ("__olive_time_monotonic", b"olive_time_monotonic\0"),
    ("__olive_time_now", b"olive_time_now\0"),
    ("__olive_time_sleep", b"olive_time_sleep\0"),
    ("__olive_toml_parse", b"olive_toml_parse\0"),
    ("__olive_toml_stringify", b"olive_toml_stringify\0"),
    ("__olive_typeof_str", b"olive_typeof_str\0"),
    ("__olive_url_decode", b"olive_url_decode\0"),
    ("__olive_url_encode", b"olive_url_encode\0"),
    ("__olive_uuid_is_valid", b"olive_uuid_is_valid\0"),
    ("__olive_uuid_nil", b"olive_uuid_nil\0"),
    ("__olive_uuid_to_hex", b"olive_uuid_to_hex\0"),
    ("__olive_uuid_v4", b"olive_uuid_v4\0"),
    ("__olive_vararg_call", b"olive_vararg_call\0"),
    ("__olive_websocket_close", b"olive_websocket_close\0"),
    ("__olive_websocket_connect", b"olive_websocket_connect\0"),
    ("__olive_websocket_recv", b"olive_websocket_recv\0"),
    (
        "__olive_websocket_recv_binary",
        b"olive_websocket_recv_binary\0",
    ),
    ("__olive_websocket_send", b"olive_websocket_send\0"),
    (
        "__olive_websocket_send_binary",
        b"olive_websocket_send_binary\0",
    ),
    ("__olive_yaml_parse", b"olive_yaml_parse\0"),
    ("__olive_yaml_stringify", b"olive_yaml_stringify\0"),
    ("__olive_zstd_compress", b"olive_zstd_compress\0"),
    ("__olive_zstd_decompress", b"olive_zstd_decompress\0"),
    (
        "__olive_signal_install_sigint",
        b"olive_signal_install_sigint\0",
    ),
    ("__olive_export_buffer_view", b"olive_export_buffer_view\0"),
    ("__olive_dlpack_export", b"olive_dlpack_export\0"),
    ("__olive_dlpack_import", b"olive_dlpack_import\0"),
    ("__olive_dlpack_data_ptr", b"olive_dlpack_data_ptr\0"),
    ("__olive_dlpack_ndim", b"olive_dlpack_ndim\0"),
    ("__olive_dlpack_shape_at", b"olive_dlpack_shape_at\0"),
    ("__olive_dlpack_dtype_code", b"olive_dlpack_dtype_code\0"),
    ("__olive_dlpack_bits", b"olive_dlpack_bits\0"),
    ("__olive_dlpack_device_type", b"olive_dlpack_device_type\0"),
    ("__olive_dlpack_release", b"olive_dlpack_release\0"),
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

pub struct CraneliftCodegen<M: Module> {
    pub(super) functions: Vec<MirFunction>,
    pub(super) module: M,
    pub(super) func_ids: HashMap<String, FuncId>,
    pub(super) string_ids: HashMap<String, DataId>,
    pub(super) struct_fields: HashMap<String, Vec<String>>,
    pub(super) field_types: HashMap<(String, String), crate::semantic::types::Type>,
    pub(super) enum_defs: HashMap<String, Vec<(String, Vec<crate::semantic::types::Type>)>>,
    pub(super) _libs: Vec<libloading::Library>,
    pub(super) native_aliases: std::collections::HashSet<String>,
    pub(super) ffi_entries: Vec<FfiFnEntry>,
    pub(super) ffi_vararg_ptrs: HashMap<String, *const u8>,
    pub(super) ffi_vararg_ids: std::collections::HashSet<String>,
    pub(super) c_struct_offsets: HashMap<String, Vec<FfiStructFieldLayout>>,
    pub(super) c_struct_sizes: HashMap<String, i64>,
    pub(super) c_struct_names: std::collections::HashSet<String>,
    pub(super) c_struct_destructors: HashMap<String, String>,
    pub(super) aot: bool,
    pub(crate) pymodule: bool,
    pub(crate) pymodule_name: Option<String>,
    pub(super) extern_var_ptrs: HashMap<String, (i64, String, String)>,
    pub(super) vtables: HashMap<String, Vec<String>>,
    pub(super) global_vars: Vec<String>,
    pub(super) file_names: HashMap<usize, String>,
    pub(super) loc_ids: HashMap<crate::span::Span, DataId>,
    /// Whether to emit per-function call-count instrumentation (`setup/profiling.rs`).
    /// On for JIT (feeds future tier-up decisions), off for AOT unless a profile is requested.
    /// `pub(crate)` (wider than most fields here) so test harnesses can toggle it pre-`generate()`.
    pub(crate) profile: bool,
    /// Function name -> its `__olive_hotcount$<name>` data segment, set by `generate_hotcounts`.
    pub(super) hotcount_ids: HashMap<String, DataId>,
    /// Function name -> its `__olive_dispatch$<name>` pointer cell, set by `generate_dispatch_cells`.
    /// Calls to a function with a cell go through it (load + call_indirect) so a future
    /// tier-up recompiler can retarget the cell without touching call sites. Populated
    /// only when `profile` is set (JIT); AOT keeps today's direct calls.
    pub(super) dispatch_ids: HashMap<String, DataId>,
    /// When set, `generate_dispatch_cells` installs a cell for every
    /// debug-instrumentable function (not just the Any-add-site subset),
    /// so `tooling::dap::launch` can swap each one between its clean and
    /// `$debug` compiled bodies. Only ever set for a `pit debug` session's
    /// codegen; `pit run` never touches this.
    pub(crate) debug_dual_variant: bool,
    /// One 1-byte kind-history cell per `Any`-typed `+` call site in source order,
    /// allocated by `generate_kind_history` (counts sites first, then allocates).
    /// `translate_binop.rs` consumes them via `any_add_site_cursor` in the same
    /// deterministic statement walk `translate_function` already does, so the same
    /// site always gets the same cell across a `generate()` call. Populated only
    /// when `profile` is set. Feeds Phase 2 (Any-op inline specialization); nothing
    /// reads these cells yet.
    pub(super) any_add_site_ids: Vec<DataId>,
    pub(super) any_add_site_cursor: usize,
    /// Function name -> `[start, end)` range in `any_add_site_ids` that function's
    /// own sites occupy, recorded by `generate_kind_history` in the same order
    /// `count_any_add_sites` visits functions. `retier()` resets the cursor to a
    /// function's own `start` before re-translating it, so re-consuming site slots
    /// during a retier reads back *that function's* observed history instead of
    /// silently running off the end of the array (which reads as permanently
    /// out-of-bounds -- safe, since every consumer bounds-checks, but it would
    /// make specialization never fire at all).
    pub(super) any_add_site_ranges: HashMap<String, (usize, usize)>,
    /// Site indices (within the function currently being retiered's own
    /// `any_add_site_ranges` range) `retier()` found graduated to all-int.
    /// `translate_binop.rs` emits the guarded native fast path for these instead
    /// of an unconditional runtime call. Always empty outside a retier.
    pub(super) specialize_sites: rustc_hash::FxHashSet<usize>,
}

// The only field blocking auto-derived `Send` is `ffi_vararg_ptrs: HashMap<String, *const u8>`
// -- raw function pointers into an FFI library kept alive for the process lifetime by `_libs`.
// They're set once during `new_jit` and only ever read as call targets afterward, so moving a
// `CraneliftCodegen<JITModule>` into the tier-up thread (`tier_up::spawn_tier_up_thread`) is
// sound: all mutation of the codegen state (including these pointers' owning struct) happens
// under the `Mutex` that wraps it, never concurrently.
unsafe impl Send for CraneliftCodegen<JITModule> {}

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
) -> (Vec<FfiStructFieldLayout>, i64) {
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

impl CraneliftCodegen<JITModule> {
    /// Surrenders the module so a test harness can `free_memory` it; cranelift
    /// leaks JIT mappings otherwise.
    #[cfg(test)]
    pub(crate) fn into_module(self) -> JITModule {
        self.module
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_jit(
        functions: Vec<MirFunction>,
        struct_fields: HashMap<String, Vec<String>>,
        field_types: HashMap<(String, String), crate::semantic::types::Type>,
        enum_defs: HashMap<String, Vec<(String, Vec<crate::semantic::types::Type>)>>,
        vtables: HashMap<String, Vec<String>>,
        global_vars: Vec<String>,
        file_names: HashMap<usize, String>,
        native_lib_paths: &[FfiLibInfo],
        release: bool,
    ) -> Self {
        let mut flag_builder = settings::builder();
        flag_builder.set("use_colocated_libcalls", "false").unwrap();
        flag_builder.set("is_pic", "false").unwrap();
        if release {
            flag_builder.set("opt_level", "speed").unwrap();
        } else {
            flag_builder.set("opt_level", "none").unwrap();
        }
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
        // Default provider mmaps per finalize call; a retier can land >2GB
        // from the runtime lib and panic the relocation (32-bit displacement).
        reserve_jit_arena(&mut builder, JIT_ARENA_SIZE);

        // Debug hooks: registered unconditionally, called only when a debug
        // session instrumented the MIR, so a plain run pays nothing for them.
        for (name, ptr) in crate::tooling::dap::hooks::jit_symbols() {
            builder.symbol(name, ptr);
        }

        let needed = imports::collect_needed_imports(&functions);
        let has_async = functions.iter().any(|f| f.is_async);

        let mut libs: Vec<libloading::Library> = Vec::new();
        let mut native_aliases = std::collections::HashSet::new();
        let mut ffi_entries: Vec<FfiFnEntry> = Vec::new();
        let mut ffi_vararg_ptrs: HashMap<String, *const u8> = HashMap::default();
        let mut c_struct_offsets: HashMap<String, Vec<FfiStructFieldLayout>> = HashMap::default();
        let mut c_struct_sizes: HashMap<String, i64> = HashMap::default();
        let mut c_struct_names: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let mut c_struct_destructors: HashMap<String, String> = HashMap::default();
        let has_c_structs = native_lib_paths
            .iter()
            .any(|(_, _, _, structs, _)| !structs.is_empty());
        let mut extern_var_ptrs: HashMap<String, (i64, String, String)> = HashMap::default();

        let has_traits = !vtables.is_empty();
        if let Some(lib) = jit_loader::register_runtime_symbols(
            &mut builder,
            &needed,
            has_async,
            has_c_structs || has_traits,
        ) {
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
                    for func in &functions {
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
                            if c_struct_sizes.get(&ret_name).is_some_and(|&size| size > 16) {
                                use_sret = true;
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
            field_types,
            enum_defs,
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
            pymodule: false,
            pymodule_name: None,
            extern_var_ptrs,
            vtables,
            global_vars,
            file_names,
            loc_ids: HashMap::default(),
            profile: true,
            hotcount_ids: HashMap::default(),
            dispatch_ids: HashMap::default(),
            debug_dual_variant: false,
            any_add_site_ids: Vec::new(),
            any_add_site_cursor: 0,
            any_add_site_ranges: HashMap::default(),
            specialize_sites: rustc_hash::FxHashSet::default(),
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

    /// Reads a function's call-count counter. Only meaningful after `finalize()`
    /// and only for functions `generate_hotcounts` instrumented (sync, non-async).
    pub fn hotcount(&mut self, func_name: &str) -> Option<i64> {
        let id = *self.hotcount_ids.get(func_name)?;
        let bytes = self.module.get_finalized_data(id).0;
        let arr: [u8; 8] = unsafe { std::slice::from_raw_parts(bytes, 8) }
            .try_into()
            .unwrap();
        Some(i64::from_ne_bytes(arr))
    }
}

impl CraneliftCodegen<ObjectModule> {
    #[allow(clippy::too_many_arguments)]
    pub fn new_aot(
        functions: Vec<MirFunction>,
        struct_fields: HashMap<String, Vec<String>>,
        field_types: HashMap<(String, String), crate::semantic::types::Type>,
        enum_defs: HashMap<String, Vec<(String, Vec<crate::semantic::types::Type>)>>,
        vtables: HashMap<String, Vec<String>>,
        global_vars: Vec<String>,
        file_names: HashMap<usize, String>,
        native_lib_paths: &[FfiLibInfo],
        release: bool,
    ) -> Self {
        let mut flag_builder = settings::builder();
        flag_builder.set("use_colocated_libcalls", "false").unwrap();
        flag_builder.set("is_pic", "true").unwrap();
        if release {
            flag_builder.set("opt_level", "speed").unwrap();
        } else {
            flag_builder.set("opt_level", "none").unwrap();
        }
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
        let mut c_struct_offsets: HashMap<String, Vec<FfiStructFieldLayout>> = HashMap::default();
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
                    if c_struct_sizes
                        .get(&ret_ty_name)
                        .is_some_and(|&size| size > 16)
                    {
                        use_sret = true;
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
            field_types,
            enum_defs,
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
            pymodule: false,
            pymodule_name: None,
            extern_var_ptrs,
            vtables,
            global_vars,
            file_names,
            loc_ids: HashMap::default(),
            profile: false,
            hotcount_ids: HashMap::default(),
            dispatch_ids: HashMap::default(),
            debug_dual_variant: false,
            any_add_site_ids: Vec::new(),
            any_add_site_cursor: 0,
            any_add_site_ranges: HashMap::default(),
            specialize_sites: rustc_hash::FxHashSet::default(),
        }
    }

    pub fn emit_object(self) -> Vec<u8> {
        self.module.finish().emit().unwrap()
    }
}

impl<M: Module> CraneliftCodegen<M> {}
