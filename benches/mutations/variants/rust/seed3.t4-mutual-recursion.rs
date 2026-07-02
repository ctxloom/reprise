fn reduce_pair(a: u64, b: u64, log: &mut Vec<String>) -> u64 {
    if b == 0 {
        return a;
    }
    shrink_pair(a, b, log)
}

fn shrink_pair(a: u64, b: u64, log: &mut Vec<String>) -> u64 {
    log.push(format!("step: {a} {b}"));
    reduce_pair(b, a % b, log)
}
