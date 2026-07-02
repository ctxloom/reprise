fn reduce_pair(mut a: u64, mut b: u64, log: &mut Vec<String>) -> u64 {
    while b != 0 {
        log.push(format!("step: {a} {b}"));
        (a, b) = (b, a % b);
    }
    a
}
