use crate::{OliveObj, StableVec, KIND_LIST, KIND_OBJ, olive_str_from_ptr, olive_str_internal};
use rustc_hash::FxHashMap as HashMap;

pub(crate) fn json_to_olive(val: &serde_json::Value) -> i64 {
    match val {
        serde_json::Value::Null => 0,
        serde_json::Value::Bool(b) => {
            if *b { 1 } else { 0 }
        }
        serde_json::Value::Number(n) => n.as_i64().unwrap_or_else(|| n.as_f64().unwrap_or(0.0) as i64),
        serde_json::Value::String(s) => olive_str_internal(s),
        serde_json::Value::Array(arr) => {
            let mut elems: Vec<i64> = arr.iter().map(json_to_olive).collect();
            let ptr = elems.as_mut_ptr();
            let cap = elems.capacity();
            let len = elems.len();
            std::mem::forget(elems);
            Box::into_raw(Box::new(StableVec { kind: KIND_LIST, ptr, cap, len })) as i64
        }
        serde_json::Value::Object(map) => {
            let mut fields: HashMap<String, i64> = HashMap::default();
            for (k, v) in map {
                fields.insert(k.clone(), json_to_olive(v));
            }
            Box::into_raw(Box::new(OliveObj { kind: KIND_OBJ, fields })) as i64
        }
    }
}

// Linux mmap_min_addr is typically 65536; no valid heap ptr lives below this.
const MIN_HEAP_PTR: i64 = 0x10000;

pub(crate) fn olive_to_json(val: i64) -> serde_json::Value {
    if val == 0 {
        return serde_json::Value::Null;
    }
    // Values below MIN_HEAP_PTR can't be valid pointers — treat as integers.
    if val > 0 && val < MIN_HEAP_PTR {
        return serde_json::Value::Number(val.into());
    }
    if val < 0 {
        return serde_json::Value::Number(val.into());
    }
    if val & 1 != 0 {
        return serde_json::Value::String(olive_str_from_ptr(val));
    }
    let kind = unsafe { *(val as *const i64) };
    match kind {
        KIND_LIST => {
            let s = unsafe { &*(val as *const StableVec) };
            let elems: Vec<serde_json::Value> =
                (0..s.len).map(|i| olive_to_json(unsafe { *s.ptr.add(i) })).collect();
            serde_json::Value::Array(elems)
        }
        KIND_OBJ => {
            let obj = unsafe { &*(val as *const OliveObj) };
            let mut map = serde_json::Map::new();
            for (k, &v) in &obj.fields {
                map.insert(k.clone(), olive_to_json(v));
            }
            serde_json::Value::Object(map)
        }
        _ => serde_json::Value::Number(val.into()),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_json_parse(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    let text = olive_str_from_ptr(s);
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(val) => json_to_olive(&val),
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_json_stringify(ptr: i64) -> i64 {
    let val = olive_to_json(ptr);
    match serde_json::to_string(&val) {
        Ok(s) => olive_str_internal(&s),
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_json_stringify_pretty(ptr: i64) -> i64 {
    let val = olive_to_json(ptr);
    match serde_json::to_string_pretty(&val) {
        Ok(s) => olive_str_internal(&s),
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::olive_str_internal;

    fn s(text: &str) -> i64 {
        olive_str_internal(text)
    }

    fn from_ptr(ptr: i64) -> String {
        olive_str_from_ptr(ptr)
    }

    #[test]
    fn parse_null() {
        assert_eq!(olive_json_parse(s("null")), 0);
    }

    #[test]
    fn parse_bool() {
        assert_eq!(olive_json_parse(s("true")), 1);
        assert_eq!(olive_json_parse(s("false")), 0);
    }

    #[test]
    fn parse_integer() {
        assert_eq!(olive_json_parse(s("42")), 42);
        assert_eq!(olive_json_parse(s("-7")), -7);
    }

    #[test]
    fn parse_string() {
        let result = olive_json_parse(s("\"hello\""));
        assert_ne!(result, 0);
        assert_eq!(from_ptr(result), "hello");
    }

    #[test]
    fn parse_array() {
        let ptr = olive_json_parse(s("[1,2,3]"));
        assert_ne!(ptr, 0);
        let list = unsafe { &*(ptr as *const StableVec) };
        assert_eq!(list.kind, KIND_LIST);
        assert_eq!(list.len, 3);
        assert_eq!(unsafe { *list.ptr }, 1);
        assert_eq!(unsafe { *list.ptr.add(1) }, 2);
        assert_eq!(unsafe { *list.ptr.add(2) }, 3);
    }

    #[test]
    fn parse_object() {
        let ptr = olive_json_parse(s(r#"{"x":10,"y":20}"#));
        assert_ne!(ptr, 0);
        let obj = unsafe { &*(ptr as *const OliveObj) };
        assert_eq!(obj.kind, KIND_OBJ);
        assert_eq!(*obj.fields.get("x").unwrap(), 10);
        assert_eq!(*obj.fields.get("y").unwrap(), 20);
    }

    #[test]
    fn stringify_null() {
        let result = olive_json_stringify(0);
        assert_eq!(from_ptr(result), "null");
    }

    #[test]
    fn stringify_string() {
        let ptr = olive_json_parse(s("\"world\""));
        let out = olive_json_stringify(ptr);
        assert_eq!(from_ptr(out), "\"world\"");
    }

    #[test]
    fn roundtrip_object() {
        let json_in = r#"{"name":"alice","age":30}"#;
        let parsed = olive_json_parse(s(json_in));
        let out_ptr = olive_json_stringify(parsed);
        let out = from_ptr(out_ptr);
        let v1: serde_json::Value = serde_json::from_str(json_in).unwrap();
        let v2: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v1, v2);
    }

    #[test]
    fn roundtrip_array() {
        let json_in = r#"[1,2,3]"#;
        let parsed = olive_json_parse(s(json_in));
        let out_ptr = olive_json_stringify(parsed);
        assert_eq!(from_ptr(out_ptr), "[1,2,3]");
    }

    #[test]
    fn parse_invalid_returns_zero() {
        assert_eq!(olive_json_parse(s("{invalid}")), 0);
        assert_eq!(olive_json_parse(0), 0);
    }

    #[test]
    fn nested_object() {
        let json_in = r#"{"a":{"b":99}}"#;
        let parsed = olive_json_parse(s(json_in));
        let obj = unsafe { &*(parsed as *const OliveObj) };
        let inner_ptr = *obj.fields.get("a").unwrap();
        let inner = unsafe { &*(inner_ptr as *const OliveObj) };
        assert_eq!(*inner.fields.get("b").unwrap(), 99);
    }
}
