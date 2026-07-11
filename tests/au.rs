//! Anti-unification soundness (spec §5.6). Regression guard for the
//! address-keyed `node_info` memo in `src/au.rs`: the memo is keyed by node
//! POINTER, sound only while every node it sees is borrowed from one of the two
//! immovable input trees. `flatten_operands` used to `clone` the operands of a
//! commutative chain into a transient `Vec`; once that `Vec` freed, the memo
//! still held entries keyed on the freed addresses, and the allocator handed the
//! same address to the NEXT chain's operands — so a later lookup returned a hash
//! and token-count computed for a DIFFERENT node. The corruption is
//! allocator-nondeterministic (tcache address reuse), hence the loop below.

use reprise::config::{Config, Normalizer};
use reprise::lang::Lang;

/// Two 3-operand commutative (`+`) chains in a single unit — the exact trigger:
/// chain 1's flattened operands free, chain 2's reuse their addresses. Chain
/// tails differ (`c`/`f` leaves vs the 4-node `x*y` subtree) so a stale hit is
/// observable as an inflated hole token count.
const A: &str =
    "def g(a, b, c, d, e, f):\n    r = a + b + c\n    s = d + e + f\n    return r + s\n";
const B: &str = "def g(a, b, c, d, e, f, x, y):\n    r = a + b + x * y\n    s = d + e + x * y\n    return r + s\n";

#[test]
fn two_commutative_chains_anti_unify_is_sound_and_stable() {
    let cfg = Config::default();
    let ua = reprise::units_from_source(A, Lang::Python, &cfg).remove(0);
    let ub = reprise::units_from_source(B, Lang::Python, &cfg).remove(0);
    let ir = cfg.normalize.normalizer == Normalizer::Ir
        && reprise::frontend::has_ir_frontend(Lang::Python);
    // A profile is required only on the historical path; the IR path passes `None`.
    let profile = (!ir).then(|| Lang::Python.profile());

    // Ground truth (sound memo == no memo): one param-list gap hole plus one
    // `leaf vs x*y` hole per chain. `x*y` is a 4-node subtree (binop + 2 leaves
    // + operator token); the leaf is 1 node. Divergence = 12 / (2*28).
    let expected_holes = [(0u32, 2u32), (1, 4), (1, 4)];
    let expected_div = 12.0 / 56.0;

    // The stale-memo corruption is nondeterministic in the allocator; loop so a
    // corrupting layout is hit with overwhelming probability on the old code.
    for iter in 0..500 {
        let o = reprise::au::anti_unify(
            ua.tree.expect_resident(),
            ub.tree.expect_resident(),
            profile,
            ir,
        );

        let mut holes: Vec<(u32, u32)> = o.holes.iter().map(|h| (h.tokens_a, h.tokens_b)).collect();
        holes.sort_unstable();
        assert_eq!(
            holes, expected_holes,
            "iter {iter}: hole token counts corrupted (stale address-keyed memo). \
             div={:.4}",
            o.divergence,
        );
        assert!(
            (o.divergence - expected_div).abs() < 1e-9,
            "iter {iter}: divergence {:.6} != expected {expected_div:.6}",
            o.divergence,
        );
        assert!(o.factorable, "iter {iter}: factorable flipped");
    }
}
