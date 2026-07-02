//! The normalized tree (spec §4, §5.2). Every node keeps a byte span into the
//! original source — a hard requirement: findings that can't be mapped to
//! source lines are useless.

/// One node of a normalized unit tree. A unit's token count is its node count:
/// one normalized token per node in the pre-order serialization (DECISIONS.md D1).
/// Serde derives exist for the on-disk cache (spec §4, DECISIONS.md D8/D19).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NormNode {
    /// tree-sitter node kind, or the token text for kept anonymous tokens
    /// (operators), or a synthesized native kind (spec §5.2.3: prefer native kinds).
    pub kind: Box<str>,
    /// Field name this node occupies in its parent (from the grammar), if any.
    pub field: Option<Box<str>>,
    /// Identifier/literal payload; `None` for pure structure.
    pub label: Option<Label>,
    /// Byte span in the original source file.
    pub span: (u32, u32),
    pub children: Vec<NormNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Label {
    /// Identifier text before abstraction (transient; must not survive P3).
    Raw(Box<str>),
    /// Literal text before abstraction (transient; must not survive P3).
    RawLit(Box<str>),
    /// Kept external name — imports, other functions, stdlib (spec §5.2.4 exception).
    External(Box<str>),
    /// Positional local: first distinct local is 0, second 1, … (spec §5.2.4).
    Local(u32),
    /// Keep-list literal whose identity is structural (spec §5.2.5).
    LitKept(Box<str>),
    /// Bucketed literal (spec §5.2.5).
    LitBucket(Bucket),
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
