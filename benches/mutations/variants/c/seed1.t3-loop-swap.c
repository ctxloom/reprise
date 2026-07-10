/* t3-loop-swap: the seed's C-style `for` loop respelled as an index-hoisted `while`
 * loop (init hoisted above, `i++` before every `continue` plus at the tail) — the
 * same loop-form swap Go's seed1.t3-loop-swap.go exercises (there: range-for ->
 * C-style for; here, C has no range-for, so the swap is for<->while per the WP
 * brief). Converges at near-normalized (not exact): a continue INSIDE a for-loop
 * whose update clause is appended after the body is a known shared (Go+C) IR gap
 * in the shared for-loop lowering — see the WP report's FINDING. Near-tier fuzzy
 * matching still recalls it, which is all this class's gate requires. */

static int classify_scores(const int *scores, int count, int threshold,
                            const char **grades, int *total)
{
    int written = 0;
    int sum = 0;

    int i = 0;
    while (i < count) {
        int score = scores[i];

        if (score < threshold) {
            i++;
            continue;
        }

        sum += score;

        if (score >= 90) {
            grades[written] = "excellent";
        } else if (score >= 50) {
            grades[written] = "adequate";
        } else {
            grades[written] = "poor";
        }

        written++;
        i++;
    }

    *total = sum;

    if (sum > 250) {
        grades[written++] = "aggregate";
    }

    return written;
}
