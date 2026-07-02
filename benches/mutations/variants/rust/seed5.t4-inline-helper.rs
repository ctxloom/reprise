pub fn normalize_readings(values: &[f64], limit: f64) -> Vec<f64> {
    let mut cleaned = Vec::new();
    for v in values {
        let adjusted = if *v > limit {
            limit
        } else if *v < 0.0 {
            0.0
        } else {
            *v
        };
        cleaned.push(adjusted * 10.0);
    }
    let mut total = 0.0;
    for c in &cleaned {
        total += *c;
    }
    if total > 500.0 {
        cleaned.push(total / 2.0);
    }
    cleaned
}
