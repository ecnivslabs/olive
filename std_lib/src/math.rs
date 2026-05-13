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
