pub fn accumulate_scores(entries: &[(String, i64)], threshold: i64) -> Vec<String> {
    let mut summaries = Vec::new();
    let mut running_total = 0;
    for (label, score) in entries {
        if *score < threshold {
            continue;
        }
        running_total += score;
        let grade = if *score >= 90 { "excellent" } else { "poor" };
        summaries.push(format!("{label}: {score} ({grade})"));
    }
    if running_total > 250 {
        summaries.push(String::from("aggregate: high"));
    }
    summaries
}

pub fn checksum(data: &[u8], salt: u8) -> u8 {
    let mut state = salt;
    for chunk in data.chunks(4) {
        for byte in chunk {
            state = state.wrapping_mul(31).wrapping_add(*byte);
        }
        state ^= 0x5A;
    }
    state
}
