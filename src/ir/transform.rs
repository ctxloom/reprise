//! The transform log — the append-only, deterministic **event stream** (§15/§15.1).
//!
//! In memory only, never persisted (D-IR-10): no serde derives, and it is excluded
//! from the D19 per-file cache. A consumer that needs it (the reporter for
//! match-explanation / template un-lowering, or the calibration harness) re-normalizes
//! the touched unit with logging on — the log is a deterministic function of source,
//! so it is free to reconstruct.
//!
//! Each transform has a defined inverse for *display* reversal (§15.2). Lossy
//! (many-to-one) transforms carry a [`Witness`] — the discriminator normalization
//! relocated out of the hashable tree — which *is* the event payload that makes
//! "un-apply the transform" well-defined.

use std::fmt::Write as _;

/// Closed, versioned vocabulary of normalization transforms (`docs/SIMILARITY-IR.md`
/// §15). Bijective ones invert from a trivial witness; lossy ones need a real one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransformKind {
    /// Redundant grouping removed (bijective given the paren locus).
    ParenDrop,
    /// Commutative operand chain sorted (bijective given the original order).
    CommSort,
    /// A source loop form (`for`/`while`/`foreach`) folded to the one `Loop`.
    LoopLower,
    /// Tail/linear recursion rewritten to a loop.
    RecursionLower,
    /// Index-over-length rewritten to the iteration protocol.
    IterProtocol,
    /// Trailing `break`+`return` folded at the break site.
    LoopExit,
    /// An intermediate value named (ANF, §13).
    AnfName,
    /// A common arm-tail effect hoisted out of a `Branch` (§14).
    BranchHoist,
    /// Augmented assignment desugared: `a op= b` → `a = a op b` (sound identity —
    /// converges with the explicit form while `+=`/`-=` stay distinct via the binop).
    AugAssign,
    /// Dead syntax removed (`pass`, empty `else`, redundant trailing `continue`).
    DeadStrip,
    /// Ordered comparison oriented to `<`/`<=` (`a > b` → `b < a`) — bijective given the
    /// operand swap, recorded as an `Order` witness so reversal restores the orientation.
    CmpOrient,
    /// A logical negation pushed inward (`!(a<b)`→`a>=b`, De Morgan, `!!x`→`x`) — bijective
    /// given the locus (reversal re-wraps the `!`), so it carries no discriminator witness.
    NotPush,
    /// A nested `if a { if b { … } }` merged into one `if a && b { … }` (§13 rung 2) — the
    /// merged nesting depth rides in the witness so reversal re-nests the guards.
    GuardMerge,
    /// A redundant `else` after a diverging then-arm dropped, its body hoisted to siblings
    /// (`if c { return } else { Y }` → `if c { return }; Y`) — the hoisted body is the witness.
    DeadElse,
    /// A parallel multi-target assignment (`a, b = X, Y`) decomposed into a minimal sequence
    /// of single assignments, a temp inserted only to break a read-after-write cycle — so it
    /// converges with the equivalent adjacent single assigns and with tail-rec reassignment.
    /// The original multi-assign rides in the witness so reversal restores it.
    MultiAssign,
    /// A literal abstracted to its typed bucket — lossy; the value rides in the witness.
    LitBucket,
    /// Identifiers relabelled to positional locals / `External` (§5.2.4) — the whole-tree
    /// abstraction; the position→original-name map rides in the witness.
    AbstractIdents,
}

impl TransformKind {
    /// Stable slug for the readable/serialized form.
    pub fn name(self) -> &'static str {
        match self {
            TransformKind::ParenDrop => "paren-drop",
            TransformKind::CommSort => "comm-sort",
            TransformKind::LoopLower => "loop-lower",
            TransformKind::RecursionLower => "recursion-lower",
            TransformKind::IterProtocol => "iter-protocol",
            TransformKind::LoopExit => "loop-exit",
            TransformKind::AnfName => "anf-name",
            TransformKind::BranchHoist => "branch-hoist",
            TransformKind::AugAssign => "aug-assign",
            TransformKind::DeadStrip => "dead-strip",
            TransformKind::CmpOrient => "cmp-orient",
            TransformKind::NotPush => "not-push",
            TransformKind::GuardMerge => "guard-merge",
            TransformKind::DeadElse => "dead-else",
            TransformKind::MultiAssign => "multi-assign",
            TransformKind::LitBucket => "lit-bucket",
            TransformKind::AbstractIdents => "abstract-idents",
        }
    }
}

/// The discriminator a lossy transform normalized away — enough to invert it for
/// display (§15.2). Bijective transforms carry [`Witness::None`] or a light record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Witness {
    /// No discriminator needed (or an inherently bijective transform).
    None,
    /// The source loop keyword/form folded into the canonical `Loop`.
    LoopForm(Box<str>),
    /// The operand order before a commutative sort (indices into the sorted list).
    Order(Vec<u32>),
    /// A free-form note — placeholder until a transform earns a typed witness.
    Note(Box<str>),
    /// The original literal text a `LitBucket` abstracted away — so two clones that
    /// differ only in a constant converge, yet the difference stays recoverable.
    Literal(Box<str>),
}

/// One recorded normalization step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransformEvent {
    pub kind: TransformKind,
    /// Source byte span the transform acted on.
    pub locus: (u32, u32),
    pub witness: Witness,
}

/// The per-unit event log: append-only, ordered, deterministic.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TransformLog {
    events: Vec<TransformEvent>,
    /// A disabled log is the null sink (D-IR-10): [`record`](Self::record) early-
    /// returns and nothing is materialized. Because the log is hash-excluded, this
    /// never changes the canonical tree — it only skips the (discarded) stream.
    disabled: bool,
}

impl TransformLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// The null sink for the bulk-scan path (`docs/transform-seam.md` §1): recording
    /// is a no-op that allocates nothing, so completing the (hash-excluded) event
    /// stream costs nothing and cannot move the tree.
    pub fn disabled() -> Self {
        Self {
            events: Vec::new(),
            disabled: true,
        }
    }

    /// Whether events are being recorded (a [`disabled`](Self::disabled) log is not) —
    /// let callers skip materializing a witness they would only discard.
    pub fn enabled(&self) -> bool {
        !self.disabled
    }

    /// Append a transform event (the only mutation — the log is append-only). A
    /// disabled sink drops it.
    pub fn record(&mut self, kind: TransformKind, locus: (u32, u32), witness: Witness) {
        if self.disabled {
            return;
        }
        self.events.push(TransformEvent {
            kind,
            locus,
            witness,
        });
    }

    pub fn events(&self) -> &[TransformEvent] {
        &self.events
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Deterministic, readable serialization — the snapshot-test / determinism
    /// harness (§12.1, the SCIP lesson) and the source of match-explanation diffs.
    /// One event per line.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for e in &self.events {
            let _ = write!(out, "{} {}..{}", e.kind.name(), e.locus.0, e.locus.1);
            match &e.witness {
                Witness::None => {}
                Witness::LoopForm(f) => {
                    let _ = write!(out, " loop-form={f}");
                }
                Witness::Order(o) => {
                    let _ = write!(out, " order={o:?}");
                }
                Witness::Note(n) => {
                    let _ = write!(out, " note={n}");
                }
                Witness::Literal(t) => {
                    let _ = write!(out, " lit={t}");
                }
            }
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_is_append_only_and_ordered() {
        let mut log = TransformLog::new();
        assert!(log.is_empty());
        log.record(
            TransformKind::LoopLower,
            (10, 20),
            Witness::LoopForm("for".into()),
        );
        log.record(TransformKind::CommSort, (0, 5), Witness::Order(vec![1, 0]));
        assert_eq!(log.len(), 2);
        assert_eq!(log.events()[0].kind, TransformKind::LoopLower);
        assert_eq!(log.events()[1].kind, TransformKind::CommSort);
    }

    #[test]
    fn to_text_is_a_deterministic_snapshot() {
        let mut log = TransformLog::new();
        log.record(
            TransformKind::LoopLower,
            (10, 20),
            Witness::LoopForm("for".into()),
        );
        log.record(
            TransformKind::AnfName,
            (30, 40),
            Witness::Note("subexpr@33".into()),
        );
        let expected = "loop-lower 10..20 loop-form=for\nanf-name 30..40 note=subexpr@33\n";
        assert_eq!(log.to_text(), expected);
        // Same input → same text (the determinism the drift baseline relies on).
        assert_eq!(log.to_text(), log.to_text());
    }
}
