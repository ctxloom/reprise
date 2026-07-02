//! P4: sibling-run folding / re-roll (spec §5.3).
//!
//! Runs of ≥ `fold_min_repeats` consecutive sibling groups (period 1..=4)
//! that are identical under MaskedAll hashing (locals AND literals masked)
//! collapse into a `REPEAT` node whose children are the first period's
//! subtrees. The repeat count is deliberately NOT part of the structural
//! identity (label = LitBucket(Int)), so REPEAT×3 converges with REPEAT×4 and
//! with the rolled loop (via the AU special case in `au.rs`).

use crate::fingerprint::{HashMode, merkle_mode};
use crate::lang::LanguageProfile;
use crate::tree::{Bucket, Label, NormNode};

#[derive(Debug, Clone, Copy)]
pub struct RepeatFinding {
    pub byte_span: (u32, u32),
    pub count: u32,
    pub template_tokens: u32,
    /// MaskedAll structural hash of one period — the run's identity. Feeds the
    /// baseline fingerprint key (unit fingerprint, template hash) (spec §2).
    pub template_hash: u128,
}

const MAX_PERIOD: usize = 8;

pub fn fold_repeats(
    node: NormNode,
    profile: &dyn LanguageProfile,
    min_repeats: usize,
    findings: &mut Vec<RepeatFinding>,
) -> NormNode {
    let mut node = node;
    node.children = node
        .children
        .into_iter()
        .map(|c| fold_repeats(c, profile, min_repeats, findings))
        .collect();
    if !profile.is_list_kind(&node.kind) || node.children.len() < min_repeats {
        return node;
    }

    let hashes: Vec<u128> = node
        .children
        .iter()
        .map(|c| merkle_mode(c, HashMode::MaskedAll))
        .collect();
    let n = node.children.len();
    let mut out: Vec<NormNode> = Vec::with_capacity(n);
    let mut children: Vec<Option<NormNode>> = node.children.into_iter().map(Some).collect();
    let mut i = 0;
    while i < n {
        let mut folded = false;
        for period in 1..=MAX_PERIOD.min((n - i) / min_repeats) {
            let mut reps = 1;
            while i + (reps + 1) * period <= n
                && (0..period).all(|j| hashes[i + reps * period + j] == hashes[i + j])
            {
                reps += 1;
            }
            if reps >= min_repeats {
                let template: Vec<NormNode> = (0..period)
                    .map(|j| children[i + j].take().unwrap())
                    .collect();
                let start = template[0].span.0;
                let end = children[i + reps * period - 1]
                    .as_ref()
                    .map(|c| c.span.1)
                    .unwrap_or(template.last().unwrap().span.1);
                let template_tokens: u32 = template.iter().map(NormNode::token_count).sum();
                let mut buf = Vec::with_capacity(16 * period);
                for h in &hashes[i..i + period] {
                    buf.extend_from_slice(&h.to_le_bytes());
                }
                // Dispatch tables (all template nodes are match/case arms)
                // fold but are not FINDINGS — an enum→value table is
                // idiomatic, not actionable duplication (D30).
                if !template.iter().all(|t| profile.is_dispatch_arm(&t.kind)) {
                    findings.push(RepeatFinding {
                        byte_span: (start, end),
                        count: reps as u32,
                        template_tokens,
                        template_hash: xxhash_rust::xxh3::xxh3_128(&buf),
                    });
                }
                let repeat = NormNode::new("REPEAT", None, (start, end), template)
                    .with_label(Label::LitBucket(Bucket::Int));
                out.push(repeat);
                i += reps * period;
                folded = true;
                break;
            }
        }
        if !folded {
            out.push(children[i].take().unwrap());
            i += 1;
        }
    }
    node.children = out;
    node
}
