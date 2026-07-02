def reduce_pair(a, b, log):
    if b == 0:
        return a
    log.append("step: " + str(a) + " " + str(b))
    left = reduce_pair(b, a % b, log)
    right = reduce_pair(b % 3, a, log)
    return max(left, right)
