//! Textual s-expression view of a canonical IR tree (`docs/SIMILARITY-IR.md`
//! principle 6) — the readable form for golden/snapshot tests and debugging.
//! A *view*, never the substrate: the tree is built directly, not re-parsed.

use crate::tree::{Label, NormNode};

/// `(Kind@field label child…)` — field and label omitted when absent.
pub fn to_sexpr(node: &NormNode) -> String {
    let mut out = String::new();
    write_node(node, &mut out);
    out
}

fn write_node(node: &NormNode, out: &mut String) {
    out.push('(');
    out.push_str(&node.kind);
    if let Some(f) = &node.field {
        out.push('@');
        out.push_str(f);
    }
    if let Some(label) = &node.label {
        out.push(' ');
        out.push_str(&label_text(label));
    }
    for child in &node.children {
        out.push(' ');
        write_node(child, out);
    }
    out.push(')');
}

fn label_text(label: &Label) -> String {
    match label {
        Label::Raw(t) | Label::RawLit(t) | Label::External(t) | Label::LitKept(t) => t.to_string(),
        Label::Local(n) => format!("v{n}"),
        Label::LitBucket(b) => b.name().to_string(),
    }
}
