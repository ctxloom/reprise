#!/usr/bin/env python3
"""§7.3 baseline comparison harness (spec §7.3 / §6.1).

Mechanically diffs reprise's findings against a jscpd (or PMD/CPD) run on the
same repo, matching clone locations by file + line-range overlap. Because
reprise emits jscpd-shaped JSON (`--format jscpd`), this is a near-mechanical
diff rather than a hand reconciliation (§6.1).

Agreement model: reprise reports a *group* (a set of ≥2 clone members); jscpd
reports a *pair*. They AGREE when a jscpd pair's two endpoints both land inside
one reprise group (each endpoint overlaps a distinct member). Then:
  - agreed         : reprise groups overlapping ≥1 jscpd pair (and vice versa);
  - reprise-only   : reprise groups no jscpd pair lands in — the Type-2/3/4
                     catches beyond jscpd's token-window reach;
  - jscpd-only     : jscpd pairs no reprise group covers — sub-function windows
                     under reprise's floors, or test-policy differences.

Usage:
  compare.py --reprise reprise.json --jscpd jscpd-report.json \
             --lang rust --root /path/to/repo [--json out.json]
"""

import argparse
import json
import os
import sys
from collections import Counter


def overlap(a, b):
    """True if two (file, start, end) locations are in the same file with
    intersecting inclusive line ranges."""
    return a[0] == b[0] and a[1] <= b[2] and b[1] <= a[2]


def norm_path(p, root):
    """reprise emits absolute member paths; jscpd emits root-relative. Normalize
    reprise paths to root-relative, slash-separated, for comparison."""
    if root and os.path.isabs(p):
        try:
            p = os.path.relpath(p, root)
        except ValueError:
            pass
    return p.replace(os.sep, "/")


def load_reprise(path, root):
    """Reprise groups (main + test + weak) as {tier, section, members:[loc]}.
    api-profile is excluded (structurally different, no line-overlap meaning)."""
    d = json.load(open(path))
    groups = []
    for section, key in [("main", "groups"), ("test", "test_groups"), ("weak", "weak_groups")]:
        for g in d.get(key, []):
            members = []
            for m in g["members"]:
                s, e = m["line_span"]
                members.append((norm_path(m["file"], root), int(s), int(e)))
            if len(members) >= 1:
                groups.append({"tier": g["tier"], "section": section, "members": members,
                               "value": g.get("value", 0.0), "template": g.get("template")})
    return groups, d.get("stats", {})


def load_jscpd(path, lang):
    """jscpd duplicate pairs filtered to one language format: (locA, locB)."""
    d = json.load(open(path))
    pairs = []
    for x in d.get("duplicates", []):
        if lang and x.get("format") != lang:
            continue
        f, s = x["firstFile"], x["secondFile"]
        a = (f["name"].replace(os.sep, "/"), int(f["start"]), int(f["end"]))
        b = (s["name"].replace(os.sep, "/"), int(s["start"]), int(s["end"]))
        pairs.append((a, b))
    return pairs, d.get("statistics", {}).get("total", {})


def group_covers_pair(group, pair):
    """A jscpd pair lands in a reprise group iff each endpoint overlaps some
    member (endpoints matched to distinct members)."""
    b1, b2 = pair
    m1 = [i for i, m in enumerate(group["members"]) if overlap(m, b1)]
    m2 = [i for i, m in enumerate(group["members"]) if overlap(m, b2)]
    if not m1 or not m2:
        return False
    # distinct members (a single member covering both endpoints is a within-
    # member overlap, not a cross-member group match)
    return any(i != j for i in m1 for j in m2)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--reprise", required=True)
    ap.add_argument("--jscpd", required=True)
    ap.add_argument("--lang", default="", help="jscpd format to keep (rust|python|...)")
    ap.add_argument("--root", default="", help="repo root, to relativize reprise paths")
    ap.add_argument("--json", default="", help="write machine-readable result here")
    args = ap.parse_args()

    groups, rstats = load_reprise(args.reprise, args.root)
    jpairs, jstats = load_jscpd(args.jscpd, args.lang)

    # reprise group ↔ jscpd pair matching
    group_matched = [False] * len(groups)
    pair_covered = [False] * len(jpairs)
    for pi, pair in enumerate(jpairs):
        for gi, g in enumerate(groups):
            if group_covers_pair(g, pair):
                pair_covered[pi] = True
                group_matched[gi] = True

    reprise_only = [g for gi, g in enumerate(groups) if not group_matched[gi]]
    reprise_agreed = [g for gi, g in enumerate(groups) if group_matched[gi]]
    jscpd_only = [p for pi, p in enumerate(jpairs) if not pair_covered[pi]]

    tier_of_only = Counter(g["tier"] for g in reprise_only)
    tier_of_agreed = Counter(g["tier"] for g in reprise_agreed)

    print(f"== reprise vs jscpd ({args.lang or 'all'}) ==")
    print(f"reprise groups (main+test+weak): {len(groups)}")
    print(f"jscpd pairs ({args.lang}):        {len(jpairs)}")
    print(f"AGREE   (reprise groups overlapping a jscpd pair): {len(reprise_agreed)}")
    print(f"AGREE   (jscpd pairs covered by a reprise group):  {sum(pair_covered)}")
    print(f"REPRISE-ONLY groups: {len(reprise_only)}  by tier: {dict(tier_of_only)}")
    print(f"JSCPD-ONLY pairs:    {len(jscpd_only)}")
    print(f"reprise-agreed by tier: {dict(tier_of_agreed)}")

    # Sample 10 reprise-only, stratified toward the interesting near/inline tiers.
    print("\n-- reprise-only sample (up to 10) --")
    order = {"near-normalized": 0, "inline-assisted": 1, "exact-region": 2,
             "internal-repeat": 3, "exact-normalized": 4, "weak-similarity": 5}
    sample = sorted(reprise_only, key=lambda g: (order.get(g["tier"], 9), -g["value"]))[:10]
    for g in sample:
        locs = "; ".join(f"{m[0]}:{m[1]}-{m[2]}" for m in g["members"][:3])
        more = f" (+{len(g['members'])-3} more)" if len(g["members"]) > 3 else ""
        print(f"  [{g['tier']}/{g['section']}] {locs}{more}")

    print("\n-- jscpd-only sample (up to 5) --")
    for p in jscpd_only[:5]:
        print(f"  {p[0][0]}:{p[0][1]}-{p[0][2]}  <->  {p[1][0]}:{p[1][1]}-{p[1][2]}")

    if args.json:
        json.dump({
            "reprise_groups": len(groups), "jscpd_pairs": len(jpairs),
            "agree_reprise_groups": len(reprise_agreed),
            "agree_jscpd_pairs": sum(pair_covered),
            "reprise_only_groups": len(reprise_only),
            "reprise_only_by_tier": dict(tier_of_only),
            "agreed_by_tier": dict(tier_of_agreed),
            "jscpd_only_pairs": len(jscpd_only),
            "reprise_only_sample": [
                {"tier": g["tier"], "section": g["section"],
                 "members": [f"{m[0]}:{m[1]}-{m[2]}" for m in g["members"]]}
                for g in sample],
            "jscpd_only_sample": [
                {"first": f"{p[0][0]}:{p[0][1]}-{p[0][2]}",
                 "second": f"{p[1][0]}:{p[1][1]}-{p[1][2]}"} for p in jscpd_only[:5]],
            "reprise_stats": {k: rstats.get(k) for k in
                              ("total_lines", "duplicated_lines", "duplicated_lines_pct",
                               "duplicated_tokens_pct", "clones_per_kloc")},
            "jscpd_stats": {k: jstats.get(k) for k in
                            ("lines", "duplicatedLines", "percentage", "clones")},
        }, open(args.json, "w"), indent=2)
        print(f"\nwrote {args.json}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
