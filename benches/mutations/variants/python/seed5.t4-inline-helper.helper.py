def record_reading(cleaned, v, limit):
    if v > limit:
        cleaned.append(limit)
    elif v < 0:
        cleaned.append(0)
    else:
        cleaned.append(v)
