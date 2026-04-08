use rand::prelude::*;
use std::cell::RefCell;

thread_local! {
    static RNG: RefCell<StdRng> = RefCell::new(StdRng::from_entropy());
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_random_seed(seed: i64) {
    RNG.with(|r| *r.borrow_mut() = StdRng::seed_from_u64(seed as u64));
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_random_get() -> f64 {
    RNG.with(|r| r.borrow_mut().r#gen())
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_random_int(min: i64, max: i64) -> i64 {
    if min >= max {
        return min;
    }
    RNG.with(|r| r.borrow_mut().gen_range(min..max))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_get_in_range() {
        for _ in 0..100 {
            let v = olive_random_get();
            assert!((0.0..1.0).contains(&v));
        }
    }

    #[test]
    fn random_int_in_range() {
        for _ in 0..100 {
            let v = olive_random_int(10, 20);
            assert!((10..20).contains(&v));
        }
    }

    #[test]
    fn random_int_equal_bounds() {
        assert_eq!(olive_random_int(5, 5), 5);
    }

    #[test]
    fn random_seed_reproducible() {
        olive_random_seed(42);
        let a = olive_random_get();
        olive_random_seed(42);
        let b = olive_random_get();
        assert_eq!(a, b);
    }

    #[test]
    fn random_no_panic_under_threads() {
        let handles: Vec<_> = (0..8)
            .map(|_| std::thread::spawn(|| (olive_random_get(), olive_random_int(0, 1000))))
            .collect();
        for h in handles {
            let (f, i) = h.join().unwrap();
            assert!((0.0..1.0).contains(&f));
            assert!((0..1000).contains(&i));
        }
    }
}
