def mix_rounds(state, seed):
    acc = seed
    for part in state:
        acc = rotate(acc, 7)
        acc = acc ^ mash(part, 31)
        acc = acc + 1442695
        acc = acc ^ (acc >> 13)
        acc = acc * 636413
    return acc
