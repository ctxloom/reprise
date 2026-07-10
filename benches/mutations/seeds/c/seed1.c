/* seed1.c — kernel-styled score-classification helper (C mutation-bench seed,
 * hand-written, mirrors the Go/Rust/Python "summarize_scores" seed family):
 * fixed-size output buffer + out-param, no dynamic allocation, per kernel idiom. */

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

        sum += score;

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
