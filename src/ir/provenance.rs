//! Per-node source provenance (`docs/SIMILARITY-IR.md` §15). Every IR node maps
//! back to source directly ([`Provenance::Source`]) or transitively
//! ([`Provenance::Derived`], for synthesized nodes), so "findings map to real
//! lines" survives arbitrarily aggressive normalization. Provenance is
//! **hash-excluded** (like spans): two clones that used different source forms
//! must still collide.

use super::transform::TransformKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provenance {
    /// A real byte span in the original source.
    Source((u32, u32)),
    /// A synthesized node (an ANF temp, a lowered loop core, a hoisted branch
    /// value): the transform that created it and the *set* of source spans it
    /// descends from (a hoisted node descends from several — hence a `Vec`).
    Derived {
        transform: TransformKind,
        from: Vec<(u32, u32)>,
        role: &'static str,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_construct_and_differ() {
        let source = Provenance::Source((1, 2));
        let derived = Provenance::Derived {
            transform: TransformKind::BranchHoist,
            from: vec![(3, 4), (5, 6)],
            role: "hoisted-branch-value",
        };
        assert_ne!(source, derived);
    }
}
