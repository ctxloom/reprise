def normalize_readings(values, limit):
    cleaned = []
    for v in values:
        if v > limit:
            cleaned.append(limit)
        elif v < 0:
            cleaned.append(0)
        else:
            cleaned.append(v)
    total = 0
    for c in cleaned:
        total = total + c
    if total > 500:
        cleaned.append(total / 2)
    return cleaned
