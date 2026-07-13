//! Test-only helpers shared by the module test suites.

/// Deterministic LCG (no external rand dependency) for the randomized
/// brute-force-oracle tests: many random shapes, reproducible from a seed.
pub(crate) struct Lcg(pub(crate) u64);

impl Lcg {
    pub(crate) fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        self.0
    }

    pub(crate) fn range(&mut self, n: u32) -> u32 {
        (self.next() % u64::from(n)) as u32
    }
}
