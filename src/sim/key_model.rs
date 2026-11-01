//! Contact bounce for the scripted keys (survey 2.7, K1). A mechanical key's edge is a train of 3 or 5 edges in
//! the 10 ms after it (an odd count, so it settles on the new level); stock debounces 15 ms after an EXTI falling
//! edge. The count and the 10 ms are typical figures for tact switches, **inferred**, not measured on the unit's keys.
//! The keys' power-button line (PA0 with PA1, K2) is not modelled: nothing in BOARD.md says how they combine.

extern crate std;

/// A key script with bounce on every edge of `f`'s mask bits.
pub fn bounce(f: impl Fn(u64) -> u8 + 'static) -> impl Fn(u64) -> u8 + 'static {
    move |t| {
        let now = f(t);
        let mut mask = now;
        for d in 0..10u64 {
            let Some(e) = t.checked_sub(d) else { break };
            let (after, before) = (f(e), e.checked_sub(1).map_or(0, &f));
            for bit in 0..8 {
                if (after ^ before) >> bit & 1 == 0 {
                    continue;
                }
                let k = if ((e * 2_654_435_761) >> 7) & 1 == 0 { 3 } else { 5 }; // edges in this train
                let n = (d * k) / 10 + 1; // edges seen by t
                let level = if n % 2 == 1 { after } else { before };
                mask = mask & !(1 << bit) | (level & (1 << bit));
            }
        }
        mask
    }
}

#[cfg(test)]
mod tests {
    use super::bounce;
    use super::std::vec::Vec;

    #[test]
    fn a_press_is_a_train_of_edges_that_settles() {
        let f = bounce(|t| if t >= 100 { 4 } else { 0 });
        let seen: Vec<u8> = (99..112).map(&f).collect();
        let edges = seen.windows(2).filter(|w| w[0] != w[1]).count();
        assert!(edges >= 3, "{seen:?}");
        assert_eq!((f(98), f(110), f(500)), (0, 4, 4), "settled before and after");
        assert_eq!(bounce(|_| 4)(7), 4, "no edge, no bounce");
    }
}
