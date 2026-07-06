//! P7 (exact slice) + P8 assembly helpers: exact-hash bucketing, containment
//! suppression, and consolidation-value ranking (spec §5.6, §6).

use crate::config::Config;
use crate::lang::Lang;
use crate::report::{Group, Member, Tier};
use crate::unit::Unit;
use std::collections::BTreeMap;

/// Exact tier (plain units only): returns groups plus their member unit
/// indices (the indices feed sequence-tier subsumption) and the below-floor
/// count. Variants are handled by `build_inline_exact_groups`.
pub fn build_exact_groups(units: &[Unit], cfg: &Config) -> (Vec<(Group, Vec<usize>)>, u32) {
    let mut below_floor = 0u32;
    let mut buckets: BTreeMap<(Lang, u128), Vec<usize>> = BTreeMap::new();
    for (idx, unit) in units.iter().enumerate() {
        if unit.variant.is_some() {
            continue;
        }
        if unit.token_count < cfg.min_unit_floor() {
            below_floor += 1;
            continue;
        }
        buckets
            .entry((unit.lang, unit.fingerprint))
            .or_default()
            .push(idx);
    }

    let raw_groups: Vec<Vec<usize>> = buckets
        .into_values()
        .filter(|members| members.len() >= 2)
        .collect();

    let kept: Vec<&Vec<usize>> = raw_groups
        .iter()
        .enumerate()
        .filter(|(i, group)| !is_contained_in_other(units, group, *i, &raw_groups))
        .map(|(_, g)| g)
        .collect();

    let groups = kept
        .into_iter()
        .map(|indices| {
            let members: Vec<&Unit> = indices.iter().map(|&i| &units[i]).collect();
            let token_count = members[0].token_count;
            (
                Group {
                    id: String::new(),
                    tier: Tier::ExactNormalized,
                    // The shared normalized structural hash (spec §2/§6.1).
                    fingerprint: crate::fingerprint::hex(members[0].fingerprint),
                    token_count,
                    value: consolidation_value(
                        &members,
                        substantive_tokens(
                            token_count,
                            crate::ir::substance::boilerplate_mass(&members[0].tree),
                        ),
                    ),
                    note: None,
                    divergence: 0.0,
                    template: None,
                    inline_chain: None,
                    members: members.iter().map(|u| member_of(u)).collect(),
                },
                indices.clone(),
            )
        })
        .collect();
    (groups, below_floor)
}

/// Inline-assisted exact tier (spec §5.4): buckets containing at least one
/// variant whose fingerprint equals other units'. Members report as the
/// variants' BASE units; the inline chain is the evidence. The D3
/// pure-wrapper tautology cannot reach here — a variant exact-equal to a
/// callee it inlined is dropped at creation (see lib.rs).
pub fn build_inline_exact_groups(units: &[Unit], cfg: &Config) -> Vec<(Group, Vec<usize>)> {
    let mut buckets: BTreeMap<(Lang, u128), Vec<usize>> = BTreeMap::new();
    for (idx, unit) in units.iter().enumerate() {
        if unit.token_count < cfg.min_unit_floor() {
            continue;
        }
        buckets
            .entry((unit.lang, unit.fingerprint))
            .or_default()
            .push(idx);
    }
    let mut out = Vec::new();
    for members in buckets.into_values() {
        if members.len() < 2 || members.iter().all(|&i| units[i].variant.is_none()) {
            continue;
        }
        let mut base_members: Vec<usize> = members
            .iter()
            .map(|&i| crate::unit::base_of(units, i))
            .collect();
        base_members.sort_by_key(|&i| (units[i].file.clone(), units[i].byte_span));
        base_members.dedup();
        if base_members.len() < 2 {
            continue;
        }
        // D18: a group needs at least two bases that are NOT exact-equal —
        // equal bases inlining equal callees match by construction (the plain
        // exact tier owns them, or the floor deliberately excluded them).
        let mut base_fps: Vec<u128> = base_members.iter().map(|&i| units[i].fingerprint).collect();
        base_fps.sort_unstable();
        base_fps.dedup();
        if base_fps.len() < 2 {
            continue;
        }
        let chains: Vec<String> = members
            .iter()
            .filter_map(|&i| units[i].chain_line())
            .collect();
        let member_refs: Vec<&Unit> = base_members.iter().map(|&i| &units[i]).collect();
        let token_count = units[members[0]].token_count;
        out.push((
            Group {
                id: String::new(),
                tier: Tier::InlineAssisted,
                // The bucket's shared hash: the converged (inlined) form.
                fingerprint: crate::fingerprint::hex(units[members[0]].fingerprint),
                token_count,
                value: consolidation_value(
                    &member_refs,
                    substantive_tokens(
                        token_count,
                        crate::ir::substance::boilerplate_mass(&units[members[0]].tree),
                    ),
                ),
                note: None,
                divergence: 0.0,
                template: None,
                inline_chain: Some(chains),
                members: member_refs.iter().map(|u| member_of(u)).collect(),
            },
            base_members,
        ));
    }
    out
}

pub fn member_of(unit: &Unit) -> Member {
    Member {
        file: unit.file.clone(),
        lang: unit.lang.name().to_string(),
        name: unit.name.clone(),
        line_span: unit.line_span,
        parse_degraded: unit.parse_degraded,
    }
}

/// Spec §6: `(members - 1) * token_count`, boosted 1.5x when members span
/// ≥2 directories (more likely genuinely missed reuse).
pub fn consolidation_value(members: &[&Unit], token_count: u32) -> f64 {
    let mut value = (members.len() as f64 - 1.0) * f64::from(token_count);
    let first_dir = members[0].file.parent();
    if members.iter().any(|u| u.file.parent() != first_dir) {
        value *= 1.5;
    }
    value
}

/// Fraction of a group's ranking weight preserved even when its matched shape is
/// *entirely* recognized boilerplate — a boilerplate-saturated group is
/// deprioritized, never zeroed. `rank()` only orders (and the report displays the
/// top-N); it never drops a group, so this can only reorder, never lose a finding.
const MIN_SUBSTANCE_FRACTION: f64 = 0.2;

/// Seam C boilerplate discount (docs/native-analysis-overrides-plan.md §5;
/// docs/substantiality-metric.md §0.3): the substantiality input to the
/// consolidation value, = raw tokens minus the mass of recognized boilerplate
/// idioms (`ir::substance`), floored at `MIN_SUBSTANCE_FRACTION` of the raw count.
/// A ranking-only, hash-neutral discount: it feeds `value`, never the fingerprint,
/// the canonical tree, or the displayed `token_count`. Historical-normalizer trees
/// carry no IR kinds, so `boilerplate_mass` returns 0 there (a no-op — the IR path
/// is the calibrated one).
pub fn substantive_tokens(token_count: u32, boilerplate_mass: u32) -> u32 {
    let floor = (f64::from(token_count) * MIN_SUBSTANCE_FRACTION).ceil() as u32;
    token_count
        .saturating_sub(boilerplate_mass)
        .max(floor)
        .max(1)
}

/// Spec §5.6 subsumption: a group entirely nested inside the members of one
/// other group (closures inside matching functions) is suppressed.
fn is_contained_in_other(
    units: &[Unit],
    group: &[usize],
    own_index: usize,
    all: &[Vec<usize>],
) -> bool {
    all.iter().enumerate().any(|(i, outer)| {
        i != own_index
            && group.iter().all(|&gi| {
                let inner = &units[gi];
                outer.iter().any(|&oi| {
                    let out = &units[oi];
                    out.file == inner.file
                        && out.byte_span != inner.byte_span
                        && out.byte_span.0 <= inner.byte_span.0
                        && inner.byte_span.1 <= out.byte_span.1
                })
            })
    })
}

/// Cross-tier membership subsumption (user-reported report gap): a group whose
/// member set is a subset of another group's adds no information — e.g. an
/// inline-assisted superset whose plain-tier subset also reports. Survivor is
/// the LARGER membership at its own tier; if the dropped group held a stronger
/// tier, the survivor is annotated so the confidence signal isn't lost.
/// Unit-granularity tiers only (regions/internal repeats have span members and
/// their own containment rules).
pub fn dedupe_subset_groups(groups: Vec<Group>) -> Vec<Group> {
    use std::collections::BTreeSet;
    fn participates(t: Tier) -> bool {
        matches!(
            t,
            Tier::ExactNormalized | Tier::NearNormalized | Tier::InlineAssisted
        )
    }
    type MemberKey = BTreeSet<(String, String, (u32, u32))>;
    fn members_key(g: &Group) -> MemberKey {
        g.members
            .iter()
            .map(|m| {
                (
                    m.file.to_string_lossy().to_string(),
                    m.name.clone(),
                    m.line_span,
                )
            })
            .collect()
    }
    // Largest membership first, stronger tier first among equals.
    let mut order: Vec<usize> = (0..groups.len()).collect();
    order.sort_by_key(|&i| {
        (
            std::cmp::Reverse(groups[i].members.len()),
            groups[i].tier.fail_rank().unwrap_or(u32::MAX),
        )
    });
    let mut kept: Vec<(usize, MemberKey)> = Vec::new();
    let mut notes: Vec<Option<String>> = vec![None; groups.len()];
    let mut dropped = vec![false; groups.len()];
    for &i in &order {
        if !participates(groups[i].tier) {
            continue;
        }
        let key = members_key(&groups[i]);
        let swallowed_by = kept.iter().find(|(_, kj)| key.is_subset(kj));
        if let Some((j, _)) = swallowed_by {
            let j: usize = *j;
            // Note the loss only when the subset carried a STRONGER tier.
            let stronger = groups[i].tier.fail_rank().unwrap_or(u32::MAX)
                < groups[j].tier.fail_rank().unwrap_or(u32::MAX);
            if stronger {
                let extra = format!(
                    "subsumes a {}-member {} group ({} of these members also group without inlining)",
                    groups[i].members.len(),
                    groups[i].tier,
                    groups[i].members.len(),
                );
                let slot = &mut notes[j];
                *slot = Some(match slot.take() {
                    Some(prev) => format!("{prev}; {extra}"),
                    None => extra,
                });
            }
            dropped[i] = true;
        } else {
            kept.push((i, key));
        }
    }
    let mut out = Vec::with_capacity(groups.len());
    for (i, mut g) in groups.into_iter().enumerate() {
        if dropped[i] {
            continue;
        }
        if let Some(n) = notes[i].take() {
            g.note = Some(match g.note.take() {
                Some(prev) => format!("{prev}; {n}"),
                None => n,
            });
        }
        out.push(g);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn mk(tier: Tier, names: &[&str]) -> Group {
        Group {
            id: String::new(),
            tier,
            fingerprint: String::new(),
            token_count: 50,
            value: 50.0,
            note: None,
            divergence: 0.0,
            template: None,
            inline_chain: None,
            members: names
                .iter()
                .map(|n| Member {
                    file: PathBuf::from(format!("{n}.rs")),
                    lang: "rust".into(),
                    name: (*n).to_string(),
                    line_span: (1, 10),
                    parse_degraded: false,
                })
                .collect(),
        }
    }

    #[test]
    fn subset_group_is_dropped_and_survivor_annotated() {
        // The user-reported shape: an inline-assisted superset (6 members)
        // and a near-normalized subset (5 of those 6).
        let superset = mk(Tier::InlineAssisted, &["a", "b", "c", "d", "e", "f"]);
        let subset = mk(Tier::NearNormalized, &["a", "b", "c", "d", "e"]);
        let out = dedupe_subset_groups(vec![subset, superset]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].tier, Tier::InlineAssisted);
        assert_eq!(out[0].members.len(), 6);
        let note = out[0].note.as_deref().expect("stronger subset noted");
        assert!(note.contains("near-normalized"), "note: {note}");
    }

    #[test]
    fn weaker_subset_dropped_silently() {
        let strong = mk(Tier::ExactNormalized, &["a", "b", "c"]);
        let weak_subset = mk(Tier::InlineAssisted, &["a", "b"]);
        let out = dedupe_subset_groups(vec![strong, weak_subset]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].tier, Tier::ExactNormalized);
        assert!(out[0].note.is_none());
    }

    #[test]
    fn disjoint_and_partial_overlap_groups_survive() {
        let a = mk(Tier::NearNormalized, &["a", "b", "c"]);
        let b = mk(Tier::NearNormalized, &["c", "d", "e"]); // overlap, not subset
        let c = mk(Tier::ExactNormalized, &["x", "y"]);
        assert_eq!(dedupe_subset_groups(vec![a, b, c]).len(), 3);
    }

    #[test]
    fn regions_do_not_participate() {
        let region_sub = mk(Tier::ExactRegion, &["a", "b"]);
        let unit_super = mk(Tier::NearNormalized, &["a", "b", "c"]);
        assert_eq!(dedupe_subset_groups(vec![region_sub, unit_super]).len(), 2);
    }
}
