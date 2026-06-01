#[unsafe(no_mangle)]
pub extern "C" fn olive_math_sin(x: f64) -> f64 {
    x.sin()
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_math_cos(x: f64) -> f64 {
    x.cos()
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_math_tan(x: f64) -> f64 {
    x.tan()
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_math_asin(x: f64) -> f64 {
    x.asin()
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_math_acos(x: f64) -> f64 {
    x.acos()
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_math_atan(x: f64) -> f64 {
    x.atan()
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_math_atan2(y: f64, x: f64) -> f64 {
    y.atan2(x)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_math_log(x: f64) -> f64 {
    x.ln()
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_math_log10(x: f64) -> f64 {
    x.log10()
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_math_exp(x: f64) -> f64 {
    x.exp()
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_math_abs(x: f64) -> f64 {
    x.abs()
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_int_abs(x: i64) -> i64 {
    x.abs()
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_math_round_to_int(x: f64) -> i64 {
    x.round() as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_math_round_with_digits(x: f64, ndigits: i64) -> f64 {
    if ndigits == 0 {
        x.round()
    } else {
        let factor = 10f64.powi(ndigits as i32);
        (x * factor).round() / factor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-10
    }

    #[test]
    fn sin_pi() {
        assert!(approx_eq(olive_math_sin(0.0), 0.0));
    }

    #[test]
    fn sin_half_pi() {
        assert!(approx_eq(olive_math_sin(std::f64::consts::FRAC_PI_2), 1.0));
    }

    #[test]
    fn cos_zero() {
        assert!(approx_eq(olive_math_cos(0.0), 1.0));
    }

    #[test]
    fn tan_zero() {
        assert!(approx_eq(olive_math_tan(0.0), 0.0));
    }

    #[test]
    fn asin_one() {
        assert!(approx_eq(olive_math_asin(1.0), std::f64::consts::FRAC_PI_2));
    }

    #[test]
    fn acos_one() {
        assert!(approx_eq(olive_math_acos(1.0), 0.0));
    }

    #[test]
    fn atan_zero() {
        assert!(approx_eq(olive_math_atan(0.0), 0.0));
    }

    #[test]
    fn atan2_positive() {
        assert!(approx_eq(
            olive_math_atan2(1.0, 1.0),
            std::f64::consts::FRAC_PI_4
        ));
    }

    #[test]
    fn log_e() {
        assert!(approx_eq(olive_math_log(std::f64::consts::E), 1.0));
    }

    #[test]
    fn log10_ten() {
        assert!(approx_eq(olive_math_log10(10.0), 1.0));
    }

    #[test]
    fn exp_zero() {
        assert!(approx_eq(olive_math_exp(0.0), 1.0));
    }

    #[test]
    fn exp_one() {
        assert!(approx_eq(olive_math_exp(1.0), std::f64::consts::E));
    }

    #[test]
    fn sin_negative() {
        assert!(approx_eq(olive_math_sin(-0.5), (-0.5f64).sin()));
    }

    #[test]
    fn cos_large_value() {
        let x = 1000.0;
        assert!(approx_eq(olive_math_cos(x), x.cos()));
    }
}
