//! Textual s-expression view of a canonical IR tree (`docs/SIMILARITY-IR.md`
//! principle 6) — the readable form for golden/snapshot tests and debugging.
//! A *view*, never the substrate: the tree is built directly, not re-parsed.

use crate::intern::LabelInterner;
use crate::tree::{Label, NormNode};

/// `(Kind@field label child…)` — field and label omitted when absent. `li` resolves
/// `Label::External`/`LitKept` ids back to text (test/debug rendering only — this
/// is never on a matching comparison path, so resolving here is fine; see the
/// interning-id-conversion WP's mechanism rule 3).
pub fn to_sexpr(node: &NormNode, li: &LabelInterner) -> String {
    let mut out = String::new();
    write_node(node, &mut out, li);
    out
}

fn write_node(node: &NormNode, out: &mut String, li: &LabelInterner) {
    out.push('(');
    out.push_str(node.kind.as_str());
    if let Some(f) = &node.field {
        out.push('@');
        out.push_str(f.as_str());
    }
    if let Some(label) = &node.label {
        out.push(' ');
        out.push_str(&label_text(label, li));
    }
    for child in &node.children {
        out.push(' ');
        write_node(child, out, li);
    }
    out.push(')');
}

fn label_text(label: &Label, li: &LabelInterner) -> String {
    match label {
        Label::Raw(t) | Label::RawLit(t) => t.to_string(),
        Label::External(sym) | Label::LitKept(sym) => li.resolve(*sym).to_string(),
        Label::Local(n) => format!("v{n}"),
        Label::LitBucket(b) => b.name().to_string(),
    }
}
