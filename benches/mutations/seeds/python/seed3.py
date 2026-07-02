def reduce_pair(a, b, log):
    while b != 0:
        log.append("step: " + str(a) + " " + str(b))
        a, b = b, a % b
    return a
