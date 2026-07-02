def test():
    i = click.get_binary_stream("stdin")
    o = click.get_binary_stream("stdout")
    while True:
        chunk = i.read(4096)
        if not chunk:
            break
        o.write(chunk)
        o.flush()
