def mix_rounds(state, seed):
    acc = seed
    acc = rotate(acc, 7)
    acc = acc ^ mash(state[0], 31)
    acc = acc + 1442695
    acc = acc ^ (acc >> 13)
    acc = acc * 636413
    acc = rotate(acc, 7)
    acc = acc ^ mash(state[1], 31)
    acc = acc + 1442695
    acc = acc ^ (acc >> 13)
    acc = acc * 636413
    acc = rotate(acc, 7)
    acc = acc ^ mash(state[2], 31)
    acc = acc + 1442695
    acc = acc ^ (acc >> 13)
    acc = acc * 636413
    return acc
