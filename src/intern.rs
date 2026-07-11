//! Interning WP, id-conversion pass (session `stark-mixed-front`,
//! `interning-id-conversion.plan.md`): the mechanism this module implements is id
//! EQUALITY at every comparison site — not a string-handle shim. Two DIFFERENT
//! interner scopes, never conflated (unchanged from the original interning design):
//!
//! - [`Kind`]/[`Field`]: process-lifetime, global, never evicted. Vocabulary is a few
//!   hundred entries total (the 22 canonical IR/Native kinds + per-grammar node-kind
//!   names + kept-operator token text), saturates almost immediately — safe even in a
//!   long-lived server process (`reprise-mcp`). Represented as a **`u16` id** — no
//!   pointer, no string, in the node. Comparison is a bare integer equality; there is
//!   no `Deref`/`AsRef<str>` impl (deliberately — see below), so a comparison site
//!   CANNOT silently fall back to string comparison. Resolving an id back to its
//!   string (`as_str`) is an explicit, visible call used only for hashing
//!   (`fingerprint.rs`, which must keep hashing resolved bytes) and output/formatting
//!   (`Display`/`Debug`/`Serialize`) — never on a comparison path.
//! - [`LabelInterner`]/[`LSym`]: per-scan (one fresh instance per `corpus_units` call).
//!   Backs `Label::External`/`Label::LitKept` identifier/literal text, which is
//!   repo-specific and open-ended — a process-global table here would grow unboundedly
//!   across a long-lived server's many requests, so this one is explicitly NOT global
//!   and NOT leaked; it frees when its last `Arc` (held by the scan's units) drops.
//!   [`LSym`] is a **`u32` id**; equality within a scan is id equality. Unlike the
//!   prior (string-handle) design, an `LSym` is NOT self-contained — resolving it back
//!   to text needs the owning scan's `LabelInterner` (`LabelInterner::resolve`). This
//!   is the necessary consequence of dropping the pointer: callers that need the
//!   resolved text (fingerprint hashing, the D19/pack wire serialization) now take an
//!   explicit `&LabelInterner` parameter — see `tree::NormNode::to_wire`.
//!
//! **Why this design, and what the FIRST interning WP got wrong.** The prior WP
//! measured a hand-rolled single `RwLock<HashMap<..>>` id table costing ~5-6x
//! wall-clock at real corpus scale, concluded ids were unworkable, and shipped shared
//! string handles instead (`Arc<str>` / leaked `&'static str`) — never falsifying the
//! ACTUAL planned mechanism. The regression was self-inflicted: it resolved
//! id→string on every comparison (a lock + lookup per node-pair in `anti_unify`'s
//! O(n·m) DP). This design never does that: a comparison is `Kind(3) == Kind(3)`,
//! two `u16`s, no lock, no lookup, ever. The ONLY lock touched during matching is the
//! (per-scan, per-string, one-time) intern step at tree CONSTRUCTION — reads of an
//! already-built tree's ids touch nothing but plain integers.

use std::sync::Arc;

// ---------------------------------------------------------------------------
// kind/field: process-lifetime, id-based interner
// ---------------------------------------------------------------------------

/// Bidirectional string<->u16-id table, process-lifetime (entries are never
/// evicted — see the module doc).
///
/// **Locking, and a regression this shape fixes.** An earlier version of this
/// table gated every genuinely-NEW string (the "slow path") behind one single
/// coarse `Mutex` shared by the whole table, on the (wrong, for [`LabelInterner`])
/// assumption that new-string sightings are rare. That holds for `Kind`/`Field`
/// (a few hundred entries, saturates almost immediately) but NOT for
/// [`LabelInterner`] (open-ended, repo-specific identifiers — a large real corpus
/// mints new distinct labels continuously through the whole scan). Serializing
/// EVERY new-label insertion across ALL of rayon's parallel extraction workers
/// through one global lock measured a ~7x wall-clock regression on a real kernel
/// subtree (`fs/`, interning-id-conversion WP G3) — a real implementation bug in
/// THIS table, not evidence against the id mechanism (per-node reads/comparisons
/// never touch this table at all; only construction-time interning does). Fixed
/// by dropping the global mutex entirely: `forward`'s own sharded lock (a
/// [`dashmap::DashMap`], same as the fast path) serializes only same-shard
/// insertions.
///
/// **`reverse` resolution (interning-resolve-fix WP, session `stark-mixed-front`):
/// lock-free id-indexed [`boxcar::Vec`], not a `DashMap`.** The id-conversion WP
/// (above) eliminated the reverse lookup on the matching COMPARISON path, but
/// `resolve`/`as_str` is also called on the fingerprint-HASHING path
/// (`fingerprint.rs`, walked over ~every node during near-tier retrieval — see
/// that module's doc) — a `DashMap::get` there reintroduced the exact class of
/// per-node lock+lookup regression the id conversion was supposed to have fully
/// removed (measured: near-phase wall-clock +224% on a real kernel corpus).
/// `boxcar::Vec` is lock-free append + O(1) indexed load, no hash, no lock, so
/// `resolve` on the hot hashing path is now as cheap as it was under the
/// pre-conversion `Arc<str>`-handle design's pointer deref.
///
/// **The id assignment is now derived FROM the `reverse` push, not a separate
/// `AtomicU32` ticket.** The prior (pre-resolve-fix) design reserved an id via
/// `next_id.fetch_add` BEFORE racing to insert into `forward`; a thread that lost
/// the `forward` race simply discarded its reserved id, leaving a permanent gap
/// in the id space — harmless when `reverse` was a sparse `DashMap` (a gap is
/// just an unused key, and no caller ever resolves an id that wasn't actually
/// returned by a winning `intern()` call). It is NOT harmless against a *dense*,
/// append-only `boxcar::Vec`: `push` always assigns the next sequential slot, so
/// a wasted ticket desyncs "highest id assigned" from "number of pushes",
/// permanently misaligning `resolve(id)` with the wrong slot for every id minted
/// after the gap. Fixed by minting the id from `reverse.push`'s own return value
/// (only the winner of the `forward.entry()` race ever pushes), which makes
/// `push index == id` a structural invariant, not merely a documented one —
/// still asserted below (`u16::try_from`'s range check plus the roundtrip unit
/// tests) for anyone editing this later.
struct IdInterner {
    forward: dashmap::DashMap<&'static str, u16>,
    reverse: boxcar::Vec<&'static str>,
}

impl IdInterner {
    fn new() -> Self {
        IdInterner {
            forward: dashmap::DashMap::new(),
            reverse: boxcar::Vec::new(),
        }
    }

    /// Intern `s`, returning its id (the SAME id for every call with equal
    /// content). Pre-registration (the canonical vocabularies) calls this in a
    /// fixed order, single-threaded (guarded by the caller's `OnceLock::
    /// get_or_init`), so the assigned ids match the hand-written `pub const` ids
    /// in `ir::kind`/`ir::field` — see those modules' tests — regardless of this
    /// table's own internal concurrency (pre-registration never races itself).
    fn intern(&self, s: &str) -> u16 {
        if let Some(id) = self.forward.get(s) {
            return *id;
        }
        let leaked: &'static str = Box::leak(s.to_string().into_boxed_str());
        match self.forward.entry(leaked) {
            dashmap::mapref::entry::Entry::Occupied(e) => *e.get(),
            dashmap::mapref::entry::Entry::Vacant(e) => {
                // The id IS the push index (see the type doc): only the winner
                // of this `forward` race ever pushes, so `reverse`'s slot count
                // and the highest minted id can never desync.
                let pushed = self.reverse.push(leaked);
                let id = u16::try_from(pushed)
                    .expect("kind/field vocabulary exceeded u16::MAX distinct names");
                e.insert(id);
                id
            }
        }
    }

    /// Resolve an id back to its string. `id` must have come from this table's
    /// own [`Self::intern`] (every [`Kind`]/[`Field`] in the process does, by
    /// construction) — out-of-range is an internal-invariant violation.
    fn resolve(&self, id: u16) -> &'static str {
        self.reverse
            .get(id as usize)
            .expect("Kind/Field id must have been produced by this table's own intern()")
    }
}

static KIND_INTERNER: std::sync::OnceLock<IdInterner> = std::sync::OnceLock::new();
static FIELD_INTERNER: std::sync::OnceLock<IdInterner> = std::sync::OnceLock::new();

/// The shared `kind` table, with the canonical IR vocabulary (`crate::ir::kind::ALL`)
/// pre-registered — in that exact order — on first touch, so the ids line up with
/// `ir::kind`'s hand-written `pub const` ids (`Kind::intern(ir::kind::LOOP) ==
/// ir::kind::LOOP` is asserted by a test in that module).
fn kind_interner() -> &'static IdInterner {
    KIND_INTERNER.get_or_init(|| {
        let it = IdInterner::new();
        for k in crate::ir::kind::ALL {
            it.intern(k);
        }
        it
    })
}

/// The shared `field` table. No canonical vocabulary is pre-registered (field
/// names are overwhelmingly per-grammar, not a closed IR set — see
/// `ir::field`'s doc comment for the handful of IR-synthesized exceptions,
/// which ARE pre-registered here in fixed order for the same reason kinds are).
fn field_interner() -> &'static IdInterner {
    FIELD_INTERNER.get_or_init(|| {
        let it = IdInterner::new();
        for f in crate::ir::field::ALL {
            it.intern(f);
        }
        it
    })
}

/// An interned `NormNode::kind` (spec §5.2.3): a process-lifetime `u16` id — no
/// pointer, no string, in the node (16 B → 2 B; see `tree.rs`'s size gate). NO
/// `Deref`/`AsRef<str>` impl: this is deliberate (the interning-id-conversion WP's
/// hard invariant) so a `node.kind == "..."` comparison site cannot compile at all —
/// it must convert to an id comparison against a pre-registered const
/// (`crate::ir::kind::LOOP`-style, or a per-module `LazyLock<Kind>` for a
/// non-canonical grammar/literal name). Resolving to the string (`as_str`) is an
/// explicit, visible method call, used only off the comparison path (hashing,
/// `Display`/`Debug`/`Serialize`, and wire conversion).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Kind(u16);

impl Kind {
    pub fn intern(s: &str) -> Self {
        Kind(kind_interner().intern(s))
    }

    /// Resolve to the underlying string. NOT for comparisons (see the type doc) —
    /// use `==` against a pre-registered const/static instead.
    pub fn as_str(&self) -> &'static str {
        kind_interner().resolve(self.0)
    }

    /// Build a `Kind` from a fixed, pre-registered id. Used only by `ir::kind`'s
    /// canonical consts, whose numeric values match `kind_interner()`'s
    /// pre-registration order — never call this with an arbitrary id.
    pub(crate) const fn from_registered_index(i: u16) -> Self {
        Kind(i)
    }
}

impl From<&str> for Kind {
    fn from(s: &str) -> Self {
        Kind::intern(s)
    }
}
impl From<Box<str>> for Kind {
    fn from(s: Box<str>) -> Self {
        Kind::intern(&s)
    }
}
impl std::fmt::Debug for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self.as_str(), f)
    }
}
impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
impl serde::Serialize for Kind {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}
impl<'de> serde::Deserialize<'de> for Kind {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Kind::intern(&s))
    }
}

/// An interned `NormNode::field` name (spec §5.2). Same shape and rationale as
/// [`Kind`] (own `u16` id space, own table — a distinct Rust type either way, so
/// sharing a table would be harmless too, but keeping them separate avoids any
/// coupling between the two vocabularies' sizes/ordering).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Field(u16);

impl Field {
    pub fn intern(s: &str) -> Self {
        Field(field_interner().intern(s))
    }

    /// Resolve to the underlying string. NOT for comparisons — see [`Kind::as_str`].
    pub fn as_str(&self) -> &'static str {
        field_interner().resolve(self.0)
    }

    /// See [`Kind::from_registered_index`].
    pub(crate) const fn from_registered_index(i: u16) -> Self {
        Field(i)
    }
}

impl From<&str> for Field {
    fn from(s: &str) -> Self {
        Field::intern(s)
    }
}
impl From<Box<str>> for Field {
    fn from(s: Box<str>) -> Self {
        Field::intern(&s)
    }
}
impl std::fmt::Debug for Field {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self.as_str(), f)
    }
}
impl std::fmt::Display for Field {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
impl serde::Serialize for Field {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}
impl<'de> serde::Deserialize<'de> for Field {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Field::intern(&s))
    }
}

// ---------------------------------------------------------------------------
// label (External/LitKept text): per-scan, id-based interner
// ---------------------------------------------------------------------------

/// Per-scan `Sync` interner for `Label::External`/`Label::LitKept` text — one fresh
/// instance per `corpus_units` call (never global — see the module doc). Produces
/// [`LSym`] — a bare `u32` id, NOT self-contained (unlike the prior `Arc<str>`
/// design): resolving it back to text needs `Self::resolve` on the SAME
/// `LabelInterner` instance that minted it.
///
/// **Locking.** No global lock anywhere on the intern path (see [`IdInterner`]'s
/// doc comment for the regression a single coarse mutex caused here specifically —
/// this table's vocabulary is open-ended, unlike `Kind`/`Field`'s, so a global
/// "new string" lock stays hot for the WHOLE scan, not just startup). `forward`'s
/// own per-shard lock (the `DashMap`) is the only synchronization on a genuinely
/// new label.
///
/// **`reverse` resolution (interning-resolve-fix WP): lock-free id-indexed
/// [`boxcar::Vec`], and `resolve` borrows instead of cloning.** Same rationale as
/// [`IdInterner`]'s doc comment: `resolve` is on the fingerprint-hashing hot path
/// (`fingerprint.rs::push_label`, called for every `Label::External`/`LitKept`
/// node during near-tier retrieval's `subtree_inventory`/`merkle_mode` walk), and
/// the prior `DashMap<u32, Arc<str>>` cost a hashed lookup PLUS an `Arc::clone`
/// there, on every occurrence, not just every distinct string — the single
/// biggest contributor to the id-conversion WP's +224% near-phase regression.
/// `resolve` now returns `&str` (no clone at all) from an O(1) indexed load (no
/// lock, no hash). The interner still owns the strings (`Box<str>`, not leaked —
/// this table is per-scan, freed when the scan's `Arc<LabelInterner>` drops,
/// same lifetime semantics as before); `resolve`'s borrow is tied to `&self`.
/// The id is minted from `reverse.push`'s own return value, for the identical
/// reason [`IdInterner`]'s doc comment explains (a separate ticket counter that
/// a race loser can "waste" desyncs against a *dense* vector — this table faces
/// that race routinely, being the exact hot, wide-open, many-new-strings table
/// the module doc above describes).
#[derive(Debug)]
pub struct LabelInterner {
    forward: dashmap::DashMap<Box<str>, u32>,
    reverse: boxcar::Vec<Box<str>>,
}

impl LabelInterner {
    pub fn new() -> Arc<Self> {
        Arc::new(LabelInterner {
            forward: dashmap::DashMap::new(),
            reverse: boxcar::Vec::new(),
        })
    }

    /// Intern `text`, returning its id within this scan. A plain `&self` method
    /// (unlike the prior `Arc<str>`-handle design's `self: &Arc<Self>`, which
    /// needed the `Arc` to hand a self-contained clone to the returned handle) —
    /// `LSym` is now a bare id, needing no owning-`Arc` back-reference, so any
    /// `&LabelInterner` (through the scan's held `Arc` or a plain reference, as
    /// `au.rs`'s per-comparison `Ctx` holds) can intern/resolve.
    pub fn intern(&self, text: &str) -> LSym {
        if let Some(id) = self.forward.get(text) {
            return LSym(*id);
        }
        match self.forward.entry(text.into()) {
            dashmap::mapref::entry::Entry::Occupied(e) => LSym(*e.get()),
            dashmap::mapref::entry::Entry::Vacant(e) => {
                // The id IS the push index (see the type doc): only the winner
                // of this `forward` race ever pushes, so `reverse`'s slot count
                // and the highest minted id can never desync.
                let id = u32::try_from(self.reverse.push(text.into()))
                    .expect("label vocabulary exceeded u32::MAX distinct strings in one scan");
                e.insert(id);
                LSym(id)
            }
        }
    }

    /// Resolve `sym` back to its text, borrowed from this interner (no clone —
    /// see the type doc). `sym` must have come from THIS interner (an `LSym`
    /// minted by a different scan's interner has no meaning here — same
    /// non-portability the prior `Arc<str>` design didn't have, traded for
    /// dropping the pointer; see the module doc).
    pub fn resolve(&self, sym: LSym) -> &str {
        self.reverse
            .get(sym.0 as usize)
            .expect("LSym must have been produced by this interner's own intern()")
    }

    /// Number of distinct strings interned so far (test/debug — also doubles as the
    /// "starts empty" assertion point for the no-cross-scan-leak invariant).
    pub fn len(&self) -> usize {
        self.reverse.count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// An interned `Label::External`/`Label::LitKept` payload: a bare `u32` id (4 B —
/// down from a 16 B `Arc<str>` handle). Equality/ordering/hash are the id's — a
/// per-node comparison in the matching hot loops (`anti_unify`'s DP, near-tier
/// candidate generation) is a plain `u32` compare, not a string compare. NOT
/// self-contained: resolving it needs the owning scan's [`LabelInterner`] (see the
/// type's module doc). `Debug` intentionally shows only the numeric id (no
/// interner reachable from `fmt`) — resolve explicitly via `LabelInterner::resolve`
/// when the text is needed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LSym(u32);

impl std::fmt::Debug for LSym {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LSym({})", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_intern_resolve_roundtrip() {
        let k = Kind::intern("a_totally_novel_kind_name_xyz123");
        assert_eq!(k.as_str(), "a_totally_novel_kind_name_xyz123");
        // interning the same text twice yields the same id
        let k2 = Kind::intern("a_totally_novel_kind_name_xyz123");
        assert_eq!(k, k2);
    }

    #[test]
    fn field_intern_resolve_roundtrip() {
        let f = Field::intern("a_totally_novel_field_name_xyz123");
        assert_eq!(f.as_str(), "a_totally_novel_field_name_xyz123");
        let f2 = Field::intern("a_totally_novel_field_name_xyz123");
        assert_eq!(f, f2);
    }

    #[test]
    fn canonical_ir_kinds_are_all_pre_registered_and_resolve() {
        // Touch the table via an unrelated intern first, to prove pre-registration
        // happens regardless of what else has already been interned.
        let _ = Kind::intern("some_unrelated_grammar_kind_touched_first");
        for &name in crate::ir::kind::ALL {
            let k = Kind::intern(name);
            assert_eq!(k.as_str(), name);
        }
    }

    #[test]
    fn label_interner_starts_empty_per_scan() {
        let scan_a = LabelInterner::new();
        scan_a.intern("alice");
        scan_a.intern("bob");
        assert_eq!(scan_a.len(), 2);

        // A second, independent "scan" (the MCP-server-longevity property: two
        // sequential corpus_units calls in one process must not leak label ids).
        let scan_b = LabelInterner::new();
        assert!(scan_b.is_empty(), "fresh LabelInterner must start empty");
        scan_b.intern("carol");
        assert_eq!(scan_b.len(), 1);
        // scan_a is untouched by scan_b's activity
        assert_eq!(scan_a.len(), 2);
    }

    #[test]
    fn lsym_equality_is_by_id_within_one_interner() {
        let scan_a = LabelInterner::new();
        let sym_a = scan_a.intern("same_text");
        let sym_a_again = scan_a.intern("same_text");
        assert_eq!(
            sym_a, sym_a_again,
            "interning equal text twice yields the same id"
        );
        let sym_c = scan_a.intern("different_text");
        assert_ne!(sym_a, sym_c);
    }

    #[test]
    fn lsym_resolves_only_through_its_owning_interner() {
        let scan_a = LabelInterner::new();
        let a1 = scan_a.intern("x");
        assert_eq!(scan_a.resolve(a1), "x");
    }

    /// interning-resolve-fix WP: `resolve(intern(s)) == s` for a batch of
    /// distinct strings, INCLUDING one interned after many others already
    /// occupy the table (a "late intern" — `Kind`/`Field` never freeze, so this
    /// exercises the boxcar `reverse` vec having already grown past its first
    /// bucket boundary, not just the trivial first-few-entries case).
    #[test]
    fn kind_batch_roundtrip_including_late_intern() {
        let names: Vec<String> = (0..200)
            .map(|i| format!("kind_batch_roundtrip_probe_{i}"))
            .collect();
        let ids: Vec<Kind> = names.iter().map(|n| Kind::intern(n)).collect();
        for (name, id) in names.iter().zip(&ids) {
            assert_eq!(id.as_str(), name.as_str());
        }
        // Late intern, well after the batch above, and re-check EVERY id from
        // the batch still resolves correctly (proves no slot got clobbered/
        // misaligned by the intervening growth).
        let late = Kind::intern("kind_batch_roundtrip_probe_LATE");
        assert_eq!(late.as_str(), "kind_batch_roundtrip_probe_LATE");
        for (name, id) in names.iter().zip(&ids) {
            assert_eq!(id.as_str(), name.as_str());
        }
    }

    /// Same invariant as [`kind_batch_roundtrip_including_late_intern`], for
    /// [`LabelInterner`] (the per-scan table, `boxcar::Vec<Box<str>>`-backed).
    #[test]
    fn label_batch_roundtrip_including_late_intern() {
        let scan = LabelInterner::new();
        let texts: Vec<String> = (0..200)
            .map(|i| format!("label_batch_roundtrip_probe_{i}"))
            .collect();
        let syms: Vec<LSym> = texts.iter().map(|t| scan.intern(t)).collect();
        for (text, sym) in texts.iter().zip(&syms) {
            assert_eq!(scan.resolve(*sym), text.as_str());
        }
        let late = scan.intern("label_batch_roundtrip_probe_LATE");
        assert_eq!(scan.resolve(late), "label_batch_roundtrip_probe_LATE");
        for (text, sym) in texts.iter().zip(&syms) {
            assert_eq!(scan.resolve(*sym), text.as_str());
        }
    }

    /// interning-resolve-fix WP, the correctness fix at the core of this WP:
    /// concurrent threads racing to intern many BRAND-NEW distinct strings
    /// (including the SAME new string from multiple threads at once, to force
    /// `forward`-entry races) must never desync `reverse`'s dense push-index
    /// from the assigned id. The prior (pre-fix) design reserved an id via a
    /// separate `AtomicU32` ticket BEFORE the `forward` race, so a race LOSER
    /// wasted its ticket — harmless against a sparse `DashMap` `reverse`, but a
    /// silent, permanent misalignment against a dense `boxcar::Vec` (every id
    /// minted after the first wasted ticket resolves to the WRONG slot). This
    /// test exercises exactly that race pattern under real concurrency and
    /// asserts every id still resolves to its own original text.
    #[test]
    fn label_interner_concurrent_new_string_races_preserve_resolve_correctness() {
        use std::sync::Barrier;
        let scan = LabelInterner::new();
        let n_threads = 16;
        let strings_per_thread = 200;
        let barrier = Arc::new(Barrier::new(n_threads));
        let handles: Vec<_> = (0..n_threads)
            .map(|_| {
                let scan = Arc::clone(&scan);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    let mut local = Vec::with_capacity(strings_per_thread);
                    for i in 0..strings_per_thread {
                        // Every thread interns the SAME sequence of strings —
                        // maximizes `forward`-entry race collisions (each
                        // string is "new" only to whichever thread wins it).
                        let text = format!("concurrent_probe_{i}");
                        let sym = scan.intern(&text);
                        local.push((text, sym));
                    }
                    local
                })
            })
            .collect();
        for h in handles {
            let local = h.join().expect("worker thread panicked");
            for (text, sym) in local {
                assert_eq!(
                    scan.resolve(sym),
                    text.as_str(),
                    "id {sym:?} must resolve back to the exact text that produced it, \
                     even under concurrent new-string races"
                );
            }
        }
        // All threads intern the same 200 strings, so exactly 200 distinct
        // ids should have been minted, not more (races must dedup) or fewer.
        assert_eq!(scan.len(), strings_per_thread);
    }

    #[test]
    fn independent_interners_can_assign_the_same_id_to_different_text() {
        // The whole point of dropping the pointer: an LSym is scan-relative. Two
        // independent scans each intern a FIRST, distinct string and both get id 0 —
        // comparing an LSym from scan_a against one from scan_b without resolving
        // is a bug at the CALL SITE (never done in this codebase: matching only
        // ever compares labels within one scan's trees), not something the type
        // itself can prevent (that's the tradeoff for a bare, pointerless id).
        let scan_a = LabelInterner::new();
        let scan_b = LabelInterner::new();
        let sym_a = scan_a.intern("alice_only");
        let sym_b = scan_b.intern("bob_only");
        assert_eq!(
            sym_a, sym_b,
            "both are id 0 in their own scan — same numeric value"
        );
    }
}
