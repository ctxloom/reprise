//! Protocol-neutral projection of reprise findings for editor surfaces
//! (docs/SERVERS.md §4.2). A clone `Group` is projected to **one finding per
//! member**, each carrying its sibling members as `related` — the shape an LSP
//! diagnostic (with `relatedInformation`) needs, expressed without any LSP types
//! so it is testable on its own. The `reprise-lsp` crate maps `Finding` onto
//! `lsp_types::Diagnostic`.

use reprise::report::Group;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// All findings anchored in one file. Empty files are omitted.
#[derive(Debug, Clone)]
pub struct FileFindings {
    pub file: PathBuf,
    pub findings: Vec<Finding>,
}

/// One member of a clone group, as an advisory finding at its own location.
#[derive(Debug, Clone)]
pub struct Finding {
    pub line_span: (u32, u32),
    /// Kebab tier string (`near-normalized`, `exact-normalized`, …).
    pub tier: String,
    /// `1 - divergence`.
    pub similarity: f64,
    /// Stable group id and structural fingerprint — the latter keeps an editor
    /// alert identified across drift (renames/whitespace), mirroring SARIF's
    /// `partialFingerprints` choice (docs/SERVERS.md §4.2).
    pub group_id: String,
    pub fingerprint: String,
    pub message: String,
    /// The other members of the same group (sibling locations).
    pub related: Vec<Related>,
}

/// A sibling member's location, for `relatedInformation`.
#[derive(Debug, Clone)]
pub struct Related {
    pub file: PathBuf,
    pub line_span: (u32, u32),
    pub name: String,
}

/// Project clone groups into per-file advisory findings: every member of every
/// multi-member group becomes a `Finding` at its location whose `related` set is
/// the group's other members. Takes the groups slice (typically
/// `&report.groups`) rather than the whole `ScanReport` so it is constructible
/// in tests without the large `Stats`.
pub fn project(groups: &[Group]) -> Vec<FileFindings> {
    let mut by_file: BTreeMap<PathBuf, Vec<Finding>> = BTreeMap::new();

    for group in groups {
        if group.members.len() < 2 {
            continue; // a lone member is not a clone finding
        }
        let similarity = crate::similarity(group);
        for (i, m) in group.members.iter().enumerate() {
            let related: Vec<Related> = group
                .members
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, o)| Related {
                    file: o.file.clone(),
                    line_span: o.line_span,
                    name: o.name.clone(),
                })
                .collect();
            let message = format!(
                "{} clone: {} member(s) of this group, {:.0}% similar — reprise. \
                 Consider consolidating rather than diverging.",
                group.tier,
                group.members.len(),
                similarity * 100.0,
            );
            by_file.entry(m.file.clone()).or_default().push(Finding {
                line_span: m.line_span,
                tier: group.tier.to_string(),
                similarity,
                group_id: group.id.clone(),
                fingerprint: group.fingerprint.clone(),
                message,
                related,
            });
        }
    }

    by_file
        .into_iter()
        .map(|(file, findings)| FileFindings { file, findings })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reprise::report::{Member, Tier};

    fn member(file: &str, name: &str) -> Member {
        Member {
            file: PathBuf::from(file),
            lang: "rust".into(),
            name: name.into(),
            line_span: (10, 20),
            parse_degraded: false,
        }
    }

    fn group(members: Vec<Member>) -> Group {
        Group {
            id: "g1".into(),
            tier: Tier::NearNormalized,
            fingerprint: "abc".into(),
            token_count: 80,
            value: 1.0,
            note: None,
            divergence: 0.02,
            template: None,
            inline_chain: None,
            members,
        }
    }

    #[test]
    fn each_member_becomes_a_finding_with_siblings_as_related() {
        let g = group(vec![
            member("a.rs", "alpha"),
            member("b.rs", "beta"),
            member("c.rs", "gamma"),
        ]);
        let files = project(&[g]);
        // Three files, one finding each, each pointing at the other two.
        assert_eq!(files.len(), 3);
        for ff in &files {
            assert_eq!(ff.findings.len(), 1);
            let f = &ff.findings[0];
            assert_eq!(f.related.len(), 2);
            assert!((f.similarity - 0.98).abs() < 1e-9);
            assert_eq!(f.fingerprint, "abc");
            // A member never lists itself as related.
            assert!(f.related.iter().all(|r| r.file != ff.file));
        }
    }

    #[test]
    fn single_member_groups_are_skipped() {
        let g = group(vec![member("solo.rs", "solo")]);
        assert!(project(&[g]).is_empty());
    }
}
