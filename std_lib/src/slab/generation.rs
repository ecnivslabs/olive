use std::sync::atomic::{AtomicU64, Ordering};

const COUNTER_BITS: u32 = 16;
const COUNTER_MASK: u64 = (1 << COUNTER_BITS) - 1;
const SHARED_BIT: u64 = 1 << 63;
const EPOCH_LIMIT: u64 = 1 << (63 - COUNTER_BITS);
static NEXT_EPOCH: AtomicU64 = AtomicU64::new(1);

// The high bit remains reserved. A new epoch is issued per chunk, or when
// a slot's local counter rolls over, so retired addresses cannot resurrect
// old borrows. Ordinary slot reuse requires no global atomic operation.
pub(super) fn fresh_generation() -> u64 {
    let mut epoch = NEXT_EPOCH.load(Ordering::Relaxed);
    loop {
        assert!(
            epoch < EPOCH_LIMIT,
            "olive: allocation generation space exhausted"
        );
        match NEXT_EPOCH.compare_exchange_weak(
            epoch,
            epoch + 1,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return (epoch << COUNTER_BITS) | 1,
            Err(current) => epoch = current,
        }
    }
}

#[inline]
pub(crate) fn advance_generation(generation: u64, step: u64) -> u64 {
    debug_assert!(step == 1 || step == 2);
    let counter = (generation & COUNTER_MASK) + step;
    if counter <= COUNTER_MASK {
        generation + step
    } else {
        (fresh_generation() & !COUNTER_MASK) | (counter & COUNTER_MASK) | (generation & SHARED_BIT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollover_reserves_a_new_epoch_and_preserves_parity() {
        for step in [1, 2] {
            let previous = fresh_generation() | COUNTER_MASK;
            let other = fresh_generation();
            let next = advance_generation(previous, step);
            assert_ne!(next >> COUNTER_BITS, previous >> COUNTER_BITS);
            assert_ne!(next >> COUNTER_BITS, other >> COUNTER_BITS);
            assert_eq!(next & COUNTER_MASK, step - 1);
            assert_eq!(next & SHARED_BIT, 0);
        }
    }

    #[test]
    fn advancing_preserves_the_reserved_shared_bit() {
        for counter in [1, COUNTER_MASK] {
            let previous = (fresh_generation() & !COUNTER_MASK) | counter | SHARED_BIT;
            let next = advance_generation(previous, 2);
            assert_eq!(next & SHARED_BIT, SHARED_BIT);
            assert_eq!(next & 1, 1);
        }
    }

    #[test]
    fn slab_recycling_rolls_over_without_reviving_an_old_borrow() {
        let mut slab = super::super::GenSlab::new(8);
        let body = slab.alloc().0;
        let old = fresh_generation() | COUNTER_MASK;
        unsafe { (*(body as *mut AtomicU64).sub(1)).store(old, Ordering::Relaxed) };
        assert!(slab.free(body));
        assert_eq!(slab.alloc(), (body, false));
        let current = super::super::slot_generation(body as i64);
        assert_ne!(current >> COUNTER_BITS, old >> COUNTER_BITS);
        assert_eq!(current & 1, 1);
        assert_eq!(
            crate::struct_obj::olive_struct_gen_stale(body as i64, old as i64),
            1
        );
    }
}
