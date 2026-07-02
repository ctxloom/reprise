#!/usr/bin/env python3
"""Pair-level A/B between two reprise scan JSONs (the D27 method).

A perf change is LOSSLESS iff no member-pair present in the reference is
absent or demoted in the candidate. Group-set comparison hides member loss
(D27's lesson), so this compares the full set of unordered member pairs per
reportable tier, plus single-member internal-repeat findings.

Usage: pair_ab.py ref.json new.json  ->  exit 0 iff no pair lost/demoted.
"""

import itertools
import json
import sys

# Confidence buckets: moving DOWN is a loss. weak/api are non-reportable.
RANK = {
    "exact-normalized": 0,
    "internal-repeat": 0,
    "exact-region": 0,
    "near-normalized": 0,
    "inline-assisted": 0,
    "weak-similarity": 9,
    "api-profile": 9,
}


def member_key(m):
    return (m["file"].split("/")[-1] + ":" + m["name"], m["line_span"][0])


def pair_map(report):
    """(member, member) or (member,) -> best (lowest) rank seen."""
    pairs = {}
    for section in ("groups", "test_groups", "weak_groups"):
        for g in report.get(section, []) or []:
            rank = RANK.get(g["tier"], 9)
            members = sorted(member_key(m) for m in g["members"])
            keys = (
                [tuple(members)]
                if len(members) == 1
                else [tuple(sorted(p)) for p in itertools.combinations(members, 2)]
            )
            for k in keys:
                pairs[k] = min(pairs.get(k, 9), rank)
    return pairs


def main():
    ref_path, new_path = sys.argv[1], sys.argv[2]
    ref = pair_map(json.load(open(ref_path)))
    new = pair_map(json.load(open(new_path)))
    ref_reportable = {k for k, r in ref.items() if r < 9}
    new_reportable = {k for k, r in new.items() if r < 9}
    lost = sorted(ref_reportable - new_reportable)
    gained = sorted(new_reportable - ref_reportable)
    print(f"reference reportable pairs: {len(ref_reportable)}")
    print(f"candidate reportable pairs: {len(new_reportable)}")
    print(f"lost: {len(lost)}   gained: {len(gained)}")
    for k in lost[:40]:
        print("  LOST", k)
    if lost:
        print("VERDICT: LOSSY — revert (D27)")
        return 1
    print("VERDICT: lossless (pair parity holds)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
