pub fn summarize_scores(entries: &[(String, i64)], threshold: i64) -> Vec<String> {
    let mut summaries = Vec::new();
    let mut running_total = 0;
    for (label, score) in entries {
        if *score < threshold {
            continue;
        }
        running_total += *score / 2;
        let grade = if *score >= 90 {
            "excellent"
        } else if *score >= 50 {
            "adequate"
        } else {
            "poor"
        };
        summaries.push(format!("{label}: {score} ({grade})"));
    }
    if running_total > 250 {
        summaries.push(String::from("aggregate: high"));
    }
    summaries
}
