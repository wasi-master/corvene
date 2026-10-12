//! Token equivalence checks for the JavaScript and CSS printers.

use crate::{Language, css, js, pretty};

fn js_tokens(text: &str) -> Vec<(js::Kind, &str)> {
    js::tokenize(text)
        .into_iter()
        .map(|t| (t.kind, &text[t.start..t.end]))
        .collect()
}

/// Pretty prints `input` and checks the output has the same tokens, prints
/// to itself again (idempotent) and keeps every source line break.
fn check_js(input: &str) -> String {
    let out = pretty(Language::JavaScript, input);
    let a = js_tokens(input);
    let b = js_tokens(&out);
    if a != b {
        let n = a
            .iter()
            .zip(&b)
            .position(|(x, y)| x != y)
            .unwrap_or(a.len().min(b.len()));
        panic!(
            "token mismatch at {n}: {:?} vs {:?}\n--- output ---\n{out}",
            a.get(n.saturating_sub(2)..(n + 3).min(a.len())),
            b.get(n.saturating_sub(2)..(n + 3).min(b.len()))
        );
    }
    let again = pretty(Language::JavaScript, &out);
    assert_eq!(again, out, "not idempotent");
    out
}

const JS_SAMPLES: &[&str] = &[
    include_str!("../tests/fixtures/umd.min.js"),
    "a=b?c:d;x=y?{a:1}:[2]",
    "for(let i=0,j=1;i<j;i++,j--){}for(;;)break",
    "for await(const x of y)z(x)",
    "if(a)b();else if(c)d();else{e()}",
    "do x++;while(x<5)",
    "x=a/b/c;y=/re[/]x/g.test(z);z=(1)/2;w=[1]/2",
    "a = b\n++c\nreturn\nx",
    "let s=`a${b+`c${d}e`}f${{x:1}.x}`",
    "var r=1..toString()+1 .toFixed()+.5+0x1F+1e-7+1_000n",
    "a+ +b;a- -b;a+ ++b;a++ +b;a-- -b",
    "x?.y?.[0]?.(1);a??=b;c||=d;e&&=f;g**=2;h>>>=1",
    "class A extends B{#x=1;static #y;get x(){return this.#x}set x(v){this.#x=v}static{init()}}",
    "label:for(;;){continue label}",
    "async function f(){await g();for await(const a of b);}const h=async x=>x,i=async(x)=>{}",
    "function*g(){yield*h();yield 1}",
    "switch(a){case 1:case 2:b();break;default:c()}",
    "const {a,b:[c,d],...e}=f;import{x as y}from'z';export default{a}",
    "//c1\na()/*c2*/;/*! keep */b()\n/**\n * doc\n */\nfunction c(){}",
    "throw new Error('x')\nreturn/*c*/x",
    "let x: Map<string, Array<number>> = new Map<string,number[]>();function f<T extends object>(a?: T, b!: number): T|undefined{return a}",
    "const el=<div className=\"x\">{items.map(i=><Item key={i.id} {...i}/>)}</div>",
    "let ü=1,_$=2,\\u0061b=3;x=ü+_$",
    "a\u{2028}b",
    "'unterminated\nnext line",
    "/unterminated regex\nx",
    "`unterminated ${a",
    "}}}))]]",
    "((((",
    "a=>b=>c=>d",
    "#!/usr/bin/env node\nconsole.log(1)",
    "x=y<z>w;a<b>(c)",
    "if(a)return b",
    "a=function(){}\n(b)",
    "var a=1,b=2,c=3;let d=a?b:c,e=1",
    "x={a:1,b:{c:2,d:[3,4]},e:function(){return{f:5}}}",
    "!function(){}(),void 0,+function(){}()",
    "a=b\n/c/g",
    "a.if.else.return=1;x.default=2;y={case:1,default:2}",
];

#[test]
fn js_output_keeps_tokens() {
    for sample in JS_SAMPLES {
        check_js(sample);
    }
}

#[test]
fn js_output_parses_like_input_on_mutations() {
    // Every prefix of the minified sample (truncated files) keeps tokens.
    let sample = include_str!("../tests/fixtures/umd.min.js");
    let mut cut = 0;
    while cut < sample.len() {
        if sample.is_char_boundary(cut) {
            let out = pretty(Language::JavaScript, &sample[..cut]);
            // An unterminated comment or template at the end absorbs the
            // trailing newline the printer adds.
            fn trim(mut v: Vec<(js::Kind, &str)>) -> Vec<(js::Kind, &str)> {
                if let Some(last) = v.last_mut() {
                    last.1 = last.1.trim_end();
                }
                v
            }
            assert_eq!(
                trim(js_tokens(&sample[..cut])),
                trim(js_tokens(&out)),
                "prefix {cut}"
            );
        }
        cut += 37;
    }
}

#[test]
fn css_output_keeps_tokens() {
    let samples = [
        include_str!("../tests/fixtures/site.min.css"),
        "a{color:red}b,c>d:hover{margin:0 auto;padding:0}",
        "@media (max-width:600px){.a{x:y}}@supports (display:grid) and (not (display:inline-grid)){.b{c:d}}",
        ".a{&:hover{color:red}.b &{c:d}}",
        "a{background:url(data:image/png;base64,iVBOR==) no-repeat;content:\"a;b{c}\"}",
        "/*! license */a{b:c}/* x */d{e:f}",
        "a{b:c",
        "}}}{{{",
    ];
    for s in samples {
        let out = pretty(Language::Css, s);
        let strip = |t: &str| -> Vec<String> {
            css::tokenize(t, false)
                .into_iter()
                .filter(|k| k.kind != css::Kind::Space)
                .map(|k| t[k.start..k.end].to_string())
                .collect()
        };
        assert_eq!(strip(s), strip(&out), "{out}");
        assert_eq!(pretty(Language::Css, &out), out, "not idempotent");
    }
}

/// Scratch check on a real file: `PRETTY_DUMP=path PRETTY_OUT=out cargo
/// test -p corvene-pretty --release dump_env -- --ignored`.
#[test]
#[ignore]
fn dump_env() {
    let Ok(path) = std::env::var("PRETTY_DUMP") else {
        return;
    };
    let text = std::fs::read_to_string(&path).unwrap();
    let lang = Language::of_path(&path).unwrap();
    let start = std::time::Instant::now();
    let n = js::tokenize(&text).len();
    eprintln!("tokenize {n} in {:?}", start.elapsed());
    let start = std::time::Instant::now();
    let out = pretty(lang, &text);
    eprintln!(
        "{} bytes -> {} lines in {:?}",
        text.len(),
        out.lines().count(),
        start.elapsed()
    );
    if matches!(lang, Language::JavaScript | Language::TypeScript) {
        assert!(js_tokens(&text) == js_tokens(&out), "token mismatch");
    } else {
        let strip = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
        assert!(strip(&text) == strip(&out), "text mismatch");
    }
    if let Ok(dest) = std::env::var("PRETTY_OUT") {
        std::fs::write(dest, out).unwrap();
    }
}

#[test]
fn js_random_inputs_keep_tokens() {
    let alphabet = [
        "{", "}", "[", "]", "(", ")", "<", ">", ">>", "/", "*", "\"", "'", "`", "${", "\\", ";",
        ",", ":", "=", "?", "?.", ".", "!", "-", "+", "++", "#", "@", "//", "/*", "*/", "\n", " ",
        "a", "1", ".5", "1e", "é", "return", "case", "default", "switch", "for", "await", "of",
        "yield", "function", "=>", "async", "x", "in", "typeof", "else", "if", "...",
    ];
    let mut seed: u64 = 0x5555_aaaa_3333_cccc;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..4000 {
        let len = (next() % 80) as usize;
        let s: String = (0..len)
            .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
            .collect();
        for ts in [false, true] {
            let out = js::pretty(&s, ts);
            fn trim(mut v: Vec<(js::Kind, &str)>) -> Vec<(js::Kind, &str)> {
                if let Some(last) = v.last_mut() {
                    last.1 = last.1.trim_end();
                }
                v
            }
            assert_eq!(trim(js_tokens(&s)), trim(js_tokens(&out)), "{s:?}\n{out}");
        }
    }
}

#[test]
fn css_random_inputs_keep_tokens() {
    let alphabet = [
        "{",
        "}",
        "(",
        ")",
        ";",
        ",",
        ":",
        "\"",
        "'",
        "\\",
        "/*",
        "*/",
        "//",
        "url(",
        "@media",
        "&",
        ">",
        "+",
        "a",
        "b-c",
        "#fff",
        "!important",
        " ",
        "\n",
        "\t",
        "é",
        "0",
        ".5",
        "--x",
        "http://",
        "/",
        "*",
    ];
    let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let tokens = |t: &str, lc: bool| -> Vec<String> {
        css::tokenize(t, lc)
            .into_iter()
            .filter(|k| k.kind != css::Kind::Space)
            .map(|k| t[k.start..k.end].trim_end().to_string())
            .collect()
    };
    for _ in 0..4000 {
        let len = (next() % 60) as usize;
        let s: String = (0..len)
            .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
            .collect();
        for lc in [false, true] {
            let out = css::pretty(&s, lc);
            assert_eq!(tokens(&s, lc), tokens(&out, lc), "{s:?}\n{out}");
        }
    }
}
