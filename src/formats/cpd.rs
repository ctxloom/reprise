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

/// True for the characters XML 1.0's Char production forbids OUTRIGHT — illegal
/// even inside CDATA, so no amount of escaping can legalize them. The only legal
/// characters below U+0020 are tab (#x9), LF (#xA), and CR (#xD); every other C0
/// control is banned, as are U+FFFE/U+FFFF. Scanned source is untrusted input
/// and legitimately carries such bytes as valid UTF-8 (form feed \x0C is a real
/// Emacs/GNU page-break that appears inside functions; ESC \x1B and NUL \x00
/// show up in fixtures and minified/binary-ish files) — `read_to_string` passes
/// them straight through. One in a `<codefragment>` or `path=` makes the whole
/// cpd document non-well-formed, so a conforming parser rejects it, breaking the
/// CI pipeline `--format cpd` exists to feed.
fn is_xml_illegal(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{8}' | '\u{B}' | '\u{C}' | '\u{E}'..='\u{1F}' | '\u{FFFE}' | '\u{FFFF}')
}

/// Replace every XML-forbidden character with U+FFFD (the replacement char):
/// visible, meaning-preserving, and keeps the fragment human-readable in reports
/// (dropping would also be sound). Tab/LF/CR — the legal C0 whitespace — survive.
fn strip_xml_illegal(s: &str) -> String {
    if s.chars().any(is_xml_illegal) {
        s.chars()
            .map(|c| if is_xml_illegal(c) { '\u{FFFD}' } else { c })
            .collect()
    } else {
        s.to_string()
    }
}

fn xml_escape(s: &str) -> String {
    strip_xml_illegal(
        &s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;"),
    )
}

/// CDATA-safe: split any `]]>` so it can't close the section early, then drop the
/// XML-forbidden control chars that are illegal even inside CDATA (`is_xml_illegal`).
fn cdata_escape(s: &str) -> String {
    strip_xml_illegal(&s.replace("]]>", "]]]]><![CDATA[>"))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Characters that XML 1.0's Char production forbids OUTRIGHT — illegal even
    /// inside CDATA, no escaping can legalize them. Scanned source is untrusted
    /// input and legitimately contains these as valid UTF-8 (form feed \x0C is a
    /// real Emacs/GNU page-break byte that appears inside functions; ESC \x1B and
    /// NUL \x00 turn up in fixtures and minified/binary-ish files). A single such
    /// byte in a <codefragment> or path= makes the whole cpd document non-well-
    /// formed and a conforming XML parser rejects it.
    const FORBIDDEN: &[char] = &['\u{0}', '\u{0C}', '\u{1B}', '\u{8}', '\u{1F}', '\u{FFFE}'];

    #[test]
    fn cdata_escape_drops_xml_forbidden_control_chars() {
        // A realistic page-broken fragment: NUL, form feed (page break), ESC.
        let fragment = "fn page_one() {}\u{0C}\n\u{0}fn page_two() {\u{1B}[0m}";
        let out = cdata_escape(fragment);
        for &bad in FORBIDDEN {
            assert!(
                !out.contains(bad),
                "cdata_escape must not emit XML-forbidden char {:#04x}",
                bad as u32
            );
        }
        // Legal C0 whitespace (tab/LF/CR) must survive untouched.
        let ws = cdata_escape("a\tb\nc\rd");
        assert_eq!(ws, "a\tb\nc\rd", "tab/LF/CR are legal XML and stay");
        // The `]]>` terminator split still works alongside the control-char pass:
        // the terminator is broken across two CDATA sections so it can't close the
        // outer section early (the byte sequence remains, but as split content).
        assert!(
            cdata_escape("]]>").contains("]]]]><![CDATA[>"),
            "]]> terminator is split across CDATA sections"
        );
    }

    #[test]
    fn xml_escape_drops_xml_forbidden_control_chars() {
        // A path attribute carrying a stray control byte.
        let path = "src/\u{0C}weird\u{0}/mod\u{1B}.rs";
        let out = xml_escape(path);
        for &bad in FORBIDDEN {
            assert!(
                !out.contains(bad),
                "xml_escape must not emit XML-forbidden char {:#04x}",
                bad as u32
            );
        }
        // Markup metacharacters still escape, and legal whitespace survives.
        assert_eq!(xml_escape("a<b>&\"'\tx"), "a&lt;b&gt;&amp;&quot;&apos;\tx");
    }
}
