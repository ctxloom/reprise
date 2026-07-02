def normalize_readings(values, limit):
    cleaned = []
    for v in values:
        record_reading(cleaned, v, limit)
    total = 0
    for c in cleaned:
        total = total + c
    if total > 500:
        cleaned.append(total / 2)
    return cleaned
