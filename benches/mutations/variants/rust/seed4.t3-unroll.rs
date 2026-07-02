fn mix_rounds(state: &[u64], seed: u64) -> u64 {
    let mut acc = seed;
    acc = acc.rotate_left(7);
    acc ^= state[0].wrapping_mul(31);
    acc = acc.wrapping_add(1442695);
    acc ^= acc >> 13;
    acc = acc.wrapping_mul(636413);
    acc = acc.rotate_left(7);
    acc ^= state[1].wrapping_mul(31);
    acc = acc.wrapping_add(1442695);
    acc ^= acc >> 13;
    acc = acc.wrapping_mul(636413);
    acc = acc.rotate_left(7);
    acc ^= state[2].wrapping_mul(31);
    acc = acc.wrapping_add(1442695);
    acc ^= acc >> 13;
    acc = acc.wrapping_mul(636413);
    acc
}
