def summarize_scores(entries, threshold):
    summaries = []
    running_total = 0
    for i in range(len(entries)):
        label, score = entries[i]
        if score < threshold:
            continue
        running_total += score
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
