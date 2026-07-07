//! The canonical IR node-set (`docs/SIMILARITY-IR.md` §5) — a **closed, versioned**
//! vocabulary (the CodeQL `dbscheme` discipline, §12.1). Under D-IR-1(a) the IR
//! reuses `NormNode`'s shape, so a "kind" is just one of these strings; frontends
//! lower into them and the language-agnostic algorithms match on them.
//!
//! `SCHEME_VERSION` bumps on any change to this set — it feeds the cache key the
//! same way `FINGERPRINT_SCHEME`/`EXTRACTION_VERSION` do (D19/D30), because the
//! vocabulary *is* the canonical form.

/// Bumps on any change to the canonical vocabulary **or lowering** (feeds the cache key,
/// D19/D30) — a warm cache must never serve a tree from an older canonical form. Bumped
/// for the P1 parity build-out: Go frontend, Branch guard-folding + flatten, keep-list,
/// aug-assign/desugars, iteration-protocol + loop-exit passes, fold, destructure binds.
/// Bumped to 3 for the relational-boolean-normalization batch — Family B desugars Go
/// `i++`/`i--` and `+=`/`-=`/… through the `AugAssign` path (moves the Go canonical tree).
/// Bumped to 4 for the canonical-IR pass correctness batch — the loop/recursion/guard
/// canonicalizers stop over-matching (nested-loop iterator retarget, non-tail recursion,
/// arity-mismatch reassignment, non-breaking counter guards) and fold loop-exit to a
/// fixpoint, so several constructs' canonical output (and merkle fingerprint) shifts.
/// Bumped to 5 for the canonicalization-correctness batch — a pure-atomic Python guard nest
/// now merges with the language's `and` token (not a hardcoded `&&`), the counter-loop
/// iteration rewrite's liveness check became function-scoped (a counter used after its
/// enclosing block no longer wrongly rewrites), and cycle-break temps are minted disjoint from
/// existing `__mt{n}` names — each shifting its construct's canonical output (merkle fingerprint).
/// Bumped to 6 for the Go `for true {}` convergence — a bare `true` condition is now recognized as
/// the infinite-loop core (no break-guard, mirroring `while true`), so `for true {}` lowers
/// identically to `for {}` and their canonical output (merkle fingerprint) converges.
/// Bumped to 7 for two IR-frontend lowering fixes that shift canonical output: (1) a Go `switch`
/// `default` case written before a later `case` is now ordered LAST in the `Branch` (a guard-less
/// arm is the "always matches" else, so a mid-list default made later real cases dead arms), so a
/// `default`-first switch converges with the semantically-identical `default`-last form; (2) a Rust
/// open-start range `..b` now labels `b` as `@right` (the end operand), tracked by the `..`/`..=`
/// operator position rather than "first named child" — previously `b` was mislabelled `@left`.
pub const SCHEME_VERSION: u32 = 7;

// ---- structural ----
pub const UNIT: &str = "Unit";
pub const BLOCK: &str = "Block";
pub const LOOP: &str = "Loop";
/// The one unified conditional (§14): if-chains / match / when / switch / ternary.
pub const BRANCH: &str = "Branch";
/// A `(guard, body)` member of a [`BRANCH`].
pub const ARM: &str = "Arm";
pub const ASSIGN: &str = "Assign";
pub const RETURN: &str = "Return";
pub const BREAK: &str = "Break";
pub const CONTINUE: &str = "Continue";

// ---- expression ----
pub const CALL: &str = "Call";
pub const BINOP: &str = "Binop";
pub const UNOP: &str = "Unop";
pub const INDEX: &str = "Index";
pub const FIELD: &str = "Field";
pub const LAMBDA: &str = "Lambda";
pub const VAR: &str = "Var";
pub const LIT: &str = "Lit";

// ---- escape hatch (§12.1: category-specific, opaque, resolution-free) ----
pub const NATIVE_STMT: &str = "NativeStmt";
pub const NATIVE_EXPR: &str = "NativeExpr";
pub const NATIVE_PAT: &str = "NativePat";
pub const NATIVE_TYPE: &str = "NativeType";

// ---- ⚠ open (start as its simplest form / `Native`; §5, resolved by P1) ----
/// The iteration protocol as an explicit node (today: synthetic `__has_next`/`__next`).
pub const ITER: &str = "Iter";

/// Every canonical kind. Frontends must emit only these (or a language token /
/// `Native*` leaf); the closed set is what lets one schema serve five frontends.
pub const ALL: &[&str] = &[
    UNIT,
    BLOCK,
    LOOP,
    BRANCH,
    ARM,
    ASSIGN,
    RETURN,
    BREAK,
    CONTINUE,
    CALL,
    BINOP,
    UNOP,
    INDEX,
    FIELD,
    LAMBDA,
    VAR,
    LIT,
    NATIVE_STMT,
    NATIVE_EXPR,
    NATIVE_PAT,
    NATIVE_TYPE,
    ITER,
];

/// Is `kind` a member of the canonical vocabulary? (A tree-sitter CST kind such
/// as `"call_expression"` is not — that is the pre-lowering surface.)
pub fn is_canonical(kind: &str) -> bool {
    ALL.contains(&kind)
}

/// Is `kind` one of the opaque escape-hatch categories? A `Native*` node carries
/// its language discriminator in its label and matches only same-tag (§12.1).
pub fn is_native(kind: &str) -> bool {
    matches!(kind, NATIVE_STMT | NATIVE_EXPR | NATIVE_PAT | NATIVE_TYPE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn all_kinds_unique_and_recognized() {
        let mut seen = HashSet::new();
        for &k in ALL {
            assert!(seen.insert(k), "duplicate canonical kind: {k}");
            assert!(is_canonical(k), "{k} missing from is_canonical");
        }
    }

    #[test]
    fn native_kinds_classify() {
        for &k in &[NATIVE_STMT, NATIVE_EXPR, NATIVE_PAT, NATIVE_TYPE] {
            assert!(is_native(k));
            assert!(is_canonical(k));
        }
        assert!(!is_native(LOOP));
        // A raw tree-sitter kind is never canonical — it is the surface the
        // frontend lowers *from*.
        assert!(!is_canonical("call_expression"));
    }
}
