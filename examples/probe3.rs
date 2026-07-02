//! Dev utility: dump tree-sitter CSTs for the M3c languages (TypeScript/TSX,
//! Go, Kotlin) to design normalization rules (DECISIONS.md D9: never write a
//! node-kind string that hasn't been seen in probe output).
//! Usage: cargo run --example probe3 [ts|tsx|go|kt|all]

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
        let err = if node.is_error() { " <<ERROR>>" } else { "" };
        let text = if node.child_count() == 0 {
            format!(" {:?}", &src[node.byte_range()])
        } else {
            String::new()
        };
        println!(
            "{}{}{}{}{}{}",
            "  ".repeat(depth),
            field,
            node.kind(),
            anon,
            text,
            err
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
    let which = std::env::args().nth(1).unwrap_or_else(|| "all".into());
    let ts: tree_sitter::Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
    let tsx: tree_sitter::Language = tree_sitter_typescript::LANGUAGE_TSX.into();
    let go: tree_sitter::Language = tree_sitter_go::LANGUAGE.into();
    let kt: tree_sitter::Language = tree_sitter_kotlin_ng::LANGUAGE.into();

    if which == "ts" || which == "all" {
        println!("=== TS function/method/arrow ===");
        dump(
            &ts,
            r#"function total(xs: number[], floor: number): number {
    let acc = 0;
    for (const x of xs) { if (x > floor) { acc += x; } }
    return acc;
}
class C {
    method(a: number): number { return a + 1; }
}
const inc = (x: number): number => x + 1;
const dec = (x) => { return x - 1; };
const obj = { a: 1, b: "s" };
obj.prop.value = foo.bar(1);
"#,
        );
        println!("\n=== TS loops: while/do/for/for-of/for-in/index ===");
        dump(
            &ts,
            r#"function loops(xs: number[]) {
    let k = 0;
    while (k > 0) { k -= 1; }
    do { k += 1; } while (k < 10);
    for (let i = 0; i < xs.length; i++) { use(xs[i]); }
    for (const x of xs) { use(x); }
    for (const key in obj) { use(key); }
}
"#,
        );
        println!("\n=== TS recursion + literals + return ===");
        dump(
            &ts,
            r#"function reduce_pair(a: number, b: number): number {
    if (b === 0) { return a; }
    return reduce_pair(b, a % b);
}
const s = `hi ${x}`;
const n = 42;
const f = 3.14;
const t = true;
"#,
        );
        println!("\n=== TS test wrappers ===");
        dump(
            &ts,
            r#"describe("suite", () => {
    it("works", () => { expect(1).toBe(1); });
    test("also", () => { expect(2).toBe(2); });
});
"#,
        );
    }

    if which == "tsx" || which == "all" {
        println!("\n=== TSX component ===");
        dump(
            &tsx,
            r#"function App(props: Props) {
    const [n, setN] = useState(0);
    return <div className="x">{n}</div>;
}
"#,
        );
    }

    if which == "go" || which == "all" {
        println!("\n=== GO func/method/loops ===");
        dump(
            &go,
            r#"package main

func total(xs []int, floor int) int {
    acc := 0
    for _, x := range xs {
        if x > floor {
            acc += x
        }
    }
    for i := 0; i < len(xs); i++ {
        use(xs[i])
    }
    for acc > 0 {
        acc--
    }
    return acc
}

func (r *Repo) Method(a int) int {
    var b int = a
    return b + 1
}
"#,
        );
        println!("\n=== GO recursion + literals + selector ===");
        dump(
            &go,
            r#"func reducePair(a int, b int) int {
    if b == 0 {
        return a
    }
    return reducePair(b, a%b)
}

func lits() {
    s := "str"
    n := 42
    f := 3.14
    t := true
    obj.Field = pkg.Call(1)
    m := map[string]int{"a": 1, "b": 2}
}
"#,
        );
        println!("\n=== GO test ===");
        dump(
            &go,
            r#"func TestSummarize(t *testing.T) {
    if got := summarize(1); got != 2 {
        t.Errorf("bad")
    }
}
"#,
        );
    }

    if which == "cores" || which == "all" {
        println!("\n=== TS cores: while(true)/break/continue/!/destructure/func-expr ===");
        dump(
            &ts,
            r#"function core(k: number) {
    while (true) {
        if (!(k > 0)) { break; }
        k -= 1;
        continue;
    }
    [a, b] = [b, a % b];
}
const fe = function(x) { return x + 1; };
"#,
        );
        println!("\n=== GO cores: for{{}}/break/continue/!/multi-assign ===");
        dump(
            &go,
            r#"package main
func core(k int) {
    for {
        if !(k > 0) {
            break
        }
        k -= 1
        continue
    }
    a, b = b, a%b
}
"#,
        );
        println!("\n=== KOTLIN cores: while(true)/break/continue/! ===");
        dump(
            &kt,
            r#"fun core(k: Int) {
    while (true) {
        if (!(k > 0)) {
            break
        }
        k -= 1
        continue
    }
    a = b
}
"#,
        );
    }

    if which == "kt" || which == "all" {
        println!("\n=== KOTLIN fun/loops ===");
        dump(
            &kt,
            r#"fun total(xs: List<Int>, floor: Int): Int {
    var acc = 0
    while (acc > 0) { acc -= 1 }
    do { acc += 1 } while (acc < 10)
    for (x in xs) { if (x > floor) { acc += x } }
    for (i in xs.indices) { use(xs[i]) }
    for (i in 0 until xs.size) { use(xs[i]) }
    return acc
}
"#,
        );
        println!("\n=== KOTLIN recursion + when + literals + navigation ===");
        dump(
            &kt,
            r#"fun reducePair(a: Int, b: Int): Int {
    if (b == 0) {
        return a
    }
    return reducePair(b, a % b)
}

fun lits(x: Int): String {
    val s = "str"
    val n = 42
    val f = 3.14
    val t = true
    obj.field = pkg.call(1)
    return when (x) {
        0 -> "zero"
        1 -> "one"
        else -> "many"
    }
}
"#,
        );
        println!("\n=== KOTLIN test ===");
        dump(
            &kt,
            r#"class T {
    @Test
    fun testSummarize() {
        assertEquals(2, summarize(1))
    }
}
"#,
        );
    }
}
