use crate::{
    OliveEnum, OliveObj, StableVec, format_list_elem, olive_str_from_ptr, olive_str_internal,
};

const D_INT: u8 = 1;
const D_FLOAT: u8 = 2;
const D_BOOL: u8 = 3;
const D_STR: u8 = 4;
const D_NULL: u8 = 5;
const D_ANY: u8 = 6;
const D_LIST: u8 = 7;
const D_SET: u8 = 8;
const D_DICT: u8 = 9;
const D_TUPLE: u8 = 10;
const D_OBJ: u8 = 11;
const D_STRUCT: u8 = 12;
const D_ENUM: u8 = 13;

/// Reads a length-prefixed (length biased by 13) string fragment from a
/// descriptor, advancing the cursor past it.
fn read_lp(desc: *const u8, pos: &mut usize) -> String {
    let len = unsafe { byte(desc, *pos) } as usize - 13;
    *pos += 1;
    let mut s = String::with_capacity(len);
    for _ in 0..len {
        s.push(unsafe { byte(desc, *pos) } as char);
        *pos += 1;
    }
    s
}

#[inline]
unsafe fn byte(desc: *const u8, pos: usize) -> u8 {
    unsafe { *desc.add(pos) }
}

/// Advances `pos` over one type-descriptor subtree without rendering anything.
fn skip(desc: *const u8, pos: &mut usize) {
    let tag = unsafe { byte(desc, *pos) };
    *pos += 1;
    match tag {
        D_LIST | D_SET => skip(desc, pos),
        D_DICT => {
            skip(desc, pos);
            skip(desc, pos);
        }
        D_TUPLE => {
            let n = unsafe { byte(desc, *pos) } as usize - 1;
            *pos += 1;
            for _ in 0..n {
                skip(desc, pos);
            }
        }
        D_STRUCT => {
            let _name = read_lp(desc, pos);
            let n = unsafe { byte(desc, *pos) } as usize - 13;
            *pos += 1;
            for _ in 0..n {
                let _f = read_lp(desc, pos);
                skip(desc, pos);
            }
        }
        D_ENUM => {
            let _name = read_lp(desc, pos);
            let n = unsafe { byte(desc, *pos) } as usize - 13;
            *pos += 1;
            for _ in 0..n {
                let _v = read_lp(desc, pos);
                let np = unsafe { byte(desc, *pos) } as usize - 13;
                *pos += 1;
                for _ in 0..np {
                    skip(desc, pos);
                }
            }
        }
        _ => {}
    }
}

/// Renders a value against a static type descriptor emitted by codegen.
///
/// Concrete collections store their elements raw (an `int` is a bare machine
/// word, not an inline-tagged `Any`), so the printer cannot recover the element
/// type from the bytes alone. The descriptor carries the static element type so
/// raw scalars render correctly and nested containers recurse with the right
/// shape.
fn fmt(val: i64, desc: *const u8, pos: &mut usize) -> String {
    let tag = unsafe { byte(desc, *pos) };
    *pos += 1;
    match tag {
        D_INT => format!("{val}"),
        D_FLOAT => crate::fmt_float(f64::from_bits(val as u64)),
        D_BOOL => if val != 0 { "True" } else { "False" }.to_string(),
        D_STR => format!("\"{}\"", olive_str_from_ptr(val)),
        D_NULL => "None".to_string(),
        D_ANY | D_OBJ => format_list_elem(val),
        D_LIST => fmt_seq(val, desc, pos, '[', ']'),
        D_SET => fmt_seq(val, desc, pos, '{', '}'),
        D_TUPLE => fmt_tuple(val, desc, pos),
        D_DICT => fmt_dict(val, desc, pos),
        D_STRUCT => fmt_struct(val, desc, pos),
        D_ENUM => fmt_enum(val, desc, pos),
        _ => format!("{val}"),
    }
}

fn fmt_enum(val: i64, desc: *const u8, pos: &mut usize) -> String {
    let _name = read_lp(desc, pos);
    let n = unsafe { byte(desc, *pos) } as usize - 13;
    *pos += 1;
    let (tag, payload_ptr, payload_len) = if val != 0 {
        let e = unsafe { &*(val as *const OliveEnum) };
        (e.tag as usize, e.payload_ptr, e.payload_len)
    } else {
        (usize::MAX, std::ptr::null_mut(), 0)
    };
    let mut result = String::new();
    for i in 0..n {
        let v_name = read_lp(desc, pos);
        let np = unsafe { byte(desc, *pos) } as usize - 13;
        *pos += 1;
        let mut payloads = Vec::with_capacity(np);
        for j in 0..np {
            let pval = if i == tag && j < payload_len {
                unsafe { *payload_ptr.add(j) }
            } else {
                0
            };
            // `fmt` advances the cursor over this payload's descriptor exactly
            // once, for every variant, so the walk stays aligned.
            let s = fmt(pval, desc, pos);
            if i == tag {
                payloads.push(s);
            }
        }
        if i == tag {
            result = if np == 0 {
                v_name
            } else {
                format!("{v_name}({})", payloads.join(", "))
            };
        }
    }
    result
}

fn fmt_struct(val: i64, desc: *const u8, pos: &mut usize) -> String {
    let name = read_lp(desc, pos);
    let n = unsafe { byte(desc, *pos) } as usize - 13;
    *pos += 1;
    let mut parts = Vec::with_capacity(n);
    for i in 0..n {
        let fname = read_lp(desc, pos);
        let fval = if val != 0 {
            unsafe { *(val as *const i64).add(i + 1) }
        } else {
            0
        };
        let fstr = fmt(fval, desc, pos);
        parts.push(format!("{fname}={fstr}"));
    }
    format!("{name}({})", parts.join(", "))
}

fn fmt_seq(val: i64, desc: *const u8, pos: &mut usize, open: char, close: char) -> String {
    let child = *pos;
    let mut parts = Vec::new();
    if val != 0 {
        let v = unsafe { &*(val as *const StableVec) };
        for i in 0..v.len {
            let elem = unsafe { *v.ptr.add(i) };
            let mut cp = child;
            parts.push(fmt(elem, desc, &mut cp));
        }
    }
    *pos = child;
    skip(desc, pos);
    format!("{open}{}{close}", parts.join(", "))
}

fn fmt_tuple(val: i64, desc: *const u8, pos: &mut usize) -> String {
    let n = unsafe { byte(desc, *pos) } as usize - 1;
    *pos += 1;
    let v = (val != 0).then(|| unsafe { &*(val as *const StableVec) });
    let mut parts = Vec::with_capacity(n);
    for i in 0..n {
        let elem = v.map(|v| unsafe { *v.ptr.add(i) }).unwrap_or(0);
        parts.push(fmt(elem, desc, pos));
    }
    if n == 1 {
        format!("({},)", parts[0])
    } else {
        format!("({})", parts.join(", "))
    }
}

fn fmt_dict(val: i64, desc: *const u8, pos: &mut usize) -> String {
    let key_start = *pos;
    let mut val_start = key_start;
    skip(desc, &mut val_start);
    let mut end = val_start;
    skip(desc, &mut end);
    let mut parts = Vec::new();
    if val != 0 {
        let m = unsafe { &*(val as *const OliveObj) };
        for (k, &v) in &m.fields {
            let mut kp = key_start;
            let key = fmt(k.0, desc, &mut kp);
            let mut vp = val_start;
            let value = fmt(v, desc, &mut vp);
            parts.push(format!("{key}: {value}"));
        }
    }
    *pos = end;
    format!("{{{}}}", parts.join(", "))
}

struct Spec {
    fill: char,
    align: Option<char>,
    sign: char,
    alt: bool,
    zero: bool,
    width: usize,
    grouping: Option<char>,
    precision: Option<usize>,
    ty: Option<char>,
}

impl Spec {
    fn parse(s: &str) -> Spec {
        let chars: Vec<char> = s.chars().collect();
        let mut i = 0;
        let mut spec = Spec {
            fill: ' ',
            align: None,
            sign: '-',
            alt: false,
            zero: false,
            width: 0,
            grouping: None,
            precision: None,
            ty: None,
        };
        if chars.len() >= 2 && matches!(chars[1], '<' | '>' | '=' | '^') {
            spec.fill = chars[0];
            spec.align = Some(chars[1]);
            i = 2;
        } else if !chars.is_empty() && matches!(chars[0], '<' | '>' | '=' | '^') {
            spec.align = Some(chars[0]);
            i = 1;
        }
        if i < chars.len() && matches!(chars[i], '+' | '-' | ' ') {
            spec.sign = chars[i];
            i += 1;
        }
        if i < chars.len() && chars[i] == '#' {
            spec.alt = true;
            i += 1;
        }
        if i < chars.len() && chars[i] == '0' {
            spec.zero = true;
            if spec.align.is_none() {
                spec.align = Some('=');
                spec.fill = '0';
            }
            i += 1;
        }
        let w0 = i;
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
        if i > w0 {
            spec.width = chars[w0..i].iter().collect::<String>().parse().unwrap_or(0);
        }
        if i < chars.len() && matches!(chars[i], ',' | '_') {
            spec.grouping = Some(chars[i]);
            i += 1;
        }
        if i < chars.len() && chars[i] == '.' {
            i += 1;
            let p0 = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            spec.precision = Some(chars[p0..i].iter().collect::<String>().parse().unwrap_or(0));
        }
        if i < chars.len() {
            spec.ty = Some(chars[i]);
        }
        spec
    }
}

fn group_digits(digits: &str, sep: char, size: usize) -> String {
    let bytes: Vec<char> = digits.chars().collect();
    let mut out = Vec::new();
    for (idx, c) in bytes.iter().rev().enumerate() {
        if idx > 0 && idx % size == 0 {
            out.push(sep);
        }
        out.push(*c);
    }
    out.iter().rev().collect()
}

fn pad(body: String, sign: &str, prefix: &str, spec: &Spec) -> String {
    let content_len = sign.len() + prefix.len() + body.chars().count();
    if content_len >= spec.width {
        return format!("{sign}{prefix}{body}");
    }
    let fill_count = spec.width - content_len;
    let fill: String = std::iter::repeat_n(spec.fill, fill_count).collect();
    match spec.align.unwrap_or('>') {
        '<' => format!("{sign}{prefix}{body}{fill}"),
        '^' => {
            let left = fill_count / 2;
            let lf: String = std::iter::repeat_n(spec.fill, left).collect();
            let rf: String = std::iter::repeat_n(spec.fill, fill_count - left).collect();
            format!("{lf}{sign}{prefix}{body}{rf}")
        }
        '=' => format!("{sign}{prefix}{fill}{body}"),
        _ => format!("{fill}{sign}{prefix}{body}"),
    }
}

fn sign_str(neg: bool, spec: &Spec) -> &'static str {
    if neg {
        "-"
    } else {
        match spec.sign {
            '+' => "+",
            ' ' => " ",
            _ => "",
        }
    }
}

fn fmt_spec_int(v: i64, spec: &Spec) -> String {
    let neg = v < 0;
    let mag = (v as i128).unsigned_abs();
    let (mut digits, prefix, group_size) = match spec.ty {
        Some('b') => (format!("{mag:b}"), if spec.alt { "0b" } else { "" }, 4),
        Some('o') => (format!("{mag:o}"), if spec.alt { "0o" } else { "" }, 4),
        Some('x') => (format!("{mag:x}"), if spec.alt { "0x" } else { "" }, 4),
        Some('X') => (format!("{mag:X}"), if spec.alt { "0X" } else { "" }, 4),
        Some('c') => {
            return pad(
                char::from_u32(v as u32)
                    .map(String::from)
                    .unwrap_or_default(),
                "",
                "",
                spec,
            );
        }
        _ => (format!("{mag}"), "", 3),
    };
    if let Some(sep) = spec.grouping {
        digits = group_digits(&digits, sep, group_size);
    }
    pad(digits, sign_str(neg, spec), prefix, spec)
}

fn fmt_spec_float(v: f64, spec: &Spec) -> String {
    let neg = v.is_sign_negative() && (v != 0.0 || spec.sign != '-');
    let mag = v.abs();
    let prec = spec.precision.unwrap_or(6);
    let mut body = match spec.ty {
        Some('e') => format!("{mag:.prec$e}"),
        Some('E') => format!("{mag:.prec$E}"),
        Some('%') => format!("{:.*}%", prec, mag * 100.0),
        Some('g') | Some('G') => {
            let p = spec.precision.unwrap_or(6).max(1);
            format!("{mag:.*}", p)
        }
        _ => {
            if spec.precision.is_some() || spec.ty == Some('f') || spec.ty == Some('F') {
                format!("{mag:.prec$}")
            } else {
                format!("{mag}")
            }
        }
    };
    if let Some(sep) = spec.grouping {
        let (int_part, frac) = match body.split_once('.') {
            Some((a, b)) => (a.to_string(), format!(".{b}")),
            None => (body.clone(), String::new()),
        };
        body = format!("{}{}", group_digits(&int_part, sep, 3), frac);
    }
    pad(body, sign_str(neg, spec), "", spec)
}

fn fmt_spec_str(s: &str, spec: &Spec) -> String {
    let trimmed: String = match spec.precision {
        Some(p) => s.chars().take(p).collect(),
        None => s.to_string(),
    };
    let mut spec = Spec {
        fill: spec.fill,
        align: Some(spec.align.unwrap_or('<')),
        ..*spec
    };
    spec.zero = false;
    pad(trimmed, "", "", &spec)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_format_int(val: i64, spec: i64) -> i64 {
    olive_str_internal(&fmt_spec_int(val, &Spec::parse(&olive_str_from_ptr(spec))))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_format_float(val: f64, spec: i64) -> i64 {
    olive_str_internal(&fmt_spec_float(
        val,
        &Spec::parse(&olive_str_from_ptr(spec)),
    ))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_format_str(val: i64, spec: i64) -> i64 {
    olive_str_internal(&fmt_spec_str(
        &olive_str_from_ptr(val),
        &Spec::parse(&olive_str_from_ptr(spec)),
    ))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_format_bool(val: i64, spec: i64) -> i64 {
    let s = if val != 0 { "True" } else { "False" };
    olive_str_internal(&fmt_spec_str(s, &Spec::parse(&olive_str_from_ptr(spec))))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_format_any(val: i64, spec: i64) -> i64 {
    let s = Spec::parse(&olive_str_from_ptr(spec));
    match val & crate::boxed::TAG_MASK {
        crate::boxed::TAG_INT => return olive_str_internal(&fmt_spec_int(val >> 3, &s)),
        crate::boxed::TAG_BOOL => {
            let b = if val >> 3 != 0 { "True" } else { "False" };
            return olive_str_internal(&fmt_spec_str(b, &s));
        }
        crate::boxed::TAG_NULL => return olive_str_internal(&fmt_spec_str("None", &s)),
        _ => {}
    }
    if val & 1 == 1 && (val & !1) > 0x10000 {
        return olive_str_internal(&fmt_spec_str(&olive_str_from_ptr(val), &s));
    }
    olive_str_internal(&fmt_spec_str(&format_list_elem(val), &s))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_format_typed(val: i64, desc: i64) -> i64 {
    let mut pos = 0usize;
    olive_str_internal(&fmt(val, desc as *const u8, &mut pos))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_print_typed(val: i64, desc: i64) -> i64 {
    let mut pos = 0usize;
    println!("{}", fmt(val, desc as *const u8, &mut pos));
    0
}
