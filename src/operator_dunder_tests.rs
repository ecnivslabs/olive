//! E6.1 regression tests (roadmap.md Phase E6.1): struct arithmetic and
//! comparison dunders. `a + b` on a struct with `__add__` used to dump core
//! (E1 step 13); these are the regression tests for that fix plus the new
//! `__eq__`/`__lt__` derivation.
#[cfg(test)]
use crate::test_utils::{call_i64, check_codes, compile};

const VEC2: &str = concat!(
    "struct Vec2:\n",
    "    x: float\n",
    "    y: float\n",
    "\n",
    "impl Vec2:\n",
    "    fn __add__(self, other: Vec2) -> Vec2:\n",
    "        Vec2(self.x + other.x, self.y + other.y)\n",
    "    fn __sub__(self, other: Vec2) -> Vec2:\n",
    "        Vec2(self.x - other.x, self.y - other.y)\n",
    "    fn __mul__(self, other: Vec2) -> Vec2:\n",
    "        Vec2(self.x * other.x, self.y * other.y)\n",
    "    fn __eq__(self: &Vec2, other: &Vec2) -> bool:\n",
    "        self.x == other.x and self.y == other.y\n",
    "    fn __lt__(self: &Vec2, other: &Vec2) -> bool:\n",
    "        (self.x * self.x + self.y * self.y) < (other.x * other.x + other.y * other.y)\n",
);

#[test]
fn struct_add_no_longer_dumps_core() {
    let mut cg = compile(&format!(
        "{VEC2}\nfn f() -> int:\n    let v = Vec2(1.0, 2.0) + Vec2(3.0, 4.0)\n    if v.x == 4.0 and v.y == 6.0:\n        return 1\n    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn struct_sub_and_mul() {
    let mut cg = compile(&format!(
        "{VEC2}\nfn f() -> int:\n    let a = Vec2(3.0, 4.0) - Vec2(1.0, 1.0)\n    let b = Vec2(2.0, 3.0) * Vec2(2.0, 2.0)\n    if a.x == 2.0 and a.y == 3.0 and b.x == 4.0 and b.y == 6.0:\n        return 1\n    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn struct_eq_no_longer_prints_false_for_equal_values() {
    let mut cg = compile(&format!(
        "{VEC2}\nfn f() -> int:\n    let a = Vec2(1.0, 1.0)\n    let b = Vec2(1.0, 1.0)\n    if a == b and not (a != b):\n        return 1\n    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn struct_eq_negation_on_unequal_values() {
    let mut cg = compile(&format!(
        "{VEC2}\nfn f() -> int:\n    let a = Vec2(1.0, 1.0)\n    let c = Vec2(9.0, 9.0)\n    if not (a == c) and a != c:\n        return 1\n    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn struct_lt_derives_gt_le_ge() {
    let mut cg = compile(&format!(
        "{VEC2}\nfn f() -> int:\n    let a = Vec2(1.0, 1.0)\n    let b = Vec2(1.0, 1.0)\n    let c = Vec2(5.0, 5.0)\n    if a < c and c > a and a <= b and c >= a and c > a and not (c < a):\n        return 1\n    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn missing_arith_dunder_is_clean_rejection() {
    let codes = check_codes(concat!(
        "struct P:\n",
        "    x: int\n",
        "fn f():\n",
        "    let a = P(1)\n",
        "    let b = P(2)\n",
        "    a + b\n",
    ));
    assert_eq!(codes, vec!["E0404"]);
}

#[test]
fn missing_lt_dunder_is_clean_rejection() {
    let codes = check_codes(concat!(
        "struct P:\n",
        "    x: int\n",
        "fn f():\n",
        "    let a = P(1)\n",
        "    let b = P(2)\n",
        "    a < b\n",
    ));
    assert_eq!(codes, vec!["E0404"]);
}

#[test]
fn user_defined_gt_is_rejected_derives_only() {
    let codes = check_codes(concat!(
        "struct P:\n",
        "    x: int\n",
        "impl P:\n",
        "    fn __gt__(self: &P, other: &P) -> bool:\n",
        "        self.x > other.x\n",
    ));
    assert_eq!(codes, vec!["E0404"]);
}

#[test]
fn cmp_dunder_bare_self_is_rejected() {
    let codes = check_codes(concat!(
        "struct P:\n",
        "    x: int\n",
        "impl P:\n",
        "    fn __eq__(self, other: P) -> bool:\n",
        "        self.x == other.x\n",
    ));
    assert_eq!(codes, vec!["E0404"]);
}

#[test]
fn unsupported_dunder_still_rejected() {
    let codes = check_codes(concat!(
        "struct P:\n",
        "    x: int\n",
        "impl P:\n",
        "    fn __call__(self):\n",
        "        pass\n",
    ));
    assert_eq!(codes, vec!["E0404"]);
}
