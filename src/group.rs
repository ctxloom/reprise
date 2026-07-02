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
        if unit.token_count < cfg.thresholds.min_unit_tokens {
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
                    value: consolidation_value(&members, token_count),
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
        if unit.token_count < cfg.thresholds.min_unit_tokens {
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
                value: consolidation_value(&member_refs, token_count),
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
