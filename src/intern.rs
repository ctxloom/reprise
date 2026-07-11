//! Interning WP (session `stark-mixed-front`, `interning.plan.md`; design preamble at
//! `~/.ctxloom/sessions/stark-mixed-front/interning-preamble.md`): two DIFFERENT
//! interner scopes, never conflated.
//!
//! - [`Kind`]/[`Field`]: process-lifetime, global, never evicted. Vocabulary is a few
//!   hundred entries total (the 22 canonical IR/Native kinds + per-grammar node-kind
//!   names + kept-operator token text), saturates almost immediately — safe even in a
//!   long-lived server process (`reprise-mcp`).
//! - [`LabelInterner`]/[`LSym`]: per-scan (one fresh instance per `corpus_units` call).
//!   Backs `Label::External`/`Label::LitKept` identifier/literal text, which is
//!   repo-specific and open-ended — a process-global table here would grow unboundedly
//!   across a long-lived server's many requests, so this one is explicitly NOT global
//!   and NOT leaked; it frees when its last `Arc` (held by the scan's units) drops.
//!
//! **Design note, corrected mid-implementation by real-corpus measurement.** The
//! initial design interned to a numeric id (`u16`/`u32`) resolved via a table lookup
//! on every read. That is wrong for `Kind`/`Field`: they are read on every one of the
//! 281+89 comparison call sites across the whole matching pipeline (per-node, inside
//! hot loops like near-tier candidate generation and `anti_unify`'s O(n·m) DP) — even a
//! *sharded* concurrent map lookup on every read measured a ~2x wall-clock regression
//! at real corpus scale (a `DashMap`-backed id table; a single `RwLock`-backed one was
//! ~5x). The fix used here: intern to a **shared, already-resolved handle**
//! (`Arc<str>` for `LSym`; a leaked `&'static str` for `Kind`/`Field`, since those are
//! process-lifetime anyway) so a lookup happens ONCE, at construction, and every
//! subsequent read/comparison is a plain pointer deref — zero indirection, matching
//! the original `Box<str>` field's read cost exactly, while still deduplicating the
//! heap allocation across every repeated occurrence of the same string (the actual
//! memory win). Confirmed by re-running the same real-corpus benchmark after the fix.

use std::sync::Arc;

// ---------------------------------------------------------------------------
// kind/field: process-lifetime interner
// ---------------------------------------------------------------------------

/// Generic string interner producing a shared, leaked `&'static str` handle.
/// `Box::leak`s each new string exactly once on first sighting — sound *only*
/// because this backs process-lifetime tables that are never dropped (leaking here
/// is the correct memory model: the vocabulary is a few hundred entries total and
/// saturates almost immediately, so there is no unbounded growth even in a
/// long-lived server process). Backed by [`dashmap::DashMap`] so concurrent interning
/// during rayon's parallel extraction shards across independent locks rather than
/// contending one global lock — but note this only matters at INTERN time; every
/// subsequent READ of the returned `&'static str` is lock-free (a plain reference).
struct LeakingInterner {
    map: dashmap::DashMap<&'static str, &'static str>,
}

impl LeakingInterner {
    fn new() -> Self {
        LeakingInterner {
            map: dashmap::DashMap::new(),
        }
    }

    /// Intern `s`, returning the shared `&'static str` handle (the SAME reference for
    /// every call with equal content).
    fn intern(&self, s: &str) -> &'static str {
        if let Some(existing) = self.map.get(s) {
            return *existing;
        }
        let leaked: &'static str = Box::leak(s.to_string().into_boxed_str());
        // `entry(..).or_insert(..)` holds that shard's write lock for the whole
        // check-then-insert, so two racing threads interning the SAME new string
        // can't leave two different leaked copies live as the "canonical" one — the
        // loser's `leaked` allocation is simply never referenced again (a rare,
        // bounded one-time leak on a losing race, not a growth path).
        *self.map.entry(leaked).or_insert(leaked)
    }
}

static KIND_FIELD_INTERNER: std::sync::OnceLock<LeakingInterner> = std::sync::OnceLock::new();

/// The shared kind/field table, with the canonical IR vocabulary
/// (`crate::ir::kind::ALL`) pre-registered on first touch.
fn kind_field_interner() -> &'static LeakingInterner {
    KIND_FIELD_INTERNER.get_or_init(|| {
        let it = LeakingInterner::new();
        for k in crate::ir::kind::ALL {
            it.intern(k);
        }
        it
    })
}

/// An interned `NormNode::kind` (spec §5.2.3): a shared, process-lifetime `&'static
/// str` handle (see the module doc's design note for why this is `&'static str`, not
/// a numeric id). `Deref<Target = str>` + `AsRef<str>` mean every existing
/// `.kind.as_ref() == "..."` comparison site and every `"...".into()` construction
/// site (target-type-inferred) keep compiling and behaving byte-for-byte as before a
/// `Box<str>` → `Kind` field-type change — see the preamble's blast-radius
/// discussion. `resolve-at-hash-time` for fingerprinting falls out for free via the
/// same `Deref`. Memory win: the heap allocation is deduplicated (one leaked copy per
/// DISTINCT string, shared by every occurrence) instead of one per node; the field
/// itself stays a 16-byte fat pointer (same as `Box<str>`), so the win is the heap
/// allocation, not the inline size — the tradeoff that keeps reads/comparisons free.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Kind(&'static str);

impl Kind {
    pub fn intern(s: &str) -> Self {
        Kind(kind_field_interner().intern(s))
    }

    pub fn as_str(&self) -> &'static str {
        self.0
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
impl std::ops::Deref for Kind {
    type Target = str;
    fn deref(&self) -> &str {
        self.0
    }
}
impl AsRef<str> for Kind {
    fn as_ref(&self) -> &str {
        self.0
    }
}
impl std::fmt::Debug for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self.0, f)
    }
}
impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl serde::Serialize for Kind {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.0)
    }
}
impl<'de> serde::Deserialize<'de> for Kind {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Kind::intern(&s))
    }
}

/// An interned `NormNode::field` name (spec §5.2). Same shape and rationale as
/// [`Kind`], sharing the same underlying table (distinct Rust types, so no
/// cross-namespace ambiguity).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Field(&'static str);

impl Field {
    pub fn intern(s: &str) -> Self {
        Field(kind_field_interner().intern(s))
    }

    pub fn as_str(&self) -> &'static str {
        self.0
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
impl std::ops::Deref for Field {
    type Target = str;
    fn deref(&self) -> &str {
        self.0
    }
}
impl AsRef<str> for Field {
    fn as_ref(&self) -> &str {
        self.0
    }
}
impl std::fmt::Debug for Field {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self.0, f)
    }
}
impl std::fmt::Display for Field {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl serde::Serialize for Field {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.0)
    }
}
impl<'de> serde::Deserialize<'de> for Field {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Field::intern(&s))
    }
}

// ---------------------------------------------------------------------------
// label (External/LitKept text): per-scan interner
// ---------------------------------------------------------------------------

/// Per-scan `Sync` interner for `Label::External`/`Label::LitKept` text — one fresh
/// instance per `corpus_units` call (never global: this vocabulary is repo-specific
/// and open-ended, so a process-global table would grow unboundedly across a
/// long-lived server's many requests). Produces [`LSym`] handles backed by `Arc<str>`
/// (not a leaked `&'static str` — this table must actually free when the scan ends,
/// unlike the process-lifetime kind/field table). `DashMap`-backed for the same
/// intern-time-contention reason as [`LeakingInterner`]; reads are lock-free (`Arc`
/// deref).
#[derive(Debug)]
pub struct LabelInterner {
    map: dashmap::DashMap<Box<str>, Arc<str>>,
}

impl LabelInterner {
    pub fn new() -> Arc<Self> {
        Arc::new(LabelInterner {
            map: dashmap::DashMap::new(),
        })
    }

    /// Intern `text`, returning a shared, self-contained symbol (an `Arc<str>`
    /// handle — no external interner reference needed to read it).
    pub fn intern(self: &Arc<Self>, text: &str) -> LSym {
        if let Some(existing) = self.map.get(text) {
            return LSym(Arc::clone(&existing));
        }
        let fresh: Arc<str> = Arc::from(text);
        let entry = self.map.entry(text.into()).or_insert_with(|| fresh);
        LSym(Arc::clone(&entry))
    }

    /// Number of distinct strings interned so far (test/debug — also doubles as the
    /// "starts empty" assertion point for the no-cross-scan-leak invariant).
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// An interned `Label::External`/`Label::LitKept` payload: a shared `Arc<str>` handle
/// — self-contained (no separate interner reference needed to resolve it, unlike an
/// id-based design), a single pointer-sized field, lock-free to read. `PartialEq`/
/// `Eq`/`Hash` compare by **resolved string content** (via `Arc<str>`'s own content-
/// based impls), so two `LSym`s built from independent scans (e.g. two separately-
/// normalized trees in a test) that happen to hold equal text still compare equal —
/// exactly matching the `Box<str>` content-equality this replaces (many existing
/// tests `assert_eq!` two independently-built trees).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct LSym(Arc<str>);

impl LSym {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::ops::Deref for LSym {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}
impl AsRef<str> for LSym {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Debug for LSym {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&*self.0, f)
    }
}
impl std::fmt::Display for LSym {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl serde::Serialize for LSym {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_intern_resolve_roundtrip() {
        let k = Kind::intern("a_totally_novel_kind_name_xyz123");
        assert_eq!(k.as_str(), "a_totally_novel_kind_name_xyz123");
        assert_eq!(k.as_ref(), "a_totally_novel_kind_name_xyz123");
        // interning the same text twice yields the same (shared) handle
        let k2 = Kind::intern("a_totally_novel_kind_name_xyz123");
        assert_eq!(k, k2);
        assert!(
            std::ptr::eq(k.as_str(), k2.as_str()),
            "must share one leaked copy"
        );
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
    fn lsym_equality_is_by_content_not_by_owning_interner() {
        let scan_a = LabelInterner::new();
        let scan_b = LabelInterner::new();
        let sym_a = scan_a.intern("same_text");
        let sym_b = scan_b.intern("same_text");
        assert_eq!(
            sym_a, sym_b,
            "LSyms from independent interners with equal text must compare equal \
             (content equality, matching today's Box<str> semantics)"
        );
        let sym_c = scan_a.intern("different_text");
        assert_ne!(sym_a, sym_c);
    }

    #[test]
    fn lsym_survives_its_interner_dropping() {
        // Two sequential interners never share state; an LSym keeps its OWN Arc<str>
        // alive independent of the interner instance that minted it.
        let scan_a = LabelInterner::new();
        let a1 = scan_a.intern("x");
        drop(scan_a);
        assert_eq!(a1.as_str(), "x");
    }

    #[test]
    fn repeated_interning_shares_one_allocation() {
        // The whole memory point: interning the same text many times must not
        // allocate a new buffer each time.
        let scan = LabelInterner::new();
        let a = scan.intern("repeated");
        let b = scan.intern("repeated");
        assert!(
            std::sync::Arc::ptr_eq(&a.0, &b.0),
            "repeated interning of equal text must share one Arc<str> allocation"
        );
    }
}
