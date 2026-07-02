def summarize_scores(entries, threshold):
    summaries = []
    running_total = 0
    for label, score in entries:
        if score < threshold:
            continue
        running_total += score // 2
        if score >= 90:
            grade = "excellent"
        elif score >= 50:
            grade = "adequate"
        else:
            grade = "poor"
        summaries.append(label + ": " + str(score) + " (" + grade + ")")
    if running_total > 250:
        summaries.append("aggregate: high")
    return summaries
