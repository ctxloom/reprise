fn reduce_pair(a: u64, b: u64, log: &mut Vec<String>) -> u64 {
    if b == 0 {
        return a;
    }
    log.push(format!("step: {a} {b}"));
    let left = reduce_pair(b, a % b, log);
    let right = reduce_pair(b % 3, a, log);
    left.max(right)
}
