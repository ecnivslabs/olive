use crate::json::{json_to_olive, olive_to_json};
use crate::{olive_str_from_ptr, olive_str_internal};

#[unsafe(no_mangle)]
pub extern "C" fn olive_yaml_parse(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    let text = olive_str_from_ptr(s);
    match serde_yaml::from_str::<serde_json::Value>(&text) {
        Ok(val) => json_to_olive(&val),
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_yaml_stringify(ptr: i64) -> i64 {
    let val = olive_to_json(ptr);
    match serde_yaml::to_string(&val) {
        Ok(s) => olive_str_internal(&s),
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_toml_parse(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    let text = olive_str_from_ptr(s);
    match toml::from_str::<serde_json::Value>(&text) {
        Ok(val) => json_to_olive(&val),
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_toml_stringify(ptr: i64) -> i64 {
    let val = olive_to_json(ptr);
    match toml::to_string(&val) {
        Ok(s) => olive_str_internal(&s),
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{KIND_LIST, KIND_OBJ, OliveObj, StableVec};

    fn s(text: &str) -> i64 {
        crate::olive_str_internal(text)
    }

    fn from_ptr(ptr: i64) -> String {
        crate::olive_str_from_ptr(ptr)
    }

    #[test]
    fn yaml_parse_mapping() {
        let yaml = s("name: alice\nage: 30\n");
        let obj_ptr = olive_yaml_parse(yaml);
        assert_ne!(obj_ptr, 0);
        let obj = unsafe { &*(obj_ptr as *const OliveObj) };
        assert_eq!(obj.kind, KIND_OBJ);
        let name = *obj.fields.get("name").unwrap();
        assert_eq!(from_ptr(name), "alice");
    }

    #[test]
    fn yaml_parse_list() {
        let yaml = s("- 1\n- 2\n- 3\n");
        let ptr = olive_yaml_parse(yaml);
        assert_ne!(ptr, 0);
        let list = unsafe { &*(ptr as *const StableVec) };
        assert_eq!(list.kind, KIND_LIST);
        assert_eq!(list.len, 3);
    }

    #[test]
    fn yaml_parse_invalid() {
        assert_eq!(olive_yaml_parse(s("{{")), 0);
    }

    #[test]
    fn yaml_null_input() {
        assert_eq!(olive_yaml_parse(0), 0);
    }

    #[test]
    fn toml_parse_basic() {
        let t = s("[server]\nhost = \"localhost\"\nport = 8080\n");
        let ptr = olive_toml_parse(t);
        assert_ne!(ptr, 0);
        let obj = unsafe { &*(ptr as *const OliveObj) };
        let server_ptr = *obj.fields.get("server").unwrap();
        let server = unsafe { &*(server_ptr as *const OliveObj) };
        assert_eq!(from_ptr(*server.fields.get("host").unwrap()), "localhost");
    }

    #[test]
    fn toml_parse_invalid() {
        assert_eq!(olive_toml_parse(s("[[invalid")), 0);
    }

    #[test]
    fn toml_null_input() {
        assert_eq!(olive_toml_parse(0), 0);
    }
}
