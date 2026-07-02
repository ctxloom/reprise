pub fn summarize_scores(entries: &[(String, i64)], threshold: i64) -> Vec<String> {
    let mut summaries = Vec::new();
    let mut running_total = 0;
    let mut i = 0;
    while i < entries.len() {
        let (label, score) = &entries[i];
        i += 1;
        if *score < threshold {
            continue;
        }
        running_total += score;
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
