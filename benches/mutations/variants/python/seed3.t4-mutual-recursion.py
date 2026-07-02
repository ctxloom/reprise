def reduce_pair(a, b, log):
    if b == 0:
        return a
    return shrink_pair(a, b, log)


def shrink_pair(a, b, log):
    log.append("step: " + str(a) + " " + str(b))
    return reduce_pair(b, a % b, log)
