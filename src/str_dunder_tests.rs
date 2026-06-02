//! E6.2 regression tests (roadmap.md Phase E6.2): `__str__` wires into
//! `str()`, `print`, and f-strings via `lower_struct_str_call`, one path
//! shared by all three call sites.
#[cfg(test)]
use crate::test_utils::{call_i64, check_codes, compile};

const POINT: &str = concat!(
    "struct Point:\n",
    "    x: int\n",
    "    y: int\n",
    "\n",
    "impl Point:\n",
    "    fn __str__(self) -> str:\n",
    "        f\"({self.x}, {self.y})\"\n",
);

#[test]
fn str_call_honors_user_str() {
    let mut cg = compile(&format!(
        "{POINT}\nfn f() -> int:\n    let p = Point(3, 4)\n    if str(p) == \"(3, 4)\":\n        return 1\n    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn fstring_interpolation_honors_user_str() {
    let mut cg = compile(&format!(
        "{POINT}\nfn f() -> int:\n    let p = Point(3, 4)\n    if f\"point: {{p}}, done\" == \"point: (3, 4), done\":\n        return 1\n    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn struct_without_str_falls_back_to_auto_repr() {
    let mut cg = compile(concat!(
        "struct Plain:\n",
        "    n: int\n",
        "fn f() -> int:\n",
        "    let q = Plain(9)\n",
        "    if str(q) == \"Plain(n=9)\":\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn wrong_str_return_type_is_clean_rejection() {
    let codes = check_codes(concat!(
        "struct P:\n",
        "    x: int\n",
        "impl P:\n",
        "    fn __str__(self) -> int:\n",
        "        self.x\n",
    ));
    assert_eq!(codes, vec!["E0404"]);
}
