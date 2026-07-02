//! Dev utility: dump tree-sitter CSTs to design/debug normalization rules.
//! Usage: cargo run --example probe

fn dump(lang: &tree_sitter::Language, src: &str) {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(lang).unwrap();
    let tree = parser.parse(src, None).unwrap();
    let mut cursor = tree.root_node().walk();
    let mut depth = 0usize;
    loop {
        let node = cursor.node();
        let field = cursor
            .field_name()
            .map(|f| format!("{f}: "))
            .unwrap_or_default();
        let anon = if node.is_named() { "" } else { " (anon)" };
        let text = if node.child_count() == 0 {
            format!(" {:?}", &src[node.byte_range()])
        } else {
            String::new()
        };
        println!(
            "{}{}{}{}{}",
            "  ".repeat(depth),
            field,
            node.kind(),
            anon,
            text
        );
        if cursor.goto_first_child() {
            depth += 1;
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return;
            }
            depth -= 1;
        }
    }
}

fn main() {
    let rust: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
    let python: tree_sitter::Language = tree_sitter_python::LANGUAGE.into();

    println!("=== RUST while/loop/for ===");
    dump(
        &rust,
        r#"fn drain(n: i64) -> i64 {
    let mut k = n;
    while k > 0 {
        k -= 2;
    }
    loop {
        if !(k > 0) {
            break;
        }
        k += 1;
    }
    for (a, b) in pairs {
        k += a;
    }
    k
}"#,
    );

    println!("\n=== RUST literals/idents/paths ===");
    dump(
        &rust,
        r#"fn f(xs: &[(String, i64)]) -> String {
    let s = String::from("x");
    let c = 'y';
    let ok = true;
    if let Some((h, t)) = xs.split_once(':') { h.parse::<u16>(); }
    format!("{s}: {c}")
}"#,
    );

    println!("\n=== PYTHON while/for/if ===");
    dump(
        &python,
        "def drain(n):\n    k = n\n    while k > 0:\n        k -= 2\n    while True:\n        if not (k > 0):\n            break\n        k += 1\n    for a, b in pairs:\n        k += a\n    return k\n",
    );

    println!("\n=== RUST tuple-assign / indexed-for / continue ===");
    dump(
        &rust,
        "fn g(mut a: u64, mut b: u64, xs: &[u64]) -> u64 {\n    let mut s = 0;\n    (a, b) = (b, a % b);\n    for i in 0..xs.len() {\n        s += xs[i];\n    }\n    loop {\n        if a == 0 { return s; }\n        a -= 1;\n        continue;\n    }\n}",
    );
    println!("\n=== PY tuple-assign / range-len-for ===");
    dump(
        &python,
        "def g(a, b, xs):\n    s = 0\n    a, b = b, a % b\n    for i in range(len(xs)):\n        s += xs[i]\n    while True:\n        if a == 0:\n            return s\n        a -= 1\n        continue\n",
    );

    println!("\n=== PYTHON literals/attrs/except ===");
    dump(
        &python,
        "def f(raw, port=80):\n    ok = True\n    s = raw.strip()\n    if \":\" in s:\n        head, _, tail = s.partition(\":\")\n        try:\n            port = int(tail)\n        except ValueError as exc:\n            pass\n    return [s, \"://\", str(port)]\n",
    );
}
