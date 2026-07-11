//! The normalized tree (spec §4, §5.2). Every node keeps a byte span into the
//! original source — a hard requirement: findings that can't be mapped to
//! source lines are useless.
//!
//! `kind`/`field` are interned as `u16` ids (interning-id-conversion WP, session
//! `stark-mixed-front`): [`crate::intern::Kind`]/[`crate::intern::Field`] have NO
//! `Deref`/`AsRef<str>` — comparison sites must use id equality against a
//! pre-registered const (`crate::ir::kind::id::LOOP`-style) or a per-module
//! `LazyLock<Kind>`, never a string compare. `Label::External`/`LitKept` carry a
//! `u32` [`crate::intern::LSym`] id, resolved only through the scan's
//! `LabelInterner` — NOT self-contained, so `NormNode`/`Label` can no longer derive
//! a context-free `Serialize` (a bare id can't resolve to its string without that
//! interner reachable from the impl, which `serde::Serialize::serialize`'s
//! signature has no room for). The write side of the D19/pack wire format is
//! therefore an EXPLICIT, interner-aware conversion — [`NormNode::to_wire`] — that
//! mirrors the read side's existing [`NormNodeWire::into_real`]; both directions
//! produce/consume the SAME wire shape as pre-interning (a resolved-string
//! `NormNodeWire`), so on-disk bytes are unchanged (D19 unaffected, no
//! `EXTRACTION_VERSION` bump).

use crate::intern::{Field, Kind, LSym, LabelInterner};

/// One node of a normalized unit tree. A unit's token count is its node count:
/// one normalized token per node in the pre-order serialization (DECISIONS.md D1).
/// No `Serialize` derive (see the module doc) — go through [`NormNode::to_wire`]
/// for the write side, [`NormNodeWire::into_real`] for the read side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormNode {
    /// tree-sitter node kind, or the token text for kept anonymous tokens
    /// (operators), or a synthesized native kind (spec §5.2.3: prefer native kinds).
    pub kind: Kind,
    /// Field name this node occupies in its parent (from the grammar), if any.
    pub field: Option<Field>,
    /// Identifier/literal payload; `None` for pure structure.
    pub label: Option<Label>,
    /// Byte span in the original source file.
    pub span: (u32, u32),
    pub children: Vec<NormNode>,
}

/// Wire mirror of [`NormNode`] — field-for-field identical to the pre-interning
/// `Box<str>`-based shape, so bincode bytes through this type are byte-identical to
/// what the pre-interning `#[derive(Serialize, Deserialize)]` produced (D19
/// unchanged). `Serialize` (write, via [`NormNode::to_wire`]) and `Deserialize`
/// (read, via [`Self::into_real`]) both go through this ONE shape — no separate
/// read/write wire types.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct NormNodeWire {
    pub kind: Box<str>,
    pub field: Option<Box<str>>,
    pub label: Option<LabelWire>,
    pub span: (u32, u32),
    pub children: Vec<NormNodeWire>,
}

impl NormNodeWire {
    pub fn into_real(self, label_interner: &std::sync::Arc<LabelInterner>) -> NormNode {
        NormNode {
            kind: Kind::intern(&self.kind),
            field: self.field.as_deref().map(Field::intern),
            label: self.label.map(|l| l.into_real(label_interner)),
            span: self.span,
            children: self
                .children
                .into_iter()
                .map(|c| c.into_real(label_interner))
                .collect(),
        }
    }
}

impl NormNode {
    /// Resolve this node (and its whole subtree) to the wire shape — the ids are
    /// resolved to their strings ONCE here (`Kind`/`Field` via their own
    /// process-global table, `Label::External`/`LitKept` via `label_interner`),
    /// never on a comparison path. This is the only place a `NormNode` tree's ids
    /// get turned back into strings for the D19 cache / scan-scoped pack.
    pub fn to_wire(&self, label_interner: &LabelInterner) -> NormNodeWire {
        NormNodeWire {
            kind: self.kind.as_str().into(),
            field: self.field.map(|f| f.as_str().into()),
            label: self.label.as_ref().map(|l| l.to_wire(label_interner)),
            span: self.span,
            children: self
                .children
                .iter()
                .map(|c| c.to_wire(label_interner))
                .collect(),
        }
    }
}

/// `External`/`LitKept` carry an [`LSym`] id (interned into the scan's per-scan
/// [`LabelInterner`] — see `crate::intern`), not a string. No `Serialize`/
/// `Deserialize` derive (see the module doc) — go through [`Label::to_wire`] /
/// [`LabelWire::into_real`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Label {
    /// Identifier text before abstraction (transient; must not survive P3).
    Raw(Box<str>),
    /// Literal text before abstraction (transient; must not survive P3).
    RawLit(Box<str>),
    /// Kept external name — imports, other functions, stdlib (spec §5.2.4 exception).
    External(LSym),
    /// Positional local: first distinct local is 0, second 1, … (spec §5.2.4).
    Local(u32),
    /// Keep-list literal whose identity is structural (spec §5.2.5).
    LitKept(LSym),
    /// Bucketed literal (spec §5.2.5).
    LitBucket(Bucket),
}

impl Label {
    /// See [`NormNode::to_wire`].
    pub fn to_wire(&self, label_interner: &LabelInterner) -> LabelWire {
        match self {
            Label::Raw(t) => LabelWire::Raw(t.clone()),
            Label::RawLit(t) => LabelWire::RawLit(t.clone()),
            Label::External(sym) => LabelWire::External(Box::from(label_interner.resolve(*sym))),
            Label::Local(n) => LabelWire::Local(*n),
            Label::LitKept(sym) => LabelWire::LitKept(Box::from(label_interner.resolve(*sym))),
            Label::LitBucket(b) => LabelWire::LitBucket(*b),
        }
    }
}

/// Wire mirror of [`Label`], field-for-field identical to the format `Label` used
/// before interning (`External`/`LitKept` as plain `Box<str>`) — so bincode bytes
/// through this type are byte-identical to the pre-interning derive. See
/// [`NormNodeWire`]/`cache.rs`.
#[derive(serde::Serialize, serde::Deserialize)]
pub enum LabelWire {
    Raw(Box<str>),
    RawLit(Box<str>),
    External(Box<str>),
    Local(u32),
    LitKept(Box<str>),
    LitBucket(Bucket),
}

impl LabelWire {
    /// Re-intern `External`/`LitKept` text into `label_interner` (the current scan's
    /// interner — a fresh process/scan reconstructing a cache hit gets fresh ids, by
    /// design; see the interning preamble's cache-serde section).
    pub fn into_real(self, label_interner: &std::sync::Arc<LabelInterner>) -> Label {
        match self {
            LabelWire::Raw(t) => Label::Raw(t),
            LabelWire::RawLit(t) => Label::RawLit(t),
            LabelWire::External(t) => Label::External(label_interner.intern(&t)),
            LabelWire::Local(n) => Label::Local(n),
            LabelWire::LitKept(t) => Label::LitKept(label_interner.intern(&t)),
            LabelWire::LitBucket(b) => Label::LitBucket(b),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Bucket {
    Int,
    Float,
    Str,
    Bool,
    Char,
}

impl Bucket {
    pub fn name(self) -> &'static str {
        match self {
            Bucket::Int => "INT",
            Bucket::Float => "FLOAT",
            Bucket::Str => "STR",
            Bucket::Bool => "BOOL",
            Bucket::Char => "CHAR",
        }
    }
}

impl NormNode {
    pub fn new(kind: &str, field: Option<&str>, span: (u32, u32), children: Vec<NormNode>) -> Self {
        NormNode {
            kind: kind.into(),
            field: field.map(Into::into),
            label: None,
            span,
            children,
        }
    }

    /// Kept anonymous token (operator): kind is the token text itself.
    pub fn token(text: &str, span: (u32, u32)) -> Self {
        NormNode::new(text, None, span, Vec::new())
    }

    /// Like [`Self::new`], but `kind` is an ALREADY-interned [`Kind`] — the common
    /// case of rebuilding a node from an existing one's kind (AU template
    /// rendering, order canonicalization): avoids a pointless resolve-then-
    /// re-intern round trip through a fresh `&str` (the id is already known).
    pub fn with_kind(
        kind: Kind,
        field: Option<Field>,
        span: (u32, u32),
        children: Vec<NormNode>,
    ) -> Self {
        NormNode {
            kind,
            field,
            label: None,
            span,
            children,
        }
    }

    pub fn with_label(mut self, label: Label) -> Self {
        self.label = Some(label);
        self
    }

    /// Clone only the scalar fields (kind/field/label/span), leaving `children`
    /// empty. The bottom-up detector walks (`src/ir/pass.rs`) do
    /// `let mut n = node.shallow_clone(); n.children = node.children.iter().map(walk).collect();`
    /// — a full [`Clone`] there would deep-clone the entire subtree only to discard the
    /// cloned `children` on the very next line and rebuild them from the recursive map
    /// (O(subtree) dead allocations per node). This clones just the node's own fields and
    /// lets the caller populate `children` once; the resulting tree is byte-identical.
    pub fn shallow_clone(&self) -> NormNode {
        NormNode {
            kind: self.kind,
            field: self.field,
            label: self.label.clone(),
            span: self.span,
            children: Vec::new(),
        }
    }

    /// Normalized token count (DECISIONS.md D1): one token per node.
    pub fn token_count(&self) -> u32 {
        1 + self.children.iter().map(NormNode::token_count).sum::<u32>()
    }

    /// Take the first child occupying `field`, removing it from `children`. Interns
    /// `field` ONCE (not per child — the id comparison across all children is a bare
    /// `u16` equality; see the interning-id-conversion WP's "runtime strings intern
    /// once per acquisition, not per comparison" rule. `field` here is always a
    /// compile-time literal at call sites, but the method itself stays reusable
    /// rather than requiring every caller to pre-register a `LazyLock`).
    pub fn take_field(&mut self, field: &str) -> Option<NormNode> {
        let target = Field::intern(field);
        let idx = self.children.iter().position(|c| c.field == Some(target))?;
        Some(self.children.remove(idx))
    }
}

#[cfg(test)]
mod size_gate {
    /// G1 (interning-id-conversion WP, hard invariant): `NormNode` must fit in 64
    /// bytes. A 16-byte string handle (the prior, WRONG WP's substitution) cannot
    /// pass this — the point of the gate is exactly that: it is a mechanical proof
    /// the id conversion actually happened, not just claimed.
    #[test]
    fn norm_node_fits_in_64_bytes() {
        let size = std::mem::size_of::<super::NormNode>();
        assert!(
            size <= 64,
            "NormNode grew to {size} bytes (budget: 64) — the Kind/Field/LSym id \
             conversion must have been bypassed somewhere"
        );
    }
}
