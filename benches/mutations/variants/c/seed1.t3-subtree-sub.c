/* t3-subtree-sub: one subexpression changed (`sum += score` -> `sum += score / 2`),
 * mirroring Go/Rust/Python's seed1.t3-subtree-sub — the rest of the tree untouched. */

static int classify_scores(const int *scores, int count, int threshold,
                            const char **grades, int *total)
{
    int written = 0;
    int sum = 0;

    for (int i = 0; i < count; i++) {
        int score = scores[i];

        if (score < threshold) {
            continue;
        }

        sum += score / 2;

        if (score >= 90) {
            grades[written] = "excellent";
        } else if (score >= 50) {
            grades[written] = "adequate";
        } else {
            grades[written] = "poor";
        }

        written++;
    }

    *total = sum;

    if (sum > 250) {
        grades[written++] = "aggregate";
    }

    return written;
}
