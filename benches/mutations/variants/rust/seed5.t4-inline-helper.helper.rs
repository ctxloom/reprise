fn clamp_reading(value: f64, limit: f64) -> f64 {
    if value > limit {
        limit
    } else if value < 0.0 {
        0.0
    } else {
        value
    }
}
