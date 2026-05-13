use rand::prelude::*;
use std::sync::{Mutex, OnceLock};

static RNG: OnceLock<Mutex<StdRng>> = OnceLock::new();

fn get_rng() -> &'static Mutex<StdRng> {
    RNG.get_or_init(|| Mutex::new(StdRng::from_entropy()))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_random_seed(seed: i64) {
    let mut rng = get_rng().lock().unwrap();
    *rng = StdRng::seed_from_u64(seed as u64);
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_random_get() -> f64 {
    let mut rng = get_rng().lock().unwrap();
    rng.r#gen()
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_random_int(min: i64, max: i64) -> i64 {
    if min >= max {
        return min;
    }
    let mut rng = get_rng().lock().unwrap();
    rng.r#gen_range(min..max)
}
