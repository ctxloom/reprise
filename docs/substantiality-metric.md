---
title: Landmark-density substantiality — a representation-invariant replacement for the token-size limiters
status: substantiality-via-landmark-peaks = **NO-GO ×2** (§0 density, §0.2 any peak metric — the distinction
  is semantic, token floors stay). BUT the investigation surfaced a real **pair-level** signal —
  **coverage-fraction (AUC 0.955)** as a match-shape filter, and confirmed the landmark layer is the near-tier
  recall engine (T1). Under pipeline validation (§0.2, A→B). Retained as the full landmark-investigation record.
sessions:
  - pulpy-wooly-list
related:
  - docs/PLAN.md line 117 (the `min_unit_tokens` size floor this replaces), §5.5.4 (landmark pairs — the mechanism this reuses), §7.4b (retrieval rivalry — the diagnostics model), §5.7 (shared IDF machinery)
  - docs/SIMILARITY-IR.md (the ~18% IR compaction that exposed the limiters' representation-dependence) + §15.2 (per-transform calibration attribution — kin to the diagnostics here)
  - src/matchtree.rs:143-202 (peak/landmark construction + `Stats`); src/config.rs (`min_unit_tokens`/`_ir`, `min_seq_tokens`, `histogram_min_votes`/`_ir`, `fold_min_repeats`, `shared_landmarks_min`); src/api.rs (IDF machinery)
  - taskloom: elder-wow (IDF substantiality for exact-region — subsumed by this), last-spill (w7 / Decision 4 — resolved by this)
  - Wang, *An Industrial-Strength Audio Search Algorithm*, ISMIR 2003 (Shazam) — the constellation/peak model reused
---

# Landmark-density substantiality

> **The novel claim.** reprise already computes, for retrieval, a rarity-weighted structural-richness
> signal per unit — the Shazam landmark constellation. That same quantity is a **better substantiality
> gate than raw token count**: it is representation-invariant (so it needs no per-normalizer
> recalibration) and it discriminates *distinctive* structure from *ubiquitous* boilerplate directly
> (so it separates a substantive small unit from a trivial one, which a size floor cannot). This
> document proposes gating substantiality on landmark density + IDF instead of token size, and
> **instruments the gate so its real usefulness is a reported number, not an assumption.**
>
> **⚠ FALSIFIED (2026-07-05, §0).** The claim above was measured and does **not** hold. The landmark
> constellation is a *size proxy* (r=0.994 with token count), not a rarity signal; structural rarity
> cannot see the substantive-vs-trivial-small distinction, which is *semantic*. The token floors stay.
> §1–§7 are retained as the record of the investigation and *why* the axis fails.

## 0. Measurement result — NO-GO (2026-07-05, `fond-mute` measure-before-implement)

A throwaway harness reproduced the exact `matchtree.rs:143-176` landmark machinery + the `api.rs` IDF
shape per unit, over realistic corpora (reprise self-scan, 960 units; `click`, 484 units), on a labeled
set (9 should-pass genuine small clones incl. w7 / `is_self_call`; 9 should-fail incl. the 25–29-token
plumbing runs + PY_ITER/PY_TREE):

| signal | AUC = P(pass > fail) | best-threshold acc | corr. with token_count |
|---|---|---|---|
| **token_count (the baseline it would replace)** | **0.654** | 0.72 | — |
| landmark_count | 0.654 | 0.67 | **r = 0.994** |
| idf_rare (sum) | 0.679 | 0.67 | r = 0.986 |
| distinctiveness (`max_df` / `min_idf`, inverted) | **< 0.5** | 0.56 | — |

- **Landmark count ≈ token count (r=0.994)** → identical AUC to the baseline, and it shifts by the *same*
  ~0.79 IR/historical compaction factor with equal variance → **not** representation-invariant; it would
  need the very `_ir` twin the proposal set out to delete. It *is* size.
- **Structural IDF saturates** — almost every ≥6-token subtree is unique (df=1 → idf≈max) → can't grade.
- **The premise is empirically false.** The plumbing runs have *as many* rare peaks as (and IDF ≥) the
  genuine small clones — they score **higher**, not lower. `is_self_call` (should-pass) has `max_df=139`:
  it contains the corpus's single most common subtree, because it is itself built from ubiquitous idioms,
  structurally indistinguishable from the plumbing. **No threshold passes w7 & `is_self_call` while
  failing the plumbing & PY_ITER/PY_TREE.**

**Root cause.** "Substantive small vs trivial small" is a **semantic/authorial** judgment (is this code
*doing something*, or is it boilerplate?), NOT a structural-rarity property the landmark/IDF layer can
observe. Structural richness ≈ size; a landmark-dense 30-token unit is just a 30-token unit.

**Conclusion.** Keep the per-normalizer token floors (`min_unit_tokens_ir`, `histogram_min_votes_ir`)
as-is; do **not** swap. w7 stays a Decision-4 hold-out (not recovered here). `elder-wow` is **not**
subsumed. If the small-region FP problem is worth pursuing, it needs a **different axis** than structural
landmark/IDF density — a candidate is *shared-fragment recurrence* (the corpus df of the shared **region**:
a fragment shared by many unrelated functions = boilerplate → suppress; shared by exactly the pair =
genuine), or a callee-semantics signal — **but that is a new design, not this one.**

## 0.2 Peak-selection sweep + the coverage-fraction signal (2026-07-05, `a34f513`)

§0 ruled out landmark *density* as a substantiality floor and named "shared-fragment recurrence" as the axis
to try instead. A follow-up swept the **peak-selection metric** (hypothesis: the algorithm is sound if the
*peaks* are right — reprise picks *every subtree below a global df floor*, unlike audio Shazam's sparse
local-maxima) and measured three landmark invariants on a labeled set (27 genuine cross-file clone pairs;
154 plumbing-FP pairs sharing a 24–30-token boilerplate region but AU-divergent).

- **T1 — the Shazam layer IS the near-tier recall engine (not dead weight).** `verified_only_landmark` =
  **181 / 192 verified pairs (94%)** on self/IR (212/221 historical) — the bag layer (Jaccard ≥ 0.70) alone
  recovers ~11; landmarks supply 16× more. Weakness: it *floods* — 30.5k candidates → 192 verified (~0.6%
  raw precision); **all** precision is downstream in histogram + AU, none in the peaks.
- **Substantiality via better peaks — second, conclusive NO-GO.** Sweeping rareness (1/2/5/10%, abs df≤2/3/5),
  min-subtree (4/6/8/12), and a sparse **top-K-IDF salience** selection: the salience variant *did* break the
  size correlation (r: 0.995 → **−0.018** — the hypothesis's mechanism confirmed), but per-unit landmark count
  separates genuine-clone units from plumbing units at **AUC ~0.45–0.50 (coin-flip) at every peak metric**,
  and sparse peaks *simultaneously destroy recall*. **Landmark count carries no substantiality signal under any
  peak definition;** the per-unit direction is exhausted. The substantiality problem is **semantic** (§0), not
  reachable by re-defining peaks. (Cognitive/cyclomatic complexity as a *per-unit* substantiality metric is
  likely dead for the same reason — `is_self_call` is a *simple* genuine clone.)
- **The reframe + the real signal — it's PAIR-level, not unit-level.** The small-region FP isn't a *trivial
  unit* (substantiality); it's *two unrelated units sharing a boilerplate region* (partial coincidence — a
  **match** property). The match-level signals are strong, **at the existing dense peaks** (sparser degrades
  them): **coverage-fraction** (shared / total landmarks, min of the pair) — **AUC 0.955** (whole-unit clones
  cover most of each unit; a coincidental region covers little — exactly the FP mechanism); shared-landmark-df
  (boilerplate landmarks recur across more units) — AUC 0.80.

**So the actionable output of this whole investigation is not a substantiality floor — it is a match-shape
filter (coverage-fraction),** usable as (i) a cheap **pre-AU candidate gate** to cut the 0.6% landmark flood
(perf/precision) and (ii) possibly an **exact-region FP filter** for the boilerplate coincidences.

**Open validation nuance (round A).** The 0.955 separated *whole-clones* from *region-coincidences*, but a
*genuine shared-fragment clone* (a real copy-pasted block in two otherwise-different functions) also has low
whole-unit coverage — so validation must confirm coverage suppresses *boilerplate* regions **without nuking
genuine fragments** (shared-landmark-df may separate those better). Round B: region-level **cognitive
complexity** of the shared run as an FP complement.

**Status of the landmark line:** substantiality — dead (2×). Retrieval — the layer earns its keep (T1).
Match-shape filtering (coverage-fraction) — the live, promising thread; under pipeline validation (A→B).

## 0.3 Round-A live-pipeline validation — coverage-fraction **GO (gate)**, shared-landmark-df **GO (Seam-C discount, qualified)** (2026-07-05, `stark-mixed-front`)

Round A of §0.2 (validate the two pair-level signals on reprise's REAL near-clone pipeline) run to
completion. (A prior partial run, agent `a2a9c26`, died mid-run and tentatively reported *weak* live
AUC 0.573–0.689 for coverage-fraction / 0.561 for shared-landmark-df, hypothesizing IR landmark sets
are edit-fragile so genuine near-clones themselves have low whole-unit coverage. **That result is
REFUTED below** — it was a broken/incomplete measurement, not a real property.)

**Method — a fidelity-proven reconstruction, not an offline probe.** A throwaway harness
(`examples/lm_validate.rs`) reconstructs the `src/matchtree.rs` landmark path **verbatim** (rep build,
rare-peak `df≤rare_cap` selection, FAN_OUT=3 landmark triples, `df_cap=50` candidate formation) over a
REAL corpus extracted by the shipping `unit::extract_file_units` (IR normalizer, default config), and
labels each landmark-candidate pair by the **real** verification chain — the verbatim
`size_gate → offset_histogram → anti_unify` with the shipping `max_divergence=0.18` / `max_holes=5` /
`factorable` acceptance. POSITIVE = a pair the pipeline accepts as a near-clone; NEGATIVE = a landmark
candidate verification rejects (the flood). **Fidelity was proven against the live pipeline** (inline
disabled so both see plain units): on `src/`, reconstruction and `reprise::scan` agree **exactly** —
`candidates_landmark 7996 = 7996`, `histogram_rejected 2212 = 2212`, `verified_pairs 55 = ACCEPT 45 +
WEAK 10`. The per-pair coverage/df numbers therefore come from the real code path, not a re-implementation.

Coverage-fraction = |shared landmarks| / min(|A|,|B|); shared-landmark-df = **mean corpus
document-frequency of the shared landmark hashes** (boilerplate landmarks recur widely → high df).
Class-3 (genuine shared fragment) is labeled **independently** of landmark-df — from the sequence
tier's exact ≥`min_seq_tokens` runs and the distinct-unit recurrence (`frag_df`) of the shared run — so
the 3-class test is non-circular (labels from exact token runs; score from landmark pair-hashes).

**(1) Live AUC (positive = pipeline-accepted near-clone, negative = rejected landmark flood).**
Stable across three independent corpora:

| eval corpus | units | landmark-cand pairs | ACCEPT / REJECT | **coverage-fraction AUC** | **shared-landmark-df AUC** |
|---|---|---|---|---|---|
| reprise `src/` (fidelity-proven) | 829 | 7 996 | 45 / 7 941 | **0.959** | 0.861 |
| whole repo (Rust+Py+Go+TS+Kt) | 1 181 | 11 688 | 91 / 11 582 | **0.967** | **0.919** |
| `benches/mutations` (synthetic t1–t4) | 41 | 51 | 20 / 29 | 0.953 | 0.874 |

Coverage-fraction lands at **0.95–0.97 live — matching the §0.2 offline 0.955, NOT the prior 0.57–0.69.**
(`benches/wild` yields only 3 candidate pairs — each fixture is an isolated 2-unit pair with no flood —
so it is degenerate for a *pair-level* AUC and is excluded; it remains the tier's recall/precision net,
a different question.) Accepted near-clones have coverage **median 0.61–0.64**, i.e. NOT low — the prior
"edit-fragile / low-coverage genuine clones" hypothesis is directly falsified on the live accept set.

**(2) Operating points (whole-repo corpus, neg=11 582 rejects, pos=91 accepts).**
- **coverage-fraction as a pre-AU candidate gate (retain coverage ≥ t):** t=0.05 → **recall 1.000, flood
  cut 0.44** (recall-neutral: 44% of the AU workload removed at zero recall cost); t=0.10 → recall 0.967,
  flood cut 0.71; t=0.15 → recall 0.956, flood cut 0.83. A cheap, recall-near-neutral flood cut at the
  existing dense peaks (pre-AU, so it saves the O(n·m) anti-unification on the coincidences).
- **shared-landmark-df filter (drop mean shared-df ≥ t):** t=25 → recall 0.956, flood cut 0.61; t=15 →
  recall 0.890, flood cut 0.83; t=10 → recall 0.780, flood cut 0.92. **Strictly dominates the prior run's
  reported "34% FP cut cost 23% recall"** (that point sits near df<10, which here cuts *92%* of flood, or
  a 34% cut costs only ~3% recall) — further evidence the prior live measurement was broken.

**(3) 3-class separation (the Seam-C question — whole-repo).** Both signals separate, but along
**different axes**, and shared-landmark-df is the one that grades *recurrence*:

| class (independent label) | n | coverage med | **shared-lm-df med** |
|---|---|---|---|
| class-1 whole-clone (accepted) | 91 | 0.64 | **6.6** |
| class-3 genuine fragment (≥30-tok region, `frag_df=2`) | 193 | 0.45 | **9.7** |
| — mild-recurrence region (`frag_df=3–5`) | 29 | 0.26 | 16.1 |
| class-2 boilerplate-FP (scattered idioms, no ≥30-tok region) | 11 375 | 0.06 | **29.8** |

shared-landmark-df is **monotone with fragment recurrence** (6.6 → 9.7 → 16.1 → 29.8) and separates
class-2 boilerplate from class-3 genuine fragments at **AUC 0.905** (coverage separates them at 0.955
but cannot grade recurrence: genuine and whole-clone both sit high). So a df-keyed boilerplate discount
**cuts the boilerplate coincidences while sparing genuine fragments (9.7) and whole clones (6.6).**

**Honest caveat (bounds the GO):** across `src`, the whole repo, and mutations, there is **zero**
widely-recurring ≥30-token boilerplate *block* (a `frag_df≥6` region among rejects = 0 in every run).
The boilerplate that actually floods the landmark layer is **short scattered idioms** (< the 30-token
seq floor, hence no exact region), which df catches cleanly. The hardest adversarial case for a
df-discount — a *long* boilerplate block recurring widely, where high df would look like boilerplate but
the block might be "genuine" duplication worth reporting — **does not occur in this corpus and so could
not be positively measured.** The GO for shared-landmark-df is therefore qualified: proven against the
FP population that exists here (short-idiom coincidences), unproven against long-recurring-block boilerplate.

**(4) Verdicts + offline-vs-live resolution.**
- **coverage-fraction → GO, as a pre-AU candidate gate.** Live AUC 0.95–0.97 (fidelity-proven); a
  recall-neutral 44% flood cut at t=0.05, 71% at t=0.10 for 3% recall. It measures match *shape*
  (whole vs region-coincidence), so it is a retrieval/perf gate, **not** the natural home for a
  *boilerplate* discount (it does not distinguish boilerplate from genuine by recurrence — both genuine
  fragments and whole clones score high; that is fine for gating, wrong for a substantiality discount).
- **shared-landmark-df → GO for the Seam-C boilerplate-idiom discount (qualified as above).** Live AUC
  0.92; monotone in recurrence; separates boilerplate (df≈30) from genuine fragments (df≈10) and whole
  clones (df≈7) at AUC 0.905 — it discounts the FP source without nuking class-3. It is the
  recurrence-graded signal Seam C wants; coverage is not.
- **offline-vs-live gap: there is none.** The reconstruction is byte-identical to the shipping pipeline
  (proven), and reproduces the offline 0.955. The prior run's 0.573–0.689 was an artifact of an
  incomplete/mislabeled measurement (it died mid-run, explicitly UNVERIFIED); its edit-fragility
  hypothesis is falsified by the live accept-set coverage medians (0.61–0.64). No representation gap, no
  live degradation.

**Round B (unchanged, still open):** region-level cognitive complexity of the shared run as an FP
complement; and — surfaced here — a corpus that actually contains long widely-recurring boilerplate
blocks, to close the qualified part of the shared-landmark-df GO.

## 0.4 Retrieval bake-off — does the landmark retriever earn its keep vs canonical schemes? **KEEP (recall-regime win, ablation-confirmed)** (2026-07-05, `stark-mixed-front`)

§0.2/T1 established the landmark layer is the near-tier recall engine, but its recall had only ever
been measured against candidates **landmark itself produced** (it "supplies 94% of verified pairs" —
circular). This bake-off answers the open question with data: **does landmark MISS real clones a
different retriever would catch, and could a canonical scheme match its recall at lower flood/cost?**

**Method — one substrate, four retrievers, an oracle independent of any of them.** A new harness
(`examples/bakeoff.rs`) generalizes `lm_validate.rs`: the rep/feature substrate, the landmark
reconstruction, and the trusted `size_gate → offset_histogram → anti_unify` verify chain are reused
**verbatim**, behind a swappable `candidates(reps) → pairs` slot. Four retrievers run on the **same IR
subtree bag** (slices documented in-file so a difference reflects the *scheme*, not the features):

1. **reprise-landmark** — incumbent: rare-peak constellation triples (matchtree.rs verbatim).
2. **minhash-lsh** — bag-Jaccard over the floor-6 `bag_set` (the D16 bag layer's features), MinHash + LSH banding.
3. **winnowing** — MOSS-style k-gram winnowing over the floor-3 ordered subtree stream.
4. **sourcerer-rare** — inverted rare-feature overlap = **landmark's exact rare peaks WITHOUT the triples**: the ablation isolating whether the constellation earns its keep.

Two retriever-independent oracles: **(A) verify-on-union** — union all four retrievers' candidates,
run the trusted verify chain, the pairs that ACCEPT = the positive set (uses verify as oracle, *not*
landmark); recall = fraction of that verified-union each retriever surfaced. **(B) synthetic** — the
`benches/mutations` seed↔mutant pairs, known clones by construction (distinct-fingerprint = near-tier).

**Fidelity re-proven vs `reprise::scan` (inline off, plain units).** Byte-exact on both corpora:
`src/` — `candidates_landmark 7996=7996, histogram_rejected 2212=2212, verified_pairs 55=55`;
whole repo — `12215=12215, 3569=3569, 107=107`. The comparative numbers come from the real code path.

**(1) Apples-to-apples, shipping operating points (`src/`, 829 plain units; whole-repo confirms).**

| retriever (operating point) | flood | recall-A | recall-B | precision | idx-entries | query-ms |
|---|---|---|---|---|---|---|
| **reprise-landmark** (shared≥2) | 7 996 | **1.000** | **1.000** | 0.0056 | 78 701 | 15.4 |
| minhash-lsh (b32×r4, thr≈0.42) | 132 | 0.533 | 0.667 | **0.182** | 63 104 | 2.8 |
| winnowing (k5,w4,shared≥2) | 1 257 | 0.644 | 0.857 | 0.023 | 10 863 | **1.7** |
| sourcerer-rare (shared≥2) | 9 076 | **1.000** | **1.000** | 0.0050 | 23 882 | 3.9 |

Winner per metric: **recall-A / recall-B** — landmark & sourcerer tie at 1.000 (the ablation, not the
challengers, ties). **flood among full-recall retrievers** — landmark (7 996 < sourcerer 9 076).
**raw precision** — minhash (0.182, but only at recall 0.53); among full-recall, landmark > sourcerer.
**cost** — winnowing cheapest (1.7 ms / 10.9 k entries); **landmark is the most expensive** (15.4 ms /
78.7 k entries, ~4–8× the challengers). A single operating point is misleading, so the recall-flood
**curves** decide.

**(2) Recall–flood curves (the crux; both corpora agree).** Sweeping each retriever's knob:

| regime | `src/` | whole repo |
|---|---|---|
| **cheapest route to recall-A = 1.000** | landmark @ flood **6 257** (shared≥3) · sourcerer @ 9 076 · minhash @ 10 315 | landmark @ **9 988** · sourcerer @ 12 988 · minhash @ 13 908 |
| **low-flood crossover (recall < 0.98)** | minhash 0.978 @ 1 699 **beats** landmark 0.911 @ 2 253 | minhash 0.935 @ 2 278 **beats** landmark 0.935 @ 3 838 |

Two facts fall out, stable across corpora:

- **Landmark is Pareto-optimal at the full-recall frontier** — the *cheapest* retriever that reaches
  recall-A 1.000 (6 257 / 9 988 flood vs the next-best ~45%/30% more). This is the regime the near
  tier is *defined* by (the §8 100%-mutation-recall gate).
- **The constellation triples earn their keep — as a flood reducer.** At equal *full* recall the
  triples cut flood **31%** (`src`: landmark 6 257 vs sourcerer 9 076) / **23%** (repo: 9 988 vs
  12 988) over the bare rare-peak overlap: a genuine clone shares *dozens* of triples while a
  coincidence shares few, so landmark can raise the shared-count threshold (2→3) with **zero** recall
  loss where the SourcererCC ablation cannot (sourcerer drops to 0.933 at shared≥3). PLAN.md §5.5.4's
  "individually common landmarks become combinatorially rare in pairs" is confirmed against an oracle.
- **But landmark is NOT globally Pareto-optimal.** MinHash+LSH strictly dominates it in the
  sub-0.98-recall / low-flood regime. If the pipeline ever traded ~2–9% near-tier recall for a 3–5×
  flood cut, MinHash would be the better retriever — but that trade contradicts the recall gate.

**(3) Verdict — the three questions.**

- **Does landmark miss verified-union pairs the canonical schemes catch?** **No.** On both corpora it
  surfaces **100%** of the verify-union ACCEPT set (45/45 `src`, 92/92 repo); `missed = 0`, and **zero**
  ACCEPT pairs are caught *only* by a challenger. Its true recall is **not** lower than the circular
  94% — against an oracle it is *complete*. There are **no concrete missed pairs to list.**
- **Does any canonical scheme match landmark's recall at lower flood/cost?** **On flood, no** — landmark
  is the cheapest full-recall retriever, and its lower flood at equal recall also saves the *downstream
  anti-unify* cost (7 996→45 vs sourcerer 9 076→45), which dominates the near tier. **On index/query
  cost, yes** — landmark carries the largest index (78.7 k) and slowest query (15.4 ms); sourcerer
  matches recall with a 3.3× smaller index and 4× faster query, at 13% more flood. So the constellation
  buys a modest flood cut at a real index/time cost — a genuine, quantified trade, not free.
- **Does landmark dominate? Say it plainly.** **Yes, at the high-recall operating point the near tier
  requires** — it is the Pareto-optimal full-recall retriever, and the ablation proves the *triples*
  (not merely the rare peaks) deliver that low-flood full recall. **KEEP the landmark code.** Caveats,
  honestly: (i) the **rare-peak selection carries most of the recall** — the triples' marginal value is
  the 23–31% flood cut, not recall; (ii) landmark is the costliest retriever by index/time; (iii) it is
  not globally dominant — a small recall sacrifice flips the winner to MinHash+LSH. Grounds to delete
  the novel code would require the pipeline to abandon its full-recall stance, which it has not.

**Deferred (not measured here):** inline-on flood (variants entering retrieval — plain units only here);
cross-language *behavioral* matching (retrievers partition per-lang; oracle B aggregates per-lang recall,
it is not a cross-lang comparability test); the second-wave retrievers (SimHash, vector-ANN,
hole-context-hash reconstruction); MinHash uses a lightweight multiply-add permutation family adequate
for LSH bucketing — a stronger family could shift its low-flood curve slightly; index-build vs query
time reported combined (as `query-ms`), with `idx-entries` as the peak-footprint proxy.

Bench file: `examples/bakeoff.rs` (run `cargo run --release --example bakeoff -- [root…]`, default `src`).

## 0.5 Landmark-definition sweep — FLOOD reduction at ZERO recall loss **GO: structural verify (H-tree-verify); the constellation encoding is a wash** (2026-07-05, `stark-mixed-front`)

§0.4 proved the incumbent landmark is **complete-recall** (100% vs the verify-union oracle) and
**Pareto-optimal on flood** at the full-recall frontier. So the only remaining lever is **flood
reduction at recall = 1.0** — cut coincidental candidates without dropping a single verified clone.
This sweep trials seven alternate landmark **definitions** as new `candidates()` entrants in
`examples/bakeoff.rs`, each on the **same** substrate (nodes3 / df6 / df3) and the **same**
owner→count→`min_shared` clique; only the definition (peak selection + relation encoding) changes,
so any delta is attributable to it. **Measurement only — `src/` production code is untouched.**
The oracle is held **fixed** (verify-union ACCEPT via the original `offset_histogram → anti_unify`),
so recall is retriever-independent. Self-check: the reconstructed `fan=3, ms=2` reproduces the
incumbent flood **byte-for-byte** (8 013 on `src`), confirming the harness measures the real
definition.

Corpus: `src/` (834 plain units, IR normalizer, ACCEPT = 45, synthetic near = 21); **confirmed on
`src crates` (881 units, ACCEPT = 48) — same ranking, same verdicts.** recall-A = verify-union
ACCEPT, recall-B = synthetic near. Incumbent = `fan=3, ms=2`, flood **8 013**, recall 1.000/1.000.

**The knobs that hold BOTH recalls at 1.000, ranked by flood (`src`):**

| definition | flood | recall-A | recall-B | precision | query-ms | vs incumbent |
|---|---|---|---|---|---|---|
| **linear fan=4 ms=4 + tree-hist** | **3 444** | 1.000 | 1.000 | 0.0131 | ~21 | **−57%** |
| **linear fan=2 ms=3 + tree-hist** | 3 501 | 1.000 | 1.000 | 0.0129 | ~15 | −56% |
| **linear fan=3 ms=3 + tree-hist** | 3 749 | 1.000 | 1.000 | 0.0120 | ~21 | −53% |
| **linear fan=3 ms=2 + tree-hist** (pure filter on the shipping op-point) | 4 195 | 1.000 | 1.000 | 0.0107 | ~17 | **−48%** |
| fan=4 ms=4 (no filter) | 5 125 | 1.000 | 1.000 | 0.0088 | ~12 | −36% |
| tree fan=2 ms=3 (structural relcode) | 5 579 | 1.000 | 1.000 | 0.0081 | ~7 | −30% |
| fan=2 ms=3 | 5 691 | 1.000 | 1.000 | 0.0079 | ~6 | −29% |
| fan=3 ms=3 (§0.4's free win) | 6 277 | 1.000 | 1.000 | 0.0072 | ~10 | −22% |
| tree fan=3 ms=2 (structural relcode) | 7 744 | 1.000 | 1.000 | 0.0058 | ~8 | −3% |
| **fan=3 ms=2 = incumbent** | 8 013 | 1.000 | 1.000 | 0.0056 | ~8 | 0 |

**The definitions that FAIL the zero-recall-loss bar (cannot be adopted):**

| definition | best flood | recall-A | recall-B | why it fails |
|---|---|---|---|---|
| **H-peak-cap** (top-K rarest peaks/unit) | 703 @ K=24 | 0.933 | 0.905 | a fixed K starves large units of landmarks; recall collapses (K≤8 → ≤0.29). Confirms the incumbent's **no peak cap**. |
| **H-floor-align** (rare test on leak-free floor-3 df) | 2 248 | 0.889 | 0.810 | fixing the `unwrap_or(0)` sub-floor auto-rare **costs recall** — the 3–5-token subtrees it demotes are genuinely discriminative for small/renamed near-clones. **The "leak" is load-bearing, not a bug.** |
| **H-salience** (peaks filtered by node-kind tier) | 2 445 @ tier≥2 | 0.867–0.911 | 0.714–0.810 | dropping low-salience peaks (field/index chains, bare operators) discards features near-clones share. Rarity, not node-type, is the right selector. |

**Findings.**

- **H-tree-verify is the win.** A structural **depth-delta histogram** — for each shared floor-3
  subtree, vote on `depth_a − depth_b` instead of the token-offset delta — used as a **retriever-side
  pre-filter** on the incumbent's own candidate stream cuts flood **8 013 → 4 195 (−48%)** at the
  shipping operating point (`fan=3, ms=2`), holding recall 1.000/1.000 on **both** oracles and **both**
  corpora. It removes only candidate pairs whose shared subtrees sit at *inconsistent relative depths*
  — coincidental landmark collisions — and empirically drops **zero** ACCEPT pairs. Stacked with the
  free threshold/fan-out knobs it reaches **3 444 (−57%)**.
- **H-tree-constellation is a WASH.** Replacing the linear `Δoffset/8` in the landmark tuple with a
  structural relcode (containment bit + depth-delta) moves flood by only ≈1–3% at identical recall
  (`tree fan=3 ms=3` 6 265 vs `fan=3 ms=3` 6 277; `tree fan=3 ms=2` 7 744 vs 8 013). The hypothesis
  "structural relations are less coincidental" holds for the **verify histogram**, **not** for the
  constellation encoding — a quantized token gap is already as discriminative as a depth-delta there.
- **H-threshold + H-fanout are free, mechanism-less wins** (already partly known from §0.4). `ms=3`
  and/or `FAN_OUT=2` cut flood 22–30% at recall 1.000 with no new code. `FAN_OUT=4` lets `ms` rise to 4
  without recall loss (more redundant landmarks), giving the best selection-only point, `fan=4 ms=4`
  (5 125, −36%).

**Recommendation.**

1. **Adopt the safe knobs now (no new mechanism):** raise `shared_landmarks_min` 2 → 3 (the §0.4 free
   win, re-confirmed) — −22% flood at recall 1.000. `FAN_OUT=2` is an additional free cut if desired.
2. **Prototype H-tree-verify (the real prize).** Add the depth-delta agreement as a cheap structural
   gate. It need not be a second pass: the depth is already available per subtree and can be voted in
   the **same merge-join** the offset-histogram already walks (compute the `depth_a−depth_b` bin
   alongside the `offset_a−offset_b` bin), making it near-free. Net expectation: ~48% fewer candidates
   reaching `anti_unify` (the near tier's dominant cost) at zero recall loss. **Re-validate on
   whole-repo + inline-on before shipping** — the ACCEPT oracle is small (45/48), and the filter is a
   distinct structural axis.
3. **H-tree-constellation is NOT worth a production change** — a wash on flood, added tuple complexity
   for no gain. The structural signal pays off in *verify*, not in the constellation.
4. **Do NOT touch peak selection.** Cap, floor-align, and salience each lose recall on both corpora.
   The incumbent rare-peak selection — **including the `unwrap_or(0)` sub-floor auto-rare** — is
   well-tuned; the "leak" is a recall-carrying feature.

**Deferred (not measured):** inline-on flood (variants entering retrieval — plain units only here);
cross-language behavioral matching (retrievers partition per-lang); a full-pipeline **cost** accounting
of the tree-hist filter (it adds ~10–15 ms retrieval-side but should net-save downstream `anti_unify`
calls — measure end-to-end before adopting); H-tree-constellation with a richer relation vocabulary
(LCA arm-lengths, sibling-in-block) — the containment+depth-delta form tried here is already a wash, so
a richer form is low-priority; larger ACCEPT oracles to tighten the recall=1.000 claim beyond 45/48
pairs.

Bench: `examples/bakeoff.rs`, section `LANDMARK-DEFINITION SWEEP` + `RECALL=1.0 FLOOD RANKING`.

## 1. Problem — the limiters are representation-dependent size proxies

reprise's precision floors all gate on a **raw count** as a proxy for "is there enough here to trust a
match":

| limiter | default | gates |
|---|---|---|
| `min_unit_tokens` | 40 | unit-tier eligibility (PLAN.md line 117: *"the single most important precision knob"*) |
| `min_seq_tokens` | 30 | sequence/region-tier maximal-repeat floor |
| `histogram_min_votes` | 5 | near-tier offset-histogram vote floor |
| `fold_min_repeats` | — | sibling-run fold threshold |

Two structural faults:

1. **Representation-dependent.** The similarity-IR lowers trees ~18% more compact
   (docs/SIMILARITY-IR.md). Every raw-count floor therefore *shifts*, and each one grew a
   per-normalizer twin scaled by the compaction ratio: `min_unit_tokens_ir = 40 × 0.815 = 33`,
   `histogram_min_votes_ir = 5 × 0.815 = 4`. The knobs **multiply with every representation** — a
   maintenance and correctness liability (a third frontend, a grammar bump, any normalization change
   re-opens the calibration).

2. **Crude.** Raw count cannot tell a *substantive* 25-token unit from a *trivial* 25-token one. The
   canonical case is **w7 (`last-spill` / Decision 4)**: its shared chunked-I/O loop
   (`read`/`write`/`flush`) is 25 IR tokens and genuinely distinctive, but so are ~134 self-scan
   plumbing runs (`HashMap.entry().or_default()`, `node.walk()` boilerplate) at 25–29 tokens. A size
   floor must admit *both* or *neither* — it forces a recall/precision either-or that has no good answer
   in the size domain.

## 2. The foundation — reprise already selects rarity peaks (how the Shazam layer works)

The novel gate is cheap because the signal already exists. In audio Shazam a *peak* is a local energy
maximum in the spectrogram; reprise's analog (`src/matchtree.rs:143-176`):

- **Peak = an occurrence of a *rare* subtree at its structural offset.** Each unit `rep` holds
  `rep.offsets`: `subtree-hash → [structural offsets]`. A subtree qualifies as a peak only if its
  **document frequency is low** — `df[h] ≤ rare_cap`, where `rare_cap = max(3, n_units/20)`
  (`matchtree.rs:152,157`). **Rarity is the salience axis** — the direct stand-in for spectral energy.
  Ubiquitous subtrees (high DF — the boilerplate) are dropped, exactly like low-energy bins.
  *(IDF is already baked into peak selection; `api.rs:102` — "the rarity shape the landmark layer uses …
  shares the IDF machinery"; PLAN.md §5.5.4.)*
- **Landmark = a fan-out pair of peaks.** Peaks are sorted by offset; each anchor `i` is paired with the
  next `FAN_OUT = 3` peaks `j` (`:161-163`), hashed as
  `xxh3_128(rare[i].hash, rare[j].hash, Δoffset/8)` (`:164-169`) — Shazam's `(f_anchor, f_target, Δt)`
  triple, offset bucketed by 8 for shift tolerance. *"Individually common landmarks become
  combinatorially rare in pairs"* (PLAN.md §5.5.4).
- **Retrieval + verify.** Two units are candidates if they share ≥ `shared_landmarks_min` (default 2,
  raised to 4 at 500k-LOC scale, D22) landmark hashes (`:199-200`), then the **offset-histogram diagonal
  check** (`:455`) confirms the constellation aligns.

The load-bearing observation: **`rep.landmarks.len()` (and the rare-peak count behind it) is a
rarity-weighted measure of how much *distinctive* structure a unit has** — with boilerplate already
excluded by construction.

## 3. The proposal — substantiality = f(landmark density, IDF), not token count

Gate substantiality on the distinctive-structure signal directly:

```
substantiality(unit) = f( distinct_landmark_count(unit),  idf_content(unit) )
unit is reportable  ⇔  substantiality(unit) ≥ K
```

replacing the `min_unit_tokens` / `min_seq_tokens` / `histogram_min_votes` size gates. Properties:

- **Representation-invariant → one calibration, no per-normalizer twins.** The count of *distinct rare
  subtrees* does not scale with raw token count the way total tokens do; a compact IR tree with the same
  distinctive structure yields ~the same landmark count. So `K` is calibrated **once** and applies to
  every normalizer — `min_unit_tokens_ir`, `histogram_min_votes_ir`, and any future twin evaporate.
  *(Compaction can merge a few subtrees; §5 caveat — but it is far more stable than raw size, which is
  the whole point.)*
- **Targets the FP source directly.** A ubiquitous-idiom unit has few rare peaks → low substantiality →
  filtered **regardless of length**; a distinctive unit passes **regardless of length**. This is the
  discrimination the size floor cannot make: **w7's I/O run passes (several rare peaks) while the +134
  plumbing runs fail (≈0 rare peaks)** — recovering the recall *and* the precision that Decision 4 casts
  as mutually exclusive.
- **Subsumes existing intent.** It is `elder-wow` (rare-token/IDF substantiality for exact-region)
  generalized to every tier, and it is what PLAN.md line 117 *wanted* the size floor to approximate.

## 4. Diagnostics — report how useful the gate is *actually* being (first-class requirement)

A precision knob must not be taken on faith. Instrument the substantiality gate so its marginal value is
a **reported number every scan**, modeled on the §7.4b retrieval rivalry (which already measures
`candidates_landmark` / `verified_only_landmark` / `landmark_index_size` in `matchtree.rs::Stats` and
mandates dropping the loser). Emit, per scan:

- **Marginal gate decisions vs the token floor.** Count units the substantiality gate decides
  *differently* than `min_unit_tokens` would:
  - `gated_in_by_score` — passed by substantiality, rejected by size (the substantive-small **recoveries**, e.g. w7);
  - `gated_out_by_score` — rejected by substantiality, passed by size (the trivial-large **rejections**, e.g. the plumbing tail).
  A gate that changes *nothing* is dead weight; a gate that flips many decisions is doing work — this
  makes that visible.
- **The TP/FP split of the marginal set.** For the flipped decisions that have labels (the §7.2 sample /
  wild gold), the true/false split → the gate's **marginal precision and recall contribution** over the
  raw-token baseline. This is the calibration evidence *and* the ongoing usefulness readout in one.
- **Separation of the score on TP vs FP.** The distribution of `substantiality(unit)` over labeled true
  vs false findings — does the score actually separate them (e.g. an AUC / a TP-vs-FP median gap)? A gate
  whose score doesn't separate the classes is a bad gate no matter where `K` sits.
- **A one-line telemetry summary** alongside the existing phase timings/stats:
  `substantiality: floor K=…, gated N units (+a recovered / −b rejected vs token-floor), score TP/FP separation = …`.

This is the SIMILARITY-IR §15.2 idea — *attribute each recall/precision delta to a specific mechanism* —
applied to a live filter: the metric is **self-justifying**, and a future regression (a representation
change that quietly makes it useless) shows up in the readout instead of hiding.

## 5. Caveats (design around these)

- **Corpus-relative rarity.** `rare_cap = max(3, n_units/20)` makes "rare" (hence substantiality)
  depend on the scan set — inherent to IDF and usually desirable, but the gate is **not an absolute
  per-unit constant**. Small scans degenerate: few units → `rare_cap` floors at 3 → most subtrees count
  as "rare" → the signal weakens exactly when the corpus is thin. Needs a corpus-size-aware fallback
  (blend with an absolute structural-diversity count, or a size-floor backstop below N units).
- **Computed at match time.** Peaks/landmarks are built in `matchtree.rs`, whereas `min_unit_tokens`
  gates earlier (candidate/group eligibility, `group.rs`/`api.rs`). Moving the gate onto landmark count
  moves it later — either precompute a per-unit rare-peak count at extraction, or re-order the pipeline.
- **Coupling to the retrieval layer.** The landmark layer is a §7.4b rivalry *winner* (landmark-pairs
  beat hole-hashes); `landmark_pairs` is a config toggle. Repurposing it as the substantiality gate ties
  the precision floor to that layer's health — if `landmark_pairs` is ever disabled, substantiality needs
  a defined fallback.

## 6. Plan (post-flip)

1. Precompute a per-unit rare-peak / distinct-landmark count (lift `matchtree.rs`'s `df` + rare selection
   to extraction, or expose it to the eligibility gates).
2. Define `substantiality = f(landmark_count, idf_content)`; **calibrate `K` once** against the
   recall/precision gates — representation-invariant, one number for all normalizers.
3. Replace the `min_unit_tokens` / `min_seq_tokens` / `histogram_min_votes` gates with the score; verify
   with the **same `K` on both normalizers**: recall parity (recovers w7) + precision (rejects the +134
   plumbing) — the proof the metric does what size floors can't.
4. Ship the §4 diagnostics with it (non-optional — the gate justifies itself).
5. Sweep the remaining limiter (`fold_min_repeats`) onto the same footing.
6. Retire the per-normalizer twins (`min_unit_tokens_ir`, `histogram_min_votes_ir`) and the raw-token
   floors; fold in `elder-wow`.

## 7. Decisions for sign-off

- **Score form** — pure distinct-landmark-count, pure IDF content, or a blend? Decide by which best
  separates TP/FP on the §7.2 sample (the §4 separation metric is the arbiter).
- **Absolute vs corpus-relative** — how to handle small-scan degeneracy (`rare_cap` floor); a hybrid
  (score OR a low absolute size backstop) may be safest.
- **Gate locus** — extraction-time precompute vs match-time; trades a little extraction cost for a
  cleaner eligibility path.
