def resolve_endpoint(raw, default_port, secure):
    cleaned = raw.strip()
    host = ""
    port = default_port
    if ":" in cleaned:
        head, _, tail = cleaned.partition(":")
        host = head
        try:
            port = int(tail)
        except ValueError:
            port = default_port
    else:
        host = cleaned
    if secure:
        scheme = "https"
    else:
        scheme = "http"
    pieces = [scheme, "://", host, ":", str(port)]
    return "".join(pieces)
