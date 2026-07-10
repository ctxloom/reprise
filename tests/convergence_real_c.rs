//! Real-corpus backing for the C frontend's promotion invariants (WP-K1b).
//!
//! CLAUDE.md's promotion gate requires that a construct lifted into the shared IR prove its
//! cross-language exact-equality invariants on REAL harvested code, not synthetic one-liners.
//! `src/frontend/c.rs`'s own `#[cfg(test)]` module already proves the headline gate
//! (`i += 1` ≡ `i = i + 1` ≡ `i++;`) on synthetic snippets; this suite re-asserts the SAME
//! invariants — plus the counter-loop-form rewrite — on functions harvested verbatim from the
//! Linux kernel v7.1 clone (`/home/babbitt/workspace/corpora/linux`, commit 8cd9520, tag v7.1),
//! matching `tests/convergence_real.rs`'s established pattern (provenance comments, harvested
//! snippet + hand-written twin, converge + precision-control pairs). Matching is same-language
//! (spec §3): every convergence invariant here compares a REAL C function to the SAME function
//! with the one construct under test respelled by hand.

use reprise::lang::Lang;

mod common;
use common::fp_ir;

// ============================================================================================
// Rung 1 — self-referential `@place` mutation: `x <op>= e` ≡ `x = x <op> e` ≡ `x++;`/`x--;`
// (the C frontend's own headline gate, src/frontend/c.rs's
// `self_ref_mutation_converges_across_all_three_c_spellings`, re-proven on real kernel code)
// ============================================================================================

// ---- lib/seq_buf.c:182-197 `seq_buf_puts` — local `len += 1;` AND field-target
// `s->len += len - 1;` (both real, both self-referential compound mutations) ----
const SEQ_BUF_PUTS_AUG: &str = r#"
int seq_buf_puts(struct seq_buf *s, const char *str)
{
	size_t len = strlen(str);

	WARN_ON(s->size == 0);

	/* Add 1 to len for the trailing null byte which must be there */
	len += 1;

	if (seq_buf_can_fit(s, len)) {
		memcpy(s->buffer + s->len, str, len);
		/* Don't count the trailing null byte against the capacity */
		s->len += len - 1;
		return 0;
	}
	seq_buf_set_overflow(s);
	return -1;
}
"#;
// Hand-written twin: both self-referential compound-assigns respelled explicitly
// (`x = x <op> e`) — the local `len` accumulator AND the field-target `s->len`.
const SEQ_BUF_PUTS_EXPLICIT: &str = r#"
int seq_buf_puts(struct seq_buf *s, const char *str)
{
	size_t len = strlen(str);

	WARN_ON(s->size == 0);

	len = len + 1;

	if (seq_buf_can_fit(s, len)) {
		memcpy(s->buffer + s->len, str, len);
		s->len = s->len + (len - 1);
		return 0;
	}
	seq_buf_set_overflow(s);
	return -1;
}
"#;

#[test]
fn real_seq_buf_puts_self_ref_mutations_converge_ir() {
    // `len += 1;` (local) and `s->len += len - 1;` (field-target) must each converge with
    // their explicit `x = x <op> e` respelling — the same @place rule Go/Rust/Python's
    // `+=`/member-target invariants prove, now on a real kernel accumulator.
    assert_eq!(
        fp_ir(SEQ_BUF_PUTS_AUG, Lang::C),
        fp_ir(SEQ_BUF_PUTS_EXPLICIT, Lang::C),
        "lib/seq_buf.c seq_buf_puts: `len += 1` / `s->len += len - 1` must converge with \
         explicit `x = x + ...` respellings",
    );
}

// ---- fs/ext4/ext4_jbd2.c:33-44 `ext4_get_nojournal` — `ref_cnt++;` (statement-position
// postfix increment) ----
const EXT4_GET_NOJOURNAL_INC: &str = r#"
static handle_t *ext4_get_nojournal(void)
{
	handle_t *handle = current->journal_info;
	unsigned long ref_cnt = (unsigned long)handle;

	BUG_ON(ref_cnt >= EXT4_NOJOURNAL_MAX_REF_COUNT);

	ref_cnt++;
	handle = (handle_t *)ref_cnt;

	current->journal_info = handle;
	return handle;
}
"#;
// Hand-written twins: the SAME real function with `ref_cnt++;` respelled as `ref_cnt += 1;`
// and as the fully-explicit `ref_cnt = ref_cnt + 1;`.
const EXT4_GET_NOJOURNAL_AUGEQ: &str = r#"
static handle_t *ext4_get_nojournal(void)
{
	handle_t *handle = current->journal_info;
	unsigned long ref_cnt = (unsigned long)handle;

	BUG_ON(ref_cnt >= EXT4_NOJOURNAL_MAX_REF_COUNT);

	ref_cnt += 1;
	handle = (handle_t *)ref_cnt;

	current->journal_info = handle;
	return handle;
}
"#;
const EXT4_GET_NOJOURNAL_EXPLICIT: &str = r#"
static handle_t *ext4_get_nojournal(void)
{
	handle_t *handle = current->journal_info;
	unsigned long ref_cnt = (unsigned long)handle;

	BUG_ON(ref_cnt >= EXT4_NOJOURNAL_MAX_REF_COUNT);

	ref_cnt = ref_cnt + 1;
	handle = (handle_t *)ref_cnt;

	current->journal_info = handle;
	return handle;
}
"#;

#[test]
fn real_ext4_get_nojournal_all_three_spellings_converge_ir() {
    // The headline three-way gate, on a real kernel function: `ref_cnt++;` ≡ `ref_cnt += 1;`
    // ≡ `ref_cnt = ref_cnt + 1;`.
    let target = fp_ir(EXT4_GET_NOJOURNAL_INC, Lang::C);
    assert_eq!(
        fp_ir(EXT4_GET_NOJOURNAL_AUGEQ, Lang::C),
        target,
        "ext4_get_nojournal: `ref_cnt++;` must converge with `ref_cnt += 1;`",
    );
    assert_eq!(
        fp_ir(EXT4_GET_NOJOURNAL_EXPLICIT, Lang::C),
        target,
        "ext4_get_nojournal: `ref_cnt++;` must converge with `ref_cnt = ref_cnt + 1;`",
    );
}

// ============================================================================================
// Rung 2 — counter-loop iteration form: `for (i=0; i<n; i++) …` ≡ the `while`-spelled twin
// (spec §5.2.2, the same rewrite convergence.rs's `rust_while_index_converges_with_foreach_ir`
// and `go_counter_loop_converges_with_rust_foreach_ir` prove for Rust/Go)
// ============================================================================================

// ---- fs/ext4/crypto.c:73-81 `uuid_is_zero` — a real, compact counter `for` loop with an
// unbraced single-statement (`if`) body, itself with an unbraced `return false;` ----
const UUID_IS_ZERO_FOR: &str = r#"
static bool uuid_is_zero(__u8 u[16])
{
	int i;

	for (i = 0; i < 16; i++)
		if (u[i])
			return false;
	return true;
}
"#;
// Hand-written twin: the same real function, the `for` respelled as an index-hoisted `while`.
const UUID_IS_ZERO_WHILE: &str = r#"
static bool uuid_is_zero(__u8 u[16])
{
	int i;

	i = 0;
	while (i < 16) {
		if (u[i])
			return false;
		i++;
	}
	return true;
}
"#;

#[test]
fn real_uuid_is_zero_for_converges_with_while_twin_ir() {
    assert_eq!(
        fp_ir(UUID_IS_ZERO_FOR, Lang::C),
        fp_ir(UUID_IS_ZERO_WHILE, Lang::C),
        "crypto.c uuid_is_zero: the `for` loop must converge with its `while`-spelled twin",
    );
}

// ============================================================================================
// Rung 3 — PRECISION: genuinely-different real/real-derived C code must stay distinct
// ============================================================================================

#[test]
fn real_increment_stays_distinct_from_decrement_ir() {
    // ext4_get_nojournal's `ref_cnt++;` must NOT collapse with a `ref_cnt--;` respelling — the
    // update_expression desugar (Family B / D-IR-14) must preserve the `+`/`-` distinction.
    let dec = EXT4_GET_NOJOURNAL_INC.replace("ref_cnt++;", "ref_cnt--;");
    assert_ne!(
        fp_ir(EXT4_GET_NOJOURNAL_INC, Lang::C),
        fp_ir(&dec, Lang::C),
        "ext4_get_nojournal: `ref_cnt++` must stay distinct from `ref_cnt--`",
    );
}

#[test]
fn real_self_ref_accumulate_stays_distinct_from_flat_reassign_ir() {
    // seq_buf_puts's self-referential accumulate `len += 1;` (value shape `Binop{len, +, 1}`)
    // must NOT collapse with a flat, non-self-referential constant reassign `len = 1;` (value
    // shape `Lit`) — even though C's plain `=` is ALWAYS `@place` (declarations are a separate
    // grammar rule, so there is no Python-style @target/@place split on self-reference; see
    // `lower_assignment_c`'s doc comment), the accumulate's VALUE shape is genuinely different
    // from a flat constant and must not be normalized away.
    let flat = SEQ_BUF_PUTS_AUG.replace("len += 1;", "len = 1;");
    assert_ne!(
        fp_ir(SEQ_BUF_PUTS_AUG, Lang::C),
        fp_ir(&flat, Lang::C),
        "seq_buf_puts: `len += 1` (accumulate) must stay distinct from `len = 1` (flat reassign)",
    );
}

#[test]
fn two_unrelated_real_kernel_functions_stay_distinct_ir() {
    // Sanity: two genuinely different, independently-harvested real kernel functions (crypto.c's
    // array scan vs ext4_jbd2.c's counter increment) must not accidentally collide.
    assert_ne!(
        fp_ir(UUID_IS_ZERO_FOR, Lang::C),
        fp_ir(EXT4_GET_NOJOURNAL_INC, Lang::C),
        "uuid_is_zero and ext4_get_nojournal are unrelated and must not collide",
    );
}

// ============================================================================================
// FINDING — a `continue` inside a C-style `for (init; cond; update)` loop, when the update
// clause is appended AFTER the body (mirrors Go's identical `for_clause` handling — this is a
// PRE-EXISTING, shared (Go+C) limitation of the update-append shape, not something new to C),
// does not reach EXACT fingerprint equality with its `while`-spelled twin: the twin's `i++`
// (inserted right before every `continue`, the only sound manual translation) becomes a real
// sibling statement inside the guard's `Block`, one the `for`-loop's implicit
// continue-runs-the-update semantics never materializes as an explicit node. The two trees are
// therefore not byte-identical — `assert_eq!` on their s-expressions fails — even though they
// are semantically equivalent C. Recall is NOT lost: reprise's similarity engine's fuzzy
// near-tier matching (histogram voting / AU acceptance, not literal fingerprint equality) still
// recalls the pair at `near-normalized` (proven positively by
// `benches/mutations/variants/c/seed1.t3-loop-swap.c`, which converges 6/6 in the mutation
// bench). This is parked here, verbatim, as a documented non-blocking finding — not weakened to
// a passing assertion — per the TDD discipline (module doc comment / CLAUDE.md's promotion
// gate): a real SMALL fix is not evident (it would mean redesigning the shared
// `Continue`-inside-a-`for`-loop lowering to special-case which statements ALWAYS run before a
// jump back to the guard, a change affecting Go too), so it is deferred rather than forced.
#[test]
#[ignore = "FINDING: a `continue` inside a for-loop whose update clause is appended after the \
            body does not reach EXACT ir fingerprint equality with its while-spelled twin \
            (pre-existing, shared with Go's identical for_clause handling, not C-specific); \
            near-tier fuzzy matching still recalls the pair, so this is a precision nicety, not \
            a recall gap — see the module-level FINDING comment above this test"]
fn continue_inside_for_with_update_does_not_reach_exact_equality_with_while_twin_ir() {
    let for_form = r#"
void f(int *xs, int n) {
    for (int i = 0; i < n; i++) {
        if (xs[i] < 0) {
            continue;
        }
        g(xs[i]);
    }
}
"#;
    let while_form = r#"
void f(int *xs, int n) {
    int i = 0;
    while (i < n) {
        if (xs[i] < 0) {
            i++;
            continue;
        }
        g(xs[i]);
        i++;
    }
}
"#;
    assert_eq!(
        fp_ir(for_form, Lang::C),
        fp_ir(while_form, Lang::C),
        "a for-loop with a mid-body continue does not (yet) reach exact equality with its \
         semantically-equivalent while twin",
    );
}
