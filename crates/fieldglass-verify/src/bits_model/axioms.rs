//! The trusted `f64` axioms the verified kernels rest on. See `bits_model`.
#[cfg(verus_keep_ghost)]
use vstd::float::float_cast_spec;
use vstd::prelude::*;
#[cfg(verus_keep_ghost)]
use vstd::std_specs::ops::{AddSpec, MulSpec};

verus! {

/// `f64` `+` and `*` have no precondition (they never panic; overflow is
/// infinity) and are deterministic, so the value an expression computes is
/// the one its `add_spec` / `mul_spec` form names. `u32 as f64` is exact.
///
/// The two `obeys_*` facts mention no variable, so there is nothing to
/// trigger on; Verus's lint for a trigger-less broadcast is allowed here.
#[verifier::external_body]
#[verifier::allow(broadcast_without_trigger)]
pub broadcast proof fn f64_ops_are_total()
    ensures
        forall|a: f64, b: f64| #[trigger] a.add_req(b),
        forall|a: f64, b: f64| #[trigger] a.mul_req(b),
        <f64 as AddSpec>::obeys_add_spec(),
        <f64 as MulSpec>::obeys_mul_spec(),
        forall|x: u32, y: f64| #[trigger] float_cast_spec::<u32, f64>(x, y) ==> y == x as f64,
{
}

} // verus!
