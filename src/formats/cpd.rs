//! PMD/CPD XML emitter (spec §6.1, `--format cpd`): the de-facto CI format for
//! duplication. `<pmd-cpd>` root, one `<duplication lines="…" tokens="…">` per
//! group with a `<file line="…" path="…"/>` per member and a `<codefragment>`
//! CDATA of the first member's source slice. `lines` = member line-span length,
//! `tokens` = the group's normalized token count (D1). Emitted so a team can
//! drop reprise into an existing CPD pipeline and so §7.3's baseline diff is
//! mechanical (§6.1).

use crate::check::CheckReport;
use crate::report::{Group, ScanReport};
use std::path::Path;

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// CDATA-safe: split any `]]>` so it can't close the section early.
fn cdata_escape(s: &str) -> String {
    s.replace("]]>", "]]]]><![CDATA[>")
}

fn duplication_xml(out: &mut String, group: &Group, root: &Path) {
    let first = &group.members[0];
    let lines = first.line_span.1.saturating_sub(first.line_span.0) + 1;
    out.push_str(&format!(
        "  <duplication lines=\"{}\" tokens=\"{}\">\n",
        lines, group.token_count
    ));
    for m in &group.members {
        out.push_str(&format!(
            "    <file line=\"{}\" endline=\"{}\" path=\"{}\"/>\n",
            m.line_span.0,
            m.line_span.1,
            xml_escape(&super::rel(root, &m.file))
        ));
    }
    let fragment = super::read_slice(root, &first.file, first.line_span);
    out.push_str("    <codefragment><![CDATA[");
    out.push_str(&cdata_escape(&fragment));
    out.push_str("]]></codefragment>\n");
    out.push_str("  </duplication>\n");
}

fn render(groups: &[&Group], root: &Path) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<pmd-cpd>\n");
    for g in groups {
        duplication_xml(&mut out, g, root);
    }
    out.push_str("</pmd-cpd>\n");
    out
}

pub fn scan_cpd(report: &ScanReport, root: &Path, verbose: bool) -> String {
    render(&super::cpd_jscpd_scan_groups(report, verbose), root)
}

pub fn check_cpd(report: &CheckReport, root: &Path) -> String {
    render(&super::cpd_jscpd_check_groups(report), root)
}
