//! P7 (exact slice) + P8 assembly helpers: exact-hash bucketing, containment
//! suppression, and consolidation-value ranking (spec §5.6, §6).

use crate::config::Config;
use crate::lang::Lang;
use crate::report::{Group, Member, Tier};
use crate::unit::Unit;
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::Path;

/// Exact tier (plain units only): returns groups plus their member unit
/// indices (the indices feed sequence-tier subsumption) and the below-floor
/// count. Variants are handled by `build_inline_exact_groups`. `digests` is
/// index-aligned with `units` (the fused digest pass carries the ranking's
/// boilerplate mass, so this tier reads no trees).
pub fn build_exact_groups(
    units: &[Unit],
    digests: Option<&[crate::digest::UnitDigest]>,
    cfg: &Config,
) -> (Vec<(Group, Vec<usize>)>, u32) {
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

    let contained = contained_flags(units, &raw_groups);
    let kept: Vec<&Vec<usize>> = raw_groups
        .iter()
        .enumerate()
        .filter(|(i, _)| !contained[*i])
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
                        // Over the gate the fused digest carries the ranking's
                        // boilerplate mass (== `boilerplate_mass(tree)`, pinned by
                        // tests/digest_oracle.rs); under it the tree is resident
                        // and walked directly, exactly as before the memory work.
                        substantive_tokens(
                            token_count,
                            unit_boilerplate(digests, indices[0], &units[indices[0]]),
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
pub fn build_inline_exact_groups(
    units: &[Unit],
    digests: Option<&[crate::digest::UnitDigest]>,
    cfg: &Config,
) -> Vec<(Group, Vec<usize>)> {
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
                    // Same dual-source rule; a variant can be the bucket
                    // representative, which is why variants carry this field too.
                    substantive_tokens(
                        token_count,
                        unit_boilerplate(digests, members[0], &units[members[0]]),
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

/// The exact/inline-exact ranking's boilerplate mass for one unit: read off the
/// fused digest over the memory gate, or computed from the resident tree under it
/// (the SAME `ir::substance::boilerplate_mass` either way — the digest is pinned
/// to it by tests/digest_oracle.rs).
fn unit_boilerplate(digests: Option<&[crate::digest::UnitDigest]>, idx: usize, unit: &Unit) -> u32 {
    match digests {
        Some(d) => d[idx].boilerplate_mass,
        None => crate::ir::substance::boilerplate_mass(unit.tree.expect_resident()),
    }
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

/// Per-file span index over raw exact-groups' members, built once and shared
/// across every group's containment check (spec §5.6). Containment can only
/// ever hold between members that share a file, so bucketing by file collapses
/// the O(raw_groups²) all-pairs scan down to work proportional to how many
/// raw-group members actually land in the same file — small even in a
/// duplication-heavy corpus, since distinct checkouts of the same tree don't
/// share file paths.
struct SpanIndex<'u> {
    // file -> (start, end, owning raw-group index), sorted by start ascending
    // (ties by end descending, immaterial to correctness — only lets the
    // `partition_point` prefix end as early as possible).
    by_file: HashMap<&'u Path, Vec<(u32, u32, usize)>>,
}

impl<'u> SpanIndex<'u> {
    fn build(units: &'u [Unit], raw_groups: &[Vec<usize>]) -> Self {
        let mut by_file: HashMap<&'u Path, Vec<(u32, u32, usize)>> = HashMap::new();
        for (gi, group) in raw_groups.iter().enumerate() {
            for &ui in group {
                let u = &units[ui];
                by_file.entry(u.file.as_path()).or_default().push((
                    u.byte_span.0,
                    u.byte_span.1,
                    gi,
                ));
            }
        }
        for spans in by_file.values_mut() {
            spans.sort_unstable_by_key(|&(start, end, _)| (start, std::cmp::Reverse(end)));
        }
        Self { by_file }
    }

    /// Distinct raw-group indices (sorted, deduped) with a member in `file`
    /// that properly contains `(start, end)` — mirrors the original pairwise
    /// check: same file, spans not identical, and non-strict containment on
    /// both endpoints. `exclude` is the querying group itself (never its own
    /// container).
    fn containers(&self, file: &Path, start: u32, end: u32, exclude: usize, out: &mut Vec<usize>) {
        out.clear();
        let Some(spans) = self.by_file.get(file) else {
            return;
        };
        // Sorted by start ascending: every candidate outer span has start <=
        // start, so this prefix is exactly (and only) the containment
        // candidates left to filter by end.
        let upper = spans.partition_point(|&(s, _, _)| s <= start);
        for &(s, e, gi) in &spans[..upper] {
            if gi != exclude && e >= end && (s, e) != (start, end) {
                out.push(gi);
            }
        }
        out.sort_unstable();
        out.dedup();
    }
}

/// Spec §5.6 subsumption: a group entirely nested inside the members of one
/// other group (closures inside matching functions) is suppressed. Formerly an
/// O(raw_groups²) nested scan (see git history for the brute-force form kept
/// as a differential-test oracle below); replaced with the per-file
/// `SpanIndex` since containment is only possible within a shared file. Each
/// group's flag is independent of every other group's (the check is always
/// against `raw_groups`, never the filtered survivors), so the flags can be
/// computed in parallel with no effect on the result.
fn contained_flags(units: &[Unit], raw_groups: &[Vec<usize>]) -> Vec<bool> {
    let index = SpanIndex::build(units, raw_groups);
    raw_groups
        .par_iter()
        .enumerate()
        .map(|(own_index, group)| is_contained_in_other(units, &index, group, own_index))
        .collect()
}

/// True if every member of `group` is properly contained (same file, strict
/// span containment) by SOME member of one other group `h` — possibly a
/// different `h`-member per `group`-member. Existence-only (spec §5.6 doesn't
/// pick a "winning" container, it only suppresses), so there is no
/// order-dependent tie-break to preserve.
fn is_contained_in_other(
    units: &[Unit],
    index: &SpanIndex,
    group: &[usize],
    own_index: usize,
) -> bool {
    // counts[h] = number of this group's members contained by some member of h;
    // h qualifies once its count reaches every member.
    let mut counts: HashMap<usize, u32> = HashMap::new();
    let mut candidates = Vec::new();
    for &gi in group {
        let inner = &units[gi];
        index.containers(
            &inner.file,
            inner.byte_span.0,
            inner.byte_span.1,
            own_index,
            &mut candidates,
        );
        // No group at all contains this member => no single outer group can
        // contain every member (matches the original short-circuit via `.all()`).
        if candidates.is_empty() {
            return false;
        }
        for &h in &candidates {
            *counts.entry(h).or_insert(0) += 1;
        }
    }
    counts.values().any(|&c| c as usize == group.len())
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
    type MemberElem = (String, String, (u32, u32));
    type MemberKey = BTreeSet<MemberElem>;
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
    /// Position in `kept` of the first (lowest-index / earliest-inserted)
    /// entry whose key is a superset of `key` — identical semantics to
    /// `kept.iter().find(|(_, kj)| key.is_subset(kj))`, using `member_index`
    /// to skip entries that cannot possibly qualify.
    fn find_superset_position(
        kept: &[(usize, MemberKey)],
        member_index: &HashMap<MemberElem, Vec<usize>>,
        key: &MemberKey,
    ) -> Option<usize> {
        if key.is_empty() {
            // An empty key is trivially a subset of anything; mirror
            // `.find()`'s first-entry semantics directly. Never hit by real
            // findings (every group has >= 2 members) — kept for exact
            // equivalence with the brute-force reference.
            return if kept.is_empty() { None } else { Some(0) };
        }
        let pivot = key
            .iter()
            .min_by_key(|elem| member_index.get(*elem).map_or(0, Vec::len))?;
        let candidates = member_index.get(pivot)?;
        candidates
            .iter()
            .copied()
            .find(|&pos| key.is_subset(&kept[pos].1))
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
    // Inverted index: one member-tuple -> the (ascending, insertion-order)
    // `kept` positions whose key contains it. `key.is_subset(kj)` can only
    // hold when `kj` contains EVERY element of `key`, so the candidates for
    // ANY single element of `key` are a superset of every actual match —
    // scanning just one element's posting list (the smallest, to minimize
    // work) instead of rescanning the whole of `kept` mirrors the
    // e787158/77f820f idiom (index once, look up instead of a full rescan)
    // applied to a subset-of-a-set query. `kept.iter().find(..)` here used to
    // rescan every already-kept group per candidate — O(participating²),
    // measured as ~80% of the assemble phase and the dominant superlinear
    // cost at kernel scale (`sleek-glade-assemble` perf writeup). Posting
    // lists are appended in the same order `kept` grows, so they stay
    // ascending — scanning one in order and taking the first full match
    // reproduces `.find()`'s "first entry in `kept`" tie-break exactly (every
    // full-superset position necessarily appears in each of `key`'s
    // elements' posting lists, since it must contain them too, so
    // restricting to one list never misses — and never reorders — a match).
    let mut member_index: HashMap<(String, String, (u32, u32)), Vec<usize>> = HashMap::new();
    let mut notes: Vec<Option<String>> = vec![None; groups.len()];
    let mut dropped = vec![false; groups.len()];
    for &i in &order {
        if !participates(groups[i].tier) {
            continue;
        }
        let key = members_key(&groups[i]);
        let swallowed_by = find_superset_position(&kept, &member_index, &key);
        if let Some(pos) = swallowed_by {
            let j = kept[pos].0;
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
            let pos = kept.len();
            for elem in &key {
                member_index.entry(elem.clone()).or_default().push(pos);
            }
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

    fn mk_unit(file: &str, span: (u32, u32)) -> Unit {
        Unit {
            file: PathBuf::from(file),
            lang: crate::lang::Lang::Rust,
            name: String::new(),
            byte_span: span,
            line_span: (0, 0),
            token_count: 10,
            parse_degraded: false,
            is_test: false,
            accept_drift: false,
            fingerprint: 0,
            tree: crate::unit::TreeSlot::Resident(crate::tree::NormNode::new(
                "Unit",
                None,
                span,
                Vec::new(),
            )),
            variant: None,
        }
    }

    /// The original O(raw_groups²) all-pairs scan, kept only here as the
    /// differential-test oracle for the `SpanIndex`-based `contained_flags`.
    fn brute_contained_flags(units: &[Unit], raw_groups: &[Vec<usize>]) -> Vec<bool> {
        (0..raw_groups.len())
            .map(|own_index| {
                let group = &raw_groups[own_index];
                raw_groups.iter().enumerate().any(|(i, outer)| {
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
            })
            .collect()
    }

    #[test]
    fn contained_flags_flags_group_nested_in_another_same_file() {
        // group 0: an outer function spanning most of the file, duplicated in
        // two files. group 1: a closure nested strictly inside it, also
        // duplicated in the same two files => group 1 is suppressed.
        let units = vec![
            mk_unit("a.rs", (0, 100)), // 0: outer in a.rs
            mk_unit("b.rs", (0, 100)), // 1: outer in b.rs
            mk_unit("a.rs", (10, 20)), // 2: inner in a.rs
            mk_unit("b.rs", (10, 20)), // 3: inner in b.rs
        ];
        let raw_groups = vec![vec![0, 1], vec![2, 3]];
        assert_eq!(contained_flags(&units, &raw_groups), vec![false, true]);
    }

    #[test]
    fn contained_flags_requires_same_file() {
        // The inner span sits inside outer's numeric range but in a DIFFERENT
        // file — must not count as containment.
        let units = vec![mk_unit("a.rs", (0, 100)), mk_unit("b.rs", (10, 20))];
        let raw_groups = vec![vec![0], vec![1]];
        assert_eq!(contained_flags(&units, &raw_groups), vec![false, false]);
    }

    #[test]
    fn contained_flags_ignores_identical_spans() {
        // Same file, same span, different group: not containment (the
        // original excludes exact byte_span equality).
        let units = vec![mk_unit("a.rs", (10, 20)), mk_unit("a.rs", (10, 20))];
        let raw_groups = vec![vec![0], vec![1]];
        assert_eq!(contained_flags(&units, &raw_groups), vec![false, false]);
    }

    #[test]
    fn contained_flags_needs_every_member_covered() {
        // group 1 has two members in a.rs; only one is nested inside group
        // 0's a.rs member, and there is no b.rs container at all — group 1
        // must NOT be suppressed since not every member is covered.
        let units = vec![
            mk_unit("a.rs", (0, 100)), // 0: outer in a.rs only
            mk_unit("a.rs", (10, 20)), // 1: covered inner in a.rs
            mk_unit("b.rs", (10, 20)), // 2: uncovered inner in b.rs
        ];
        let raw_groups = vec![vec![0], vec![1, 2]];
        assert_eq!(contained_flags(&units, &raw_groups), vec![false, false]);
    }

    /// The original O(participating²) all-pairs rescan, kept only here as the
    /// differential-test oracle for the indexed `dedupe_subset_groups`.
    fn brute_dedupe_subset_groups(groups: Vec<Group>) -> Vec<Group> {
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

    type MemberSignature = (String, String, (u32, u32));
    type GroupSignature = (Tier, Option<String>, Vec<MemberSignature>);

    /// Projection used to compare oracle vs. indexed output without requiring
    /// `Group: PartialEq` — tier, note, and the member key set are the only
    /// fields `dedupe_subset_groups` can affect.
    fn group_signature(g: &Group) -> GroupSignature {
        let mut members: Vec<MemberSignature> = g
            .members
            .iter()
            .map(|m| {
                (
                    m.file.to_string_lossy().to_string(),
                    m.name.clone(),
                    m.line_span,
                )
            })
            .collect();
        members.sort();
        (g.tier, g.note.clone(), members)
    }

    fn mk_at(tier: Tier, members: &[(&str, &str, (u32, u32))]) -> Group {
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
            members: members
                .iter()
                .map(|(file, name, span)| Member {
                    file: PathBuf::from(*file),
                    lang: "rust".into(),
                    name: (*name).to_string(),
                    line_span: *span,
                    parse_degraded: false,
                })
                .collect(),
        }
    }

    #[test]
    fn dedupe_subset_groups_matches_brute_force_oracle() {
        // Deterministic LCG (no external rand dependency), mirroring the
        // `contained_flags`/`test_unit_key_index` oracle tests: many random
        // group shapes over a small file/name/span alphabet so subset
        // relations (and ties) actually occur, checked against the original
        // O(participating²) rescan.
        struct Lcg(u64);
        impl Lcg {
            fn next(&mut self) -> u64 {
                self.0 = self
                    .0
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                self.0
            }
            fn range(&mut self, n: u32) -> u32 {
                (self.next() % u64::from(n)) as u32
            }
        }
        let tiers = [
            Tier::ExactNormalized,
            Tier::NearNormalized,
            Tier::InlineAssisted,
            Tier::ExactRegion, // non-participating control
            Tier::WeakSimilarity,
        ];

        for seed in 0..200u64 {
            let mut rng = Lcg(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1));
            let file_count = 1 + rng.range(2);
            let files: Vec<String> = (0..file_count).map(|i| format!("f{i}.rs")).collect();
            let name_count = 1 + rng.range(3);
            let names: Vec<String> = (0..name_count).map(|i| format!("n{i}")).collect();

            let group_count = 1 + rng.range(10);
            let mut groups: Vec<Group> = Vec::new();
            for _ in 0..group_count {
                let tier = tiers[rng.range(tiers.len() as u32) as usize];
                // Small member alphabet + small span alphabet so members
                // frequently coincide across groups, producing real subset
                // and tie shapes rather than all-disjoint sets.
                let member_count = 1 + rng.range(4);
                let mut members: Vec<(&str, &str, (u32, u32))> = Vec::new();
                for _ in 0..member_count {
                    let file = &files[rng.range(file_count) as usize];
                    let name = &names[rng.range(name_count) as usize];
                    let start = rng.range(3) * 10;
                    members.push((file.as_str(), name.as_str(), (start, start + 10)));
                }
                groups.push(mk_at(tier, &members));
            }

            let expected: Vec<_> = brute_dedupe_subset_groups(groups.clone())
                .iter()
                .map(group_signature)
                .collect();
            let actual: Vec<_> = dedupe_subset_groups(groups)
                .iter()
                .map(group_signature)
                .collect();
            assert_eq!(actual, expected, "seed {seed}");
        }
    }

    #[test]
    fn contained_flags_matches_brute_force_oracle() {
        // Deterministic LCG (no external rand dependency) exercising many
        // random file/span/group shapes against the O(n²) oracle above.
        struct Lcg(u64);
        impl Lcg {
            fn next(&mut self) -> u64 {
                self.0 = self
                    .0
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                self.0
            }
            fn range(&mut self, n: u32) -> u32 {
                (self.next() % u64::from(n)) as u32
            }
        }

        for seed in 0..50u64 {
            let mut rng = Lcg(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1));
            let file_count = 1 + rng.range(3);
            let files: Vec<String> = (0..file_count).map(|i| format!("f{i}.rs")).collect();

            let mut units: Vec<Unit> = Vec::new();
            let mut raw_groups: Vec<Vec<usize>> = Vec::new();
            let group_count = 2 + rng.range(6);
            for _ in 0..group_count {
                let member_count = 2 + rng.range(3);
                let mut members = Vec::new();
                for _ in 0..member_count {
                    let file = &files[rng.range(file_count) as usize];
                    let start = rng.range(20);
                    let len = 1 + rng.range(10);
                    let span = (start, start + len);
                    units.push(mk_unit(file, span));
                    members.push(units.len() - 1);
                }
                raw_groups.push(members);
            }

            let expected = brute_contained_flags(&units, &raw_groups);
            let actual = contained_flags(&units, &raw_groups);
            assert_eq!(actual, expected, "seed {seed}");
        }
    }
}
