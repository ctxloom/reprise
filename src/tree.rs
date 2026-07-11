//! The normalized tree (spec §4, §5.2). Every node keeps a byte span into the
//! original source — a hard requirement: findings that can't be mapped to
//! source lines are useless.
//!
//! `kind`/`field` are interned (interning WP, session `stark-mixed-front`):
//! [`crate::intern::Kind`]/[`crate::intern::Field`] are `Deref<Target = str>` +
//! `AsRef<str>` + `From<&str>`, so every existing string comparison/construction site
//! keeps compiling and behaving byte-for-byte as before — see
//! `~/.ctxloom/sessions/stark-mixed-front/interning-preamble.md`.

use crate::intern::{Field, Kind, LSym};

/// One node of a normalized unit tree. A unit's token count is its node count:
/// one normalized token per node in the pre-order serialization (DECISIONS.md D1).
/// `Serialize` is derived for the on-disk cache (spec §4, DECISIONS.md D8/D19) — it
/// works transparently through `Kind`/`Field`/`Label`'s own `Serialize` impls, so the
/// bytes are unchanged from pre-interning. `Deserialize` is NOT derived (see
/// [`Label`]'s doc comment) — deserialize through [`NormNodeWire`] instead.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
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

/// Deserialize-only mirror of [`NormNode`] (see [`LabelWire`]) — field-for-field
/// identical to the pre-interning `Box<str>`-based shape, so bincode bytes through
/// this type match byte-for-byte what `#[derive(Deserialize)]` on the OLD `NormNode`
/// produced.
#[derive(serde::Deserialize)]
pub struct NormNodeWire {
    pub kind: Box<str>,
    pub field: Option<Box<str>>,
    pub label: Option<LabelWire>,
    pub span: (u32, u32),
    pub children: Vec<NormNodeWire>,
}

impl NormNodeWire {
    pub fn into_real(
        self,
        label_interner: &std::sync::Arc<crate::intern::LabelInterner>,
    ) -> NormNode {
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

/// `External`/`LitKept` carry an [`LSym`] (interned into the scan's per-scan
/// `LabelInterner` — see `crate::intern`), not a `Box<str>`. Only `serde::Serialize`
/// is derived here (works transparently through `LSym`'s own `Serialize`, which
/// resolves via its embedded interner — no external context needed to serialize).
/// `Deserialize` is deliberately NOT derived: reconstructing an `LSym` needs the
/// CURRENT scan's `LabelInterner`, which a context-free `Deserialize` impl cannot
/// reach. The deserialize path instead goes through `LabelWire` (below) + an explicit
/// `into_real` conversion — see `cache.rs`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
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

/// Deserialize-only mirror of [`Label`], field-for-field identical to the format
/// `Label` used before interning (`External`/`LitKept` as plain `Box<str>`) — so
/// bincode bytes through this type are byte-identical to what the pre-interning
/// `#[derive(Deserialize)]` produced. See [`NormNodeWire`]/`cache.rs`.
#[derive(serde::Deserialize)]
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
    pub fn into_real(self, label_interner: &std::sync::Arc<crate::intern::LabelInterner>) -> Label {
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

    /// Take the first child occupying `field`, removing it from `children`.
    pub fn take_field(&mut self, field: &str) -> Option<NormNode> {
        let idx = self
            .children
            .iter()
            .position(|c| c.field.as_deref() == Some(field))?;
        Some(self.children.remove(idx))
    }
}
