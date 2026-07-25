//! Small math helpers shared across the sim (and, via [`crate::prelude`], the
//! client) that would otherwise be reimplemented per module.

use std::f32::consts::{PI, TAU};

/// Wrap an angle to `(-PI, PI]`, so a heading/offset never accumulates without
/// bound (which otherwise degrades aiming trigonometry).
pub fn wrap_angle(angle: f32) -> f32 {
    let a = angle.rem_euclid(TAU);
    if a > PI {
        a - TAU
    } else {
        a
    }
}

/// Step a linear-congruential generator, returning the next pseudo-random
/// float in `0.0..1.0`. Callers wanting `-1.0..1.0` (e.g. positional noise)
/// remap with `lcg_next(seed) * 2.0 - 1.0`.
pub fn lcg_next(seed: &mut u32) -> f32 {
    *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    (*seed >> 8) as f32 / (1u32 << 24) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_angle_keeps_values_in_range() {
        assert!((wrap_angle(0.0) - 0.0).abs() < 1e-6);
        assert!((wrap_angle(TAU) - 0.0).abs() < 1e-6);
        assert!((wrap_angle(PI + 0.1) - (0.1 - PI)).abs() < 1e-5);
        assert!((wrap_angle(-PI - 0.1) - (PI - 0.1)).abs() < 1e-5);
    }

    #[test]
    fn lcg_next_stays_in_unit_range_and_varies() {
        let mut seed = 1u32;
        let a = lcg_next(&mut seed);
        let b = lcg_next(&mut seed);
        assert!((0.0..1.0).contains(&a));
        assert!((0.0..1.0).contains(&b));
        assert_ne!(a, b);
    }
}
