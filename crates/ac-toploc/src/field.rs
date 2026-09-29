//! Arithmetic modulo the prime 65,497 and Newton interpolation, as in the reference
//! implementation's `ndd.cpp` (`MOD_N`, `compute_newton_coefficients`, `evaluate_polynomial`).
//!
//! Every value is kept in `0..P` as a `u32`; products fit in `u64`.

use alloc::vec;
use alloc::vec::Vec;

/// The prime modulus of the reference implementation.
pub const P: u32 = 65_497;

const P64: u64 = 65_497;

/// `v mod P` of any 64-bit value.
fn reduce(v: u64) -> u32 {
    // The remainder is below P < 2^32.
    u32::try_from(v % P64).unwrap_or(0)
}

/// `a + b mod P` of reduced values.
fn add(a: u32, b: u32) -> u32 {
    reduce(u64::from(a).saturating_add(u64::from(b)))
}

/// `a − b mod P` of reduced values.
fn sub(a: u32, b: u32) -> u32 {
    reduce(
        u64::from(a)
            .saturating_add(P64)
            .saturating_sub(u64::from(b)),
    )
}

/// `a × b mod P` of reduced values.
fn mul(a: u32, b: u32) -> u32 {
    reduce(u64::from(a).saturating_mul(u64::from(b)))
}

/// Inverse of a nonzero reduced value (extended Euclid, as `modInverse` in the reference).
pub(crate) fn inv(a: u32) -> Option<u32> {
    if a == 0 {
        return None;
    }
    let (mut t, mut new_t) = (0i64, 1i64);
    let (mut r, mut new_r) = (i64::from(P), i64::from(a));
    while new_r != 0 {
        let q = r.checked_div(new_r)?;
        (t, new_t) = (new_t, t.checked_sub(q.checked_mul(new_t)?)?);
        (r, new_r) = (new_r, r.checked_sub(q.checked_mul(new_r)?)?);
    }
    if r != 1 {
        return None;
    }
    let t = t.rem_euclid(i64::from(P));
    u32::try_from(t).ok()
}

/// Coefficients, in ascending powers and reduced mod P, of the polynomial of degree below
/// `x.len()` through the points `(x[i], y[i])`: Newton divided differences expanded into
/// standard form in one pass, exactly as the reference does.
///
/// `None` if the lists differ in length, are empty, or two `x` coincide mod P.
pub(crate) fn interpolate(x: &[u32], y: &[u32]) -> Option<Vec<u32>> {
    let n = x.len();
    if n == 0 || y.len() != n {
        return None;
    }
    let xs: Vec<u32> = x.iter().map(|v| reduce(u64::from(*v))).collect();
    let mut dd: Vec<u32> = y.iter().map(|v| reduce(u64::from(*v))).collect();
    for k in 1..n {
        for i in (k..n).rev() {
            let num = sub(*dd.get(i)?, *dd.get(i.checked_sub(1)?)?);
            let den = sub(*xs.get(i)?, *xs.get(i.checked_sub(k)?)?);
            *dd.get_mut(i)? = mul(num, inv(den)?);
        }
    }
    // coeffs += dd[i] · Π_{j<i} (X − x[j]), with the product kept in `factor`.
    let mut coeffs = vec![0u32; n];
    let mut factor = vec![0u32; n];
    *factor.get_mut(0)? = 1;
    for i in 0..n {
        let d = *dd.get(i)?;
        for j in 0..=i {
            let c = coeffs.get_mut(j)?;
            *c = add(*c, mul(d, *factor.get(j)?));
        }
        if i.checked_add(1)? < n {
            // factor ← factor · (X − x[i])
            let minus_xi = sub(0, *xs.get(i)?);
            let mut prev = *factor.first()?;
            *factor.first_mut()? = mul(prev, minus_xi);
            for k in 1..=i.checked_add(1)? {
                let old = *factor.get(k)?;
                *factor.get_mut(k)? = add(prev, mul(old, minus_xi));
                prev = old;
            }
        }
    }
    Some(coeffs)
}

/// Value at `x` of the polynomial with `coeffs` (ascending powers), reduced mod P (Horner, as
/// `evaluate_polynomial` in the reference). `None` without coefficients.
pub(crate) fn evaluate(coeffs: &[u32], x: u32) -> Option<u32> {
    let (last, rest) = coeffs.split_last()?;
    let x = reduce(u64::from(x));
    let mut acc = reduce(u64::from(*last));
    for c in rest.iter().rev() {
        acc = add(mul(acc, x), reduce(u64::from(*c)));
    }
    Some(acc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn inverses() {
        assert_eq!(inv(0), None);
        assert_eq!(inv(1), Some(1));
        for a in [2u32, 3, 1_000, 32_768, P - 1] {
            let i = inv(a).unwrap();
            assert_eq!(mul(a, i), 1, "{a}");
        }
        // P − 1 is its own inverse.
        assert_eq!(inv(P - 1), Some(P - 1));
    }

    #[test]
    fn small_interpolation_matches_hand_computation() {
        // Through (0, 1), (1, 3), (2, 7): 1 + X + X².
        assert_eq!(interpolate(&[0, 1, 2], &[1, 3, 7]), Some(vec![1, 1, 1]));
        // A constant.
        assert_eq!(interpolate(&[5], &[9]), Some(vec![9]));
        assert_eq!(evaluate(&[1, 1, 1], 10), Some(111));
    }

    #[test]
    fn bad_inputs_are_refused() {
        assert_eq!(interpolate(&[], &[]), None);
        assert_eq!(interpolate(&[1, 2], &[1]), None);
        // x values that coincide mod P.
        assert_eq!(interpolate(&[1, 1 + P], &[1, 2]), None);
        assert_eq!(evaluate(&[], 3), None);
    }

    proptest! {
        #[test]
        fn interpolation_passes_through_every_point(
            points in proptest::collection::btree_map(0u32..P, 0u32..P, 1..64),
        ) {
            let (x, y): (Vec<u32>, Vec<u32>) = points.into_iter().unzip();
            let c = interpolate(&x, &y).unwrap();
            prop_assert_eq!(c.len(), x.len());
            for (xi, yi) in x.iter().zip(&y) {
                prop_assert_eq!(evaluate(&c, *xi), Some(*yi));
            }
        }
    }
}
