use crate::olive_str_internal;
use uuid::Uuid;

#[unsafe(no_mangle)]
pub extern "C" fn olive_uuid_v4() -> i64 {
    olive_str_internal(&Uuid::new_v4().to_string())
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_uuid_nil() -> i64 {
    olive_str_internal(&Uuid::nil().to_string())
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_uuid_is_valid(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    let text = crate::olive_str_from_ptr(s);
    if Uuid::parse_str(&text).is_ok() { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_uuid_to_hex(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    let text = crate::olive_str_from_ptr(s);
    match Uuid::parse_str(&text) {
        Ok(u) => olive_str_internal(&u.simple().to_string()),
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::olive_str_internal;

    fn from_ptr(ptr: i64) -> String {
        crate::olive_str_from_ptr(ptr)
    }

    #[test]
    fn v4_format() {
        let u = from_ptr(olive_uuid_v4());
        assert_eq!(u.len(), 36);
        assert_eq!(u.chars().filter(|&c| c == '-').count(), 4);
        assert_eq!(olive_uuid_is_valid(olive_uuid_v4()), 1);
    }

    #[test]
    fn v4_unique() {
        let a = from_ptr(olive_uuid_v4());
        let b = from_ptr(olive_uuid_v4());
        assert_ne!(a, b);
    }

    #[test]
    fn nil_uuid() {
        let u = from_ptr(olive_uuid_nil());
        assert_eq!(u, "00000000-0000-0000-0000-000000000000");
    }

    #[test]
    fn is_valid_checks() {
        let valid = olive_str_internal("550e8400-e29b-41d4-a716-446655440000");
        let invalid = olive_str_internal("not-a-uuid");
        assert_eq!(olive_uuid_is_valid(valid), 1);
        assert_eq!(olive_uuid_is_valid(invalid), 0);
        assert_eq!(olive_uuid_is_valid(0), 0);
    }

    #[test]
    fn to_hex_strips_dashes() {
        let u = olive_str_internal("550e8400-e29b-41d4-a716-446655440000");
        let hex = from_ptr(olive_uuid_to_hex(u));
        assert_eq!(hex.len(), 32);
        assert!(!hex.contains('-'));
    }
}
