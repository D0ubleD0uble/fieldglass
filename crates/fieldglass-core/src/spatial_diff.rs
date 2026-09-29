//! Inverse spatial differencing: rebuilding a run of packed integers from the
//! differences a GRIB encoder stored in their place.
//!
//! An encoder that differences a field of order `k` keeps the first `k`
//! values as they are (the *seeds*) and replaces each later value with its
//! `k`-th difference, reduced by an overall minimum, the *bias*. Decoding runs
//! the recurrence the other way. With `g` the rebuilt values and `d` the stored
//! differences, value `i` past the seeds is
//!
//! - order 1: `d[i] + g[i-1] + bias`;
//! - order 2: `d[i] + 2·g[i-1] − g[i-2] + bias`;
//! - order 3: `d[i] + 3·g[i-1] − 3·g[i-2] + g[i-3] + bias`.
//!
//! Every operation wraps in two's complement, as eccodes' C does. That is
//! deliberate: a malformed bias or seed overflows an `i64`, and eccodes' answer
//! for such input is the wrapped one. It also means no input can make the
//! reconstruction panic.
//!
//! Both GRIB editions use it. GRIB1 second-order packing (orders 0 to 3) and
//! the GRIB2 second-order templates 5.50001 and 5.50002 store the seeds in the
//! first slots of the run, and call `apply_spd_inverse`. GRIB2 complex packing
//! with spatial differencing (template 5.3, orders 1 and 2) can mark points
//! missing inside the run, which take no part in the recurrence, and carries
//! its seeds separately, so it calls `apply_spd_inverse_skipping_missing`. The
//! two share `next_value`, the single statement of the recurrence.
//!
//! eccodes writes the GRIB1 reconstruction with running accumulators (for
//! order 2, `y += d + bias; z += y`) and the GRIB2 one as above. In wrapping
//! arithmetic the two are the same function of the input, since integers
//! modulo `2^64` form a ring, so one form serves both editions and decodes
//! each to the bit.
//!
//! # Verified kernel
//!
//! This file is compiled twice. This crate compiles it as ordinary Rust; the
//! verification crate `crates/fieldglass-verify` includes the same file with
//! `#[path]` and proves it with Verus. The proofs are the
//! `cfg_attr(verus_keep_ghost, ...)` attributes and the `verus_keep_ghost`
//! items at the bottom, which a normal build never sees. What is proved is
//! listed in `docs/verification.md`. The file names only `crate::FieldglassError`,
//! which both crates provide, and its docs use plain backticks rather than
//! intra-doc links, which would resolve in only one of the two.

use crate::FieldglassError;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

/// Inverse spatial differencing of order `order`, in place, over a run whose
/// first `order` slots hold the seeds and whose other slots hold the stored
/// differences. See the module docs for the recurrence.
///
/// A run shorter than `order` is all seeds and is left as it is. Order 0 is no
/// differencing. An order above 3 is not defined by WMO Code Table 5.6 and is
/// an error, which leaves `x` untouched.
///
/// Proved (see the module docs): `Ok` exactly when `order` is at most 3; then
/// the seeds are unchanged and every later slot is `next_value` of its stored
/// difference and the values already rebuilt before it, in wrapping
/// arithmetic. No length or order makes it index out of bounds or panic.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r is Ok <==> order <= 3,
        r is Err ==> final(x)@ == old(x)@,
        final(x)@.len() == old(x)@.len(),
        r is Ok ==> forall|i: int| 0 <= i < old(x)@.len() ==> #[trigger] final(x)@[i] == if order == 0 || i < order {
            old(x)@[i]
        } else {
            rebuilt(order as int, bias, old(x)@[i], final(x)@, i)
        },
))]
pub fn apply_spd_inverse(x: &mut [i64], order: u8, bias: i64) -> Result<(), FieldglassError> {
    let k = order as usize;
    if k > 3 {
        return Err(unsupported_order(k));
    }
    if k == 0 {
        return Ok(());
    }
    let n = x.len();
    let mut i = k;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            1 <= k <= 3,
            k == order,
            n == x@.len(),
            n == old(x)@.len(),
            k <= i,
            forall|j: int| i <= j < n ==> #[trigger] x@[j] == old(x)@[j],
            forall|j: int| 0 <= j < i && j < n ==> #[trigger] x@[j] == if j < k {
                old(x)@[j]
            } else {
                rebuilt(k as int, bias, old(x)@[j], x@, j)
            },
        decreases n - i,
    ))]
    while i < n {
        let g1 = x[i - 1];
        let g2 = if k >= 2 { x[i - 2] } else { 0 };
        let g3 = if k >= 3 { x[i - 3] } else { 0 };
        x[i] = next_value(order, bias, x[i], g1, g2, g3);
        i += 1;
    }
    Ok(())
}

/// Inverse spatial differencing of order `seeds.len()`, in place, over a run in
/// which `None` marks a missing point: GRIB2 complex packing with spatial
/// differencing (template 5.3).
///
/// Missing points take no part in the recurrence, as in eccodes'
/// `DataG22OrderPacking`: the seeds replace the first `seeds.len()` present
/// values, and each later present value is rebuilt from its stored difference
/// and the nearest present values before it. A run with fewer present points
/// than seeds takes as many seeds as it has room for. Missing points stay
/// missing. More than 3 seeds is an order WMO Code Table 5.6 does not define,
/// and an error that leaves `vals` untouched.
///
/// Proved (see the module docs): `Ok` exactly when there are at most 3 seeds;
/// then a point is present afterwards exactly when it was before, and the
/// present values, read in order, are the seeds followed by `next_value` of
/// each stored difference and the present values rebuilt before it. No
/// length, seed count or pattern of missing points makes it index out of
/// bounds or panic.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r is Ok <==> seeds@.len() <= 3,
        r is Err ==> final(vals)@ == old(vals)@,
        final(vals)@.len() == old(vals)@.len(),
        r is Ok ==> forall|i: int|
            0 <= i < old(vals)@.len() ==> (#[trigger] final(vals)@[i] is Some <==> old(vals)@[i] is Some),
        r is Ok ==> present(final(vals)@).len() == present(old(vals)@).len(),
        r is Ok ==> forall|j: int| 0 <= j < present(old(vals)@).len() ==>
            #[trigger] present(final(vals)@)[j] == if seeds@.len() == 0 {
                present(old(vals)@)[j]
            } else if j < seeds@.len() {
                seeds@[j]
            } else {
                rebuilt(seeds@.len() as int, bias, present(old(vals)@)[j], present(final(vals)@), j)
            },
))]
pub fn apply_spd_inverse_skipping_missing(
    vals: &mut [Option<i64>],
    seeds: &[i64],
    bias: i64,
) -> Result<(), FieldglassError> {
    let k = seeds.len();
    if k > 3 {
        return Err(unsupported_order(k));
    }
    if k == 0 {
        return Ok(());
    }
    // How many seeds have been placed, and the last three present values
    // rebuilt (`g1` the most recent). The count stops at `k`, the only point
    // past which it stops mattering, so it cannot overflow.
    let mut seen: usize = 0;
    let (mut g1, mut g2, mut g3) = (0i64, 0i64, 0i64);
    let n = vals.len();
    let mut i = 0;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            k == seeds@.len(),
            1 <= k <= 3,
            n == vals@.len(),
            n == old(vals)@.len(),
            i <= n,
            seen <= k,
            seen < k ==> seen == present(vals@.take(i as int)).len(),
            seen <= present(vals@.take(i as int)).len(),
            forall|j: int| i <= j < n ==> #[trigger] vals@[j] == old(vals)@[j],
            forall|j: int| 0 <= j < i ==> (#[trigger] vals@[j] is Some <==> old(vals)@[j] is Some),
            present(vals@.take(i as int)).len() == present(old(vals)@.take(i as int)).len(),
            history(present(vals@.take(i as int)), g1, g2, g3),
            forall|j: int| 0 <= j < present(old(vals)@.take(i as int)).len() ==>
                #[trigger] present(vals@.take(i as int))[j] == if j < k {
                    seeds@[j]
                } else {
                    rebuilt(k as int, bias, present(old(vals)@.take(i as int))[j], present(vals@.take(i as int)), j)
                },
        decreases n - i,
    ))]
    while i < n {
        #[cfg(verus_keep_ghost)]
        proof_decl! {
            let ghost before = vals@;
        }
        if let Some(d) = vals[i] {
            let v = if seen < k {
                seeds[seen]
            } else {
                next_value(k as u8, bias, d, g1, g2, g3)
            };
            #[cfg(verus_keep_ghost)]
            proof! {
                let q0 = present(vals@.take(i as int));
                assert(q0.len() < k ==> v == seeds@[q0.len() as int]);
                assert(q0.len() >= k ==> v == rebuilt(k as int, bias, d, q0.push(v), q0.len() as int));
            }
            vals[i] = Some(v);
            g3 = g2;
            g2 = g1;
            g1 = v;
            if seen < k {
                seen += 1;
            }
        }
        #[cfg(verus_keep_ghost)]
        proof! {
            assert(vals@.take(i as int) =~= before.take(i as int));
            lemma_present_step(old(vals)@, i as int);
            lemma_present_step(vals@, i as int);
            let q0 = present(before.take(i as int));
            let p0 = present(old(vals)@.take(i as int));
            let q1 = present(vals@.take(i + 1));
            let p1 = present(old(vals)@.take(i + 1));
            assert forall|j: int| 0 <= j < p1.len() implies #[trigger] q1[j] == if j < k {
                seeds@[j]
            } else {
                rebuilt(k as int, bias, p1[j], q1, j)
            } by {
                if j < p0.len() && j >= k {
                    assert(rebuilt(k as int, bias, p1[j], q1, j) == rebuilt(k as int, bias, p0[j], q0, j));
                }
            }
        }
        i += 1;
    }
    #[cfg(verus_keep_ghost)]
    proof! {
        assert(vals@.take(n as int) =~= vals@);
        assert(old(vals)@.take(n as int) =~= old(vals)@);
    }
    Ok(())
}

/// Value `i` of a run differenced to order `order` (1, 2 or 3), from its
/// stored difference `d` and the three values before it (`g1` the nearest),
/// in wrapping arithmetic. See the module docs; an order that uses fewer than
/// three previous values ignores the rest.
#[cfg_attr(verus_keep_ghost, verus_spec(out =>
    ensures
        out == next_value_spec(order as int, bias, d, g1, g2, g3),
))]
#[inline]
fn next_value(order: u8, bias: i64, d: i64, g1: i64, g2: i64, g3: i64) -> i64 {
    match order {
        1 => d.wrapping_add(g1).wrapping_add(bias),
        2 => d
            .wrapping_add(g1.wrapping_mul(2))
            .wrapping_sub(g2)
            .wrapping_add(bias),
        _ => d
            .wrapping_add(g1.wrapping_mul(3))
            .wrapping_sub(g2.wrapping_mul(3))
            .wrapping_add(g3)
            .wrapping_add(bias),
    }
}

/// The error for an order of differencing above 3. Building the message is
/// not something the proofs need to see into, so they take it on trust.
#[cfg_attr(verus_keep_ghost, verus_verify(external_body))]
#[cold]
fn unsupported_order(order: usize) -> FieldglassError {
    FieldglassError::Parse(format!("unsupported SPD order {order}"))
}

#[cfg(verus_keep_ghost)]
verus! {

/// The recurrence of the module docs, stated with the wrapping operations
/// themselves rather than over unbounded integers, so that the proof holds
/// the code to eccodes' two's-complement answer and not to the mathematical
/// one, which differs whenever an intermediate overflows.
pub open spec fn next_value_spec(order: int, bias: i64, d: i64, g1: i64, g2: i64, g3: i64) -> i64 {
    if order == 1 {
        d.wrapping_add(g1).wrapping_add(bias)
    } else if order == 2 {
        d.wrapping_add(g1.wrapping_mul(2)).wrapping_sub(g2).wrapping_add(bias)
    } else {
        d.wrapping_add(g1.wrapping_mul(3)).wrapping_sub(g2.wrapping_mul(3)).wrapping_add(
            g3,
        ).wrapping_add(bias)
    }
}

/// Value `j` of the rebuilt run `g`, from stored difference `d`: the recurrence
/// over the (up to) three values of `g` before it.
pub open spec fn rebuilt(order: int, bias: i64, d: i64, g: Seq<i64>, j: int) -> i64 {
    next_value_spec(
        order,
        bias,
        d,
        g[j - 1],
        if order >= 2 { g[j - 2] } else { 0 },
        if order >= 3 { g[j - 3] } else { 0 },
    )
}

/// The present values of `s`, in order: `s` with its `None`s dropped.
pub open spec fn present(s: Seq<Option<i64>>) -> Seq<i64>
    decreases s.len(),
{
    if s.len() == 0 {
        Seq::empty()
    } else {
        let rest = present(s.drop_last());
        match s.last() {
            Some(v) => rest.push(v),
            None => rest,
        }
    }
}

/// `g1`, `g2`, `g3` are the last three values of `q`, as far as it has them.
pub open spec fn history(q: Seq<i64>, g1: i64, g2: i64, g3: i64) -> bool {
    &&& q.len() >= 1 ==> g1 == q[q.len() - 1]
    &&& q.len() >= 2 ==> g2 == q[q.len() - 2]
    &&& q.len() >= 3 ==> g3 == q[q.len() - 3]
}

/// Taking one more element of `s` adds it to the present values if it is
/// present, and changes nothing if it is missing.
pub proof fn lemma_present_step(s: Seq<Option<i64>>, i: int)
    requires
        0 <= i < s.len(),
    ensures
        present(s.take(i + 1)) == match s[i] {
            Some(v) => present(s.take(i)).push(v),
            None => present(s.take(i)),
        },
{
    assert(s.take(i + 1).drop_last() =~= s.take(i));
}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spd_inverse_order1_is_cumulative_sum_with_bias() {
        // Order-1 reconstructs running sum y starting at X[0], adding
        // X[i] + bias at each step. With bias=0 it's a plain cumulative sum.
        let mut seq = vec![10i64, 1, 2, 3];
        apply_spd_inverse(&mut seq, 1, 0).unwrap();
        assert_eq!(seq, vec![10, 11, 13, 16]);

        // Bias of 1 shifts each successive y by +1 cumulatively.
        let mut seq = vec![10i64, 1, 2, 3];
        apply_spd_inverse(&mut seq, 1, 1).unwrap();
        // y starts at 10. y += 1+1=2 → 12; y += 2+1=3 → 15; y += 3+1=4 → 19.
        assert_eq!(seq, vec![10, 12, 15, 19]);
    }

    // Overflow regression: pre-fix, plain `+=` would panic in debug. Values are
    // unspecified at the i64 boundary; just verify the loop ran (tail slot
    // mutated from its sentinel) without panicking.
    #[test]
    fn spd_inverse_order1_does_not_panic_on_overflow() {
        let mut seq = vec![i64::MAX, 1, 2, 0];
        apply_spd_inverse(&mut seq, 1, i64::MAX).unwrap();
        assert_ne!(seq[3], 0, "tail slot must be reconstructed");
    }

    #[test]
    fn spd_inverse_order3_does_not_panic_on_overflow() {
        let mut seq = vec![i64::MIN, i64::MAX, i64::MIN, 1, 2, 0];
        apply_spd_inverse(&mut seq, 3, i64::MIN).unwrap();
        assert_ne!(seq[5], 0, "tail slot must be reconstructed");
    }

    #[test]
    fn spd_inverse_order2_reconstructs_quadratic_with_zero_bias() {
        // Values u[i] = i*i, second-order forward differences with bias 0,
        // then the inverse. After SPD-2 inverse with seeds u[0]=0, u[1]=1,
        // deltas [2,2,2,2] and bias=0:
        //   y_init = X[1] - X[0] = 1; z_init = X[1] = 1
        //   i=2: y += 2 → 3; z += 3 → 4
        //   i=3: y += 2 → 5; z += 5 → 9
        //   i=4: y += 2 → 7; z += 7 → 16
        //   i=5: y += 2 → 9; z += 9 → 25
        // → [0, 1, 4, 9, 16, 25]   (the squares!)
        let mut seq = vec![0i64, 1, 2, 2, 2, 2];
        apply_spd_inverse(&mut seq, 2, 0).unwrap();
        assert_eq!(seq, vec![0, 1, 4, 9, 16, 25]);
    }

    #[test]
    fn spd_inverse_order3_reconstructs_cubes_with_zero_bias() {
        // u[i] = i³ has constant third difference 6.
        let mut seq = vec![0i64, 1, 8, 6, 6, 6];
        apply_spd_inverse(&mut seq, 3, 0).unwrap();
        assert_eq!(seq, vec![0, 1, 8, 27, 64, 125]);
    }

    #[test]
    fn spd_inverse_rejects_order_above_3() {
        let mut seq = vec![0i64, 1, 2, 3];
        assert!(apply_spd_inverse(&mut seq, 4, 0).is_err());
        assert_eq!(
            seq,
            vec![0, 1, 2, 3],
            "a rejected order leaves the run alone"
        );
    }

    /// A run no longer than its seeds, including an empty one, has nothing to
    /// rebuild. Order 1 over an empty run used to index `x[0]` and panic.
    #[test]
    fn spd_inverse_leaves_a_run_of_only_seeds_alone() {
        for order in 0..=3u8 {
            for len in 0..=order as usize {
                let mut seq: Vec<i64> = (0..len as i64).map(|v| v * 7 - 3).collect();
                let before = seq.clone();
                apply_spd_inverse(&mut seq, order, 5).unwrap();
                assert_eq!(seq, before, "order {order}, length {len}");
            }
        }
    }

    /// The accumulator form eccodes' GRIB1 decoder uses, as this crate shipped
    /// it before the recurrence was shared with GRIB2. Kept to show the shared
    /// form rebuilds every run to the same bits.
    fn accumulator_form(x: &mut [i64], order: u8, bias: i64) {
        match order {
            1 if !x.is_empty() => {
                let mut y = x[0];
                for v in x.iter_mut().skip(1) {
                    y = y.wrapping_add(v.wrapping_add(bias));
                    *v = y;
                }
            }
            2 if x.len() >= 2 => {
                let mut y = x[1].wrapping_sub(x[0]);
                let mut z = x[1];
                for v in x.iter_mut().skip(2) {
                    y = y.wrapping_add(v.wrapping_add(bias));
                    z = z.wrapping_add(y);
                    *v = z;
                }
            }
            3 if x.len() >= 3 => {
                let mut y = x[2].wrapping_sub(x[1]);
                let mut z = y.wrapping_sub(x[1].wrapping_sub(x[0]));
                let mut w = x[2];
                for v in x.iter_mut().skip(3) {
                    z = z.wrapping_add(v.wrapping_add(bias));
                    y = y.wrapping_add(z);
                    w = w.wrapping_add(y);
                    *v = w;
                }
            }
            _ => {}
        }
    }

    /// A small deterministic generator (SplitMix64), so the comparison below
    /// covers the whole `i64` range, overflow included, without a dependency.
    fn splitmix(state: &mut u64) -> i64 {
        *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = *state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        (z ^ (z >> 31)) as i64
    }

    #[test]
    fn the_shared_form_matches_the_grib1_accumulator_form_to_the_bit() {
        let mut state = 0x5EED;
        let edges = [i64::MIN, i64::MIN + 1, -1, 0, 1, i64::MAX - 1, i64::MAX];
        for trial in 0..4000usize {
            let order = (trial % 4) as u8;
            let len = trial % 13;
            // Alternate full-range values with the edges, so both random
            // overflow and every boundary pairing come up.
            let mut pick = |i: usize| {
                if (trial + i).is_multiple_of(3) {
                    edges[(trial * 7 + i) % edges.len()]
                } else {
                    splitmix(&mut state)
                }
            };
            let run: Vec<i64> = (0..len).map(&mut pick).collect();
            let bias = pick(len);
            let mut shared = run.clone();
            let mut accumulated = run.clone();
            apply_spd_inverse(&mut shared, order, bias).unwrap();
            accumulator_form(&mut accumulated, order, bias);
            assert_eq!(
                shared, accumulated,
                "order {order}, run {run:?}, bias {bias}"
            );
        }
    }

    #[test]
    fn skipping_missing_seeds_the_first_present_points_and_skips_the_rest() {
        // Order 1, seed 100, bias 1: 100, then +2+1, then +3+1.
        let mut vals = vec![None, Some(7), None, Some(2), Some(3), None];
        apply_spd_inverse_skipping_missing(&mut vals, &[100], 1).unwrap();
        assert_eq!(
            vals,
            vec![None, Some(100), None, Some(103), Some(107), None]
        );

        // Order 2 over the squares, with gaps: seeds 0 and 1, differences 2.
        let mut vals = vec![
            Some(9),
            None,
            Some(9),
            Some(2),
            None,
            None,
            Some(2),
            Some(2),
        ];
        apply_spd_inverse_skipping_missing(&mut vals, &[0, 1], 0).unwrap();
        assert_eq!(
            vals,
            vec![
                Some(0),
                None,
                Some(1),
                Some(4),
                None,
                None,
                Some(9),
                Some(16)
            ]
        );
    }

    #[test]
    fn skipping_missing_matches_the_dense_form_on_the_present_points() {
        let mut state = 0xFACE;
        for trial in 0..2000usize {
            let order = trial % 4;
            let len = trial % 11;
            let run: Vec<Option<i64>> = (0..len)
                .map(|i| (!(trial + i).is_multiple_of(4)).then(|| splitmix(&mut state)))
                .collect();
            let seeds: Vec<i64> = (0..order).map(|_| splitmix(&mut state)).collect();
            let bias = splitmix(&mut state);

            let mut dense: Vec<i64> = run.iter().flatten().copied().collect();
            for (slot, seed) in dense.iter_mut().zip(&seeds) {
                *slot = *seed;
            }
            apply_spd_inverse(&mut dense, order as u8, bias).unwrap();

            let mut sparse = run.clone();
            apply_spd_inverse_skipping_missing(&mut sparse, &seeds, bias).unwrap();
            let present: Vec<i64> = sparse.iter().flatten().copied().collect();
            assert_eq!(present, dense, "order {order}, run {run:?}");
            for (after, before) in sparse.iter().zip(&run) {
                assert_eq!(after.is_some(), before.is_some());
            }
        }
    }

    #[test]
    fn skipping_missing_takes_only_the_seeds_it_has_room_for() {
        let mut vals = vec![None, Some(5)];
        apply_spd_inverse_skipping_missing(&mut vals, &[1, 2], 0).unwrap();
        assert_eq!(vals, vec![None, Some(1)]);
        let mut vals: Vec<Option<i64>> = vec![None, None];
        apply_spd_inverse_skipping_missing(&mut vals, &[1, 2], 0).unwrap();
        assert_eq!(vals, vec![None, None]);
    }

    #[test]
    fn skipping_missing_rejects_more_than_three_seeds() {
        let mut vals = vec![Some(1), Some(2)];
        assert!(apply_spd_inverse_skipping_missing(&mut vals, &[0; 4], 0).is_err());
        assert_eq!(vals, vec![Some(1), Some(2)]);
    }
}
