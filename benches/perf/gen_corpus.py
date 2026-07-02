#!/usr/bin/env python3
"""Synthetic ~500k-LOC Rust corpus for the M3b perf gate (spec §8: PR check
< 5s warm). Functions are sampled from a statement pool; roughly half the
statement instances call a function-unique external helper, so units are
structurally varied and don't collapse into corpus-wide clone classes (a
first attempt with one shared skeleton made every function near-match every
other — no real repo looks like that). 20 planted 3-member clone families
exercise the baseline/check path.
"""
import os
import random
import sys

ROOT = sys.argv[1]
FILES = 1000
FNS_PER_FILE = 24
rng = random.Random(42)

# Generic small statements (each well under half of min_seq_tokens once
# normalized, so even runs of a few identical picks stay below the region
# floor) and unique-call statements (carry a per-instance external name).
# Generic statements are COMPOSED from random expression trees rather than
# drawn from a small fixed pool: masked locals and bucketed literals collapse
# most surface variety, so only tree shape / operator / keep-list-literal
# differences survive normalization — a fixed pool of 10 forms collides
# birthday-style across 24k functions and tiles the corpus with exact-region
# bridges. Real code varies structurally; this approximates that.
G_ATOMS = ["acc", "count", "{k}", "0", "1"]
G_OPS = ["-", "/", "%", "<<", ">>", "^", "+", "*"]


def gen_rhs(rng) -> str:
    a, b = rng.choice(G_ATOMS), rng.choice(G_ATOMS)
    op1, op2 = rng.choice(G_OPS), rng.choice(G_OPS)
    shape = rng.random()
    if shape < 0.4:
        return f"{a} {op1} {b}"
    c = rng.choice(G_ATOMS)
    if shape < 0.7:
        return f"({a} {op1} {b}) {op2} {c}"
    return f"{a} {op1} ({b} {op2} {c})"


def gen_generic(rng, k: int) -> str:
    lhs = rng.choice(["acc", "count"])
    op = rng.choice(["+=", "-=", "^=", "*=", "/=", "%=", "="])
    return f"{lhs} {op} {gen_rhs(rng)};".format(k=k)
UNIQUE = [
    "acc += helper_{fid}_{j}({args});",
    "count = helper_{fid}_{j}({args});",
    "acc -= helper_{fid}_{j}({args}) % {k2};",
    "if acc > {k} {{ acc = helper_{fid}_{j}({args}); }}",
    "while count > helper_{fid}_{j}({args}) {{ count -= {k}; }}",
    # The unique call sits in the RANGE so the loop-lowering machinery
    # (which duplicates the iterated expression into __has_next/__next)
    # carries the unique external twice — no constant machinery bridge.
    "for x in 0..helper_{fid}_{j}({args}) {{ acc += x * {k}; }}",
]
ARGS = ["acc", "count", "acc, count", "count, acc", "acc, count, {k}", "count, {k}"]

# Family-distinct planted clones: the audit_{gid} external call keeps the 20
# families from collapsing into one corpus-wide exact group (external names
# are kept by identifier abstraction).
CLONE_TEMPLATE = """\
pub fn planted_{gid}_{copy}({xs}: &[(String, i64)], cutoff: i64) -> Vec<String> {{
    let mut out = Vec::new();
    let mut {acc} = 0;
    for (label, score) in {xs} {{
        if *score < cutoff {{
            continue;
        }}
        {acc} += score;
        let grade = if *score >= {lit} {{ "high" }} else {{ "low" }};
        out.push(format!("{{label}}: {{score}} ({{grade}})"));
    }}
    let checked = audit_{gid}(&out, {acc});
    if {acc} > {lit2} && checked > 0 {{
        out.push(String::from("aggregate: high"));
    }}
    out
}}
"""

NAMES = ["entries", "items", "rows", "records"]
ACCS = ["total", "acc", "tally", "summed"]


def make_fn(fid: int) -> str:
    # Unique seed/init calls directly after the header: real functions differ
    # immediately (names, types, arity); without this every pair of functions
    # shares a ≥30-token normalized prefix and the sequence tier tiles the
    # whole corpus.
    ty = rng.choice(["i64", "u64", "i32", "usize", "isize"])
    lines = [
        f"pub fn func_{fid}(items: &[{ty}], limit: {ty}) -> {ty} {{",
        f"    let mut acc = seed_{fid}(limit);",
        f"    let mut count = init_{fid}(acc, {rng.randint(2, 9)});",
    ]
    body_n = rng.randint(4, 7)
    loop_n = rng.randint(2, 4)
    tail_n = rng.randint(3, 5)

    def stmt(j: int, indent: str, unique: bool) -> str:
        k = rng.randint(2, 9)
        if not unique:
            return indent + gen_generic(rng, k)
        form = rng.choice(UNIQUE)
        return indent + form.format(
            fid=fid,
            j=j,
            k=k,
            k2=rng.randint(2, 9),
            args=rng.choice(ARGS).format(k=k),
        )

    # Alternate unique/generic so no cross-unit token run can span more than
    # one generic statement (unique external names break every run — the same
    # reason real code doesn't tile into corpus-wide exact regions).
    j = 0
    for i in range(body_n):
        lines.append(stmt(j, "    ", i % 2 == 0))
        j += 1
    # Unit-unique iterated expression: the lowering duplicates it into the
    # __has_next/__next machinery, so the loop-header token run can never
    # match across units (the way distinct field/method chains do in real
    # code — a shared pool here collides birthday-style at 24k functions).
    iterated = rng.choice(
        [
            f"stream_{fid}(items)",
            f"stream_{fid}(items).iter()",
            f"items.iter().map(|v| shift_{fid}(v))",
            f"items.iter().filter(|v| keep_{fid}(**v))",
            f"stream_{fid}(items).chunks({rng.randint(2, 5)})",
        ]
    )
    lines.append(f"    for it in {iterated} {{")
    lines.append(f"        acc += helper_{fid}_loop(*it, count);")
    for i in range(loop_n):
        lines.append(stmt(j, "        ", i % 2 == 1))
        j += 1
    lines.append("    }")
    for i in range(tail_n):
        lines.append(stmt(j, "    ", i % 2 == 0))
        j += 1
    lines.append("    acc - count")
    lines.append("}")
    return "\n".join(lines) + "\n"


os.makedirs(ROOT, exist_ok=True)
loc = 0
fid = 0
for f in range(FILES):
    parts = [f"//! module {f}\n"]
    for _ in range(FNS_PER_FILE):
        parts.append(make_fn(fid))
        fid += 1
    # 20 planted 3-member clone families spread over the first 60 files.
    if f < 60:
        gid, copy = f // 3, f % 3
        parts.append(
            CLONE_TEMPLATE.format(
                gid=gid,
                copy=copy,
                xs=NAMES[copy],
                acc=ACCS[copy],
                lit=60 + copy * 10,
                lit2=300 + copy * 50,
            )
        )
    text = "\n".join(parts)
    loc += text.count("\n")
    with open(os.path.join(ROOT, f"mod_{f:04}.rs"), "w") as fh:
        fh.write(text)

print(f"generated {FILES} files, {loc} lines, {fid} sampled functions + 60 planted clones")
