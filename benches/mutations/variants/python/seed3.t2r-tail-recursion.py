def reduce_pair(a, b, log):
    if b == 0:
        return a
    log.append("step: " + str(a) + " " + str(b))
    return reduce_pair(b, a % b, log)
