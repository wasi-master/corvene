//! Golden outputs, text preservation, minified detection and speed.

use corvene_pretty::{Language, is_minified, is_minified_name, pretty};
use std::time::{Duration, Instant};

const UMD: &str = include_str!("fixtures/umd.min.js");
const CSS: &str = include_str!("fixtures/site.min.css");

fn strip(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

#[test]
fn paths() {
    assert_eq!(
        Language::of_path("a/b/app.min.js"),
        Some(Language::JavaScript)
    );
    assert_eq!(Language::of_path("x.TSX"), Some(Language::TypeScript));
    assert_eq!(Language::of_path("dist/app.js.map"), Some(Language::Json));
    assert_eq!(Language::of_path("site.webmanifest"), Some(Language::Json));
    assert_eq!(Language::of_path("theme.scss"), Some(Language::Scss));
    assert_eq!(Language::of_path("icon.svg"), Some(Language::Xml));
    assert_eq!(Language::of_path("Info.plist"), Some(Language::Xml));
    assert_eq!(Language::of_path("index.htm"), Some(Language::Html));
    assert_eq!(Language::of_path("Makefile"), None);
    assert_eq!(Language::of_path("notes.md"), None);
    assert_eq!(Language::of_path("dir.json/file"), None);
    assert_eq!(Language::Json.name(), "JSON");
    assert_eq!(Language::JavaScript.name(), "JavaScript");

    assert!(is_minified_name("app.min.js"));
    assert!(is_minified_name("dist/style.MIN.css"));
    assert!(is_minified_name("x.min.mjs"));
    assert!(is_minified_name("bundle.min.json"));
    assert!(!is_minified_name("admin.js"));
    assert!(!is_minified_name("min.js"));
    assert!(!is_minified_name("a.min/app.js"));
}

#[test]
fn json_golden() {
    let input =
        r#"{"b":1,"a":[1,2.50,-3e+2,true,null],"e":{},"f":[],"s":"x\"é\n","n":{"x":{"y":[{}]}}}"#;
    let expected = r#"{
  "b": 1,
  "a": [
    1,
    2.50,
    -3e+2,
    true,
    null
  ],
  "e": {},
  "f": [],
  "s": "x\"é\n",
  "n": {
    "x": {
      "y": [
        {}
      ]
    }
  }
}
"#;
    assert_eq!(pretty(Language::Json, input), expected);
    assert_eq!(pretty(Language::Json, expected), expected);
    assert_eq!(
        pretty(Language::Json, "{\"a\":1}\n{\"a\":2}"),
        "{\n  \"a\": 1\n}\n{\n  \"a\": 2\n}\n"
    );
    assert_eq!(
        pretty(Language::Json, " 12345678901234567890.0000000001 "),
        "12345678901234567890.0000000001\n"
    );
    assert_eq!(pretty(Language::Json, "\u{feff}[1]"), "\u{feff}[\n  1\n]\n");
    assert_eq!(pretty(Language::Json, "  \n "), "");
}

#[test]
fn json_invalid_falls_back() {
    let samples = [
        "{\"a\":1,}",
        "{\"a\":1 // c\n,\"b\":[1,2}",
        "{\"unterminated",
        "[[[[",
        "]]]}}}",
        "{'a':NaN,b:undefined}",
        "{\"a\":01}",
    ];
    for s in samples {
        assert_eq!(strip(&pretty(Language::Json, s)), strip(s), "{s}");
    }
    assert_eq!(
        pretty(Language::Json, "{\"a\":[1,2,}"),
        "{\n  \"a\":[\n    1,\n    2,\n  }\n"
    );
}

#[test]
fn js_golden() {
    let input = "function f(a,b){if(a>b){return a}else{for(let i=0;i<b;i++)g(i);return b?.c??-1}}const o={x:1,y:[1,2],z:`t${a+1}`};x=a?b:c;";
    let expected = "\
function f(a, b) {
  if (a > b) {
    return a
  } else {
    for (let i = 0; i < b; i++) g(i);
    return b?.c ?? -1
  }
}
const o = {
  x: 1,
  y: [1, 2],
  z: `t${a + 1}`
};
x = a ? b : c;
";
    assert_eq!(pretty(Language::JavaScript, input), expected);

    let switch = "switch(x){case 1:a();break;default:b()}";
    assert_eq!(
        pretty(Language::JavaScript, switch),
        "switch (x) {\n  case 1:\n    a();\n    break;\n  default:\n    b()\n}\n"
    );
    // Source line breaks stay (automatic semicolon insertion).
    assert_eq!(
        pretty(Language::JavaScript, "a=b\n(c)\nreturn\nd"),
        "a = b\n(c)\nreturn\nd\n"
    );
    // A license comment keeps its own line.
    assert_eq!(
        pretty(Language::JavaScript, "/*! MIT */var a=1"),
        "/*! MIT */\nvar a = 1\n"
    );
    assert_eq!(
        pretty(Language::JavaScript, "x=a/b/c,y=/[/]/g"),
        "x = a / b / c,\ny = /[/]/g\n"
    );
    // TypeScript generics keep their source spacing.
    assert_eq!(
        pretty(
            Language::TypeScript,
            "let m:Map<string,number[]> =new Map<string,number[]>()"
        ),
        "let m: Map<string, number[]> = new Map<string, number[]>()\n"
    );
}

#[test]
fn js_minified_sample_reads_well() {
    let out = pretty(Language::JavaScript, UMD);
    assert!(out.lines().count() > 150, "{out}");
    assert!(out.lines().all(|l| l.len() < 250), "{out}");
    assert!(out.starts_with("/*! tinylib"));
    assert!(out.contains("\n  class l extends Map {\n"));
    assert!(out.contains("\n    *[Symbol.iterator]() {\n"));
    assert!(out.contains("        if (!t.ok) throw new Error(`HTTP ${t.status}`);\n"));
    assert!(out.ends_with("});\n//# sourceMappingURL=tinylib.min.js.map\n"));
}

#[test]
fn css_golden() {
    let input = "/*! x */a,b:hover>c{color:red;margin:0 auto}@media (max-width:600px){.a{x:y!important}}.n{&:hover{c:d}.m{e:f}}a{background:url(x;y{z}.png)}";
    let expected = "\
/*! x */
a,
b:hover>c {
  color: red;
  margin: 0 auto
}
@media (max-width:600px) {
  .a {
    x: y!important
  }
}
.n {
  &:hover {
    c: d
  }
  .m {
    e: f
  }
}
a {
  background: url(x;y{z}.png)
}
";
    assert_eq!(pretty(Language::Css, input), expected);
    assert_eq!(pretty(Language::Css, expected), expected);
    assert_eq!(
        pretty(Language::Scss, "a{// c\nb:c}"),
        "a {\n  // c\n  b: c\n}\n"
    );
}

#[test]
fn css_reads_well() {
    let out = pretty(Language::Css, CSS);
    assert!(out.contains("\n.btn {\n  display: inline-flex;\n"));
    assert!(
        out.contains(
            "\n@media (max-width:768px) and (prefers-reduced-motion:reduce) {\n  .btn {\n"
        )
    );
    assert!(out.contains("a:hover,\na:focus-visible {\n"));
    assert!(out.contains("  src: url(mona.woff2) format(\"woff2\")\n"));
}

#[test]
fn html_golden() {
    let input = "<!DOCTYPE html><html><head><meta charset=utf-8><title>T</title><style>a{b:c}</style></head>\
<body><!-- c --><div  class=\"a  b\"><p>Hello <b>world</b>!</p><br><img src=x.png alt=\"a > b\"/>\
<ul><li>one<li>two</ul><pre>  keep\n   this </pre><script>if(a){b()}</script></div></body></html>";
    let expected = "\
<!DOCTYPE html>
<html>
  <head>
    <meta charset=utf-8>
    <title>T</title>
    <style>
      a {
        b: c
      }
    </style>
  </head>
  <body>
    <!-- c -->
    <div class=\"a  b\">
      <p>
        Hello
        <b>world</b>
        !
      </p>
      <br>
      <img src=x.png alt=\"a > b\"/>
      <ul>
        <li>
          one
        <li>
          two
      </ul>
      <pre>  keep
   this </pre>
      <script>
        if (a) {
          b()
        }
      </script>
    </div>
  </body>
</html>
";
    assert_eq!(pretty(Language::Html, input), expected);
}

#[test]
fn xml_golden() {
    let input = "<?xml version=\"1.0\"?><!DOCTYPE r [<!ENTITY e \"x>\">]><r xmlns:a=\"u\"><a:b k='1'>text</a:b><c/><d><![CDATA[<raw>]]></d><E></E></r>";
    let expected = "\
<?xml version=\"1.0\"?>
<!DOCTYPE r [<!ENTITY e \"x>\">]>
<r xmlns:a=\"u\">
  <a:b k='1'>text</a:b>
  <c/>
  <d>
    <![CDATA[<raw>]]>
  </d>
  <E></E>
</r>
";
    assert_eq!(pretty(Language::Xml, input), expected);
}

#[test]
fn markup_malformed_keeps_text() {
    let samples = [
        "<a><b></a></b>",
        "</x></y><z>",
        "<div class=\"unterminated",
        "<!-- open comment",
        "a < b && c > d",
        "<script>if(a<b)x()",
        "<p>one<p>two<div>three</span>",
        "<<<>>>",
        "<a href='x'>link</a>tail",
    ];
    for s in samples {
        assert_eq!(strip(&pretty(Language::Html, s)), strip(s), "{s}");
        assert_eq!(strip(&pretty(Language::Xml, s)), strip(s), "{s}");
    }
}

#[test]
fn every_printer_keeps_text() {
    let samples = [
        UMD,
        CSS,
        "{\"a\":[1,{\"b\":\"c d\"}]}",
        "<svg><g><path d=\"M0 0L1 1\"/></g></svg>",
        "\u{feff}héllo wörld {ünïcode: 'ß'}",
        "",
        "   ",
        "{{{{[[[[((((",
        "}}}}]]]]))))",
        "\"\"\"''''``````",
        "/* /* */ */ // //",
    ];
    for s in samples {
        for lang in [
            Language::Json,
            Language::JavaScript,
            Language::TypeScript,
            Language::Css,
            Language::Scss,
            Language::Html,
            Language::Xml,
        ] {
            let out = pretty(lang, s);
            assert_eq!(strip(&out), strip(s), "{lang:?} {s}");
            assert!(out.is_empty() || out.ends_with('\n'));
        }
    }
}

#[test]
fn small_edit_changes_few_lines() {
    let edited = UMD.replace("retries:t=3", "retries:t=5");
    let a = pretty(Language::JavaScript, UMD);
    let b = pretty(Language::JavaScript, &edited);
    let (al, bl): (Vec<_>, Vec<_>) = (a.lines().collect(), b.lines().collect());
    assert_eq!(al.len(), bl.len());
    assert_eq!(al.iter().zip(&bl).filter(|(x, y)| x != y).count(), 1);
}

#[test]
fn minified_detection() {
    assert!(is_minified(Language::JavaScript, UMD));
    assert!(is_minified(Language::Css, CSS));
    let one_line_json = format!(
        "[{}]",
        (0..300)
            .map(|i| format!("{{\"id\":{i},\"name\":\"item {i}\"}}"))
            .collect::<Vec<_>>()
            .join(",")
    );
    assert!(is_minified(Language::Json, &one_line_json));

    // Hand formatted files.
    let pretty_umd = pretty(Language::JavaScript, UMD);
    assert!(!is_minified(Language::JavaScript, &pretty_umd));
    assert!(!is_minified(Language::Css, &pretty(Language::Css, CSS)));
    assert!(!is_minified(
        Language::Json,
        &pretty(Language::Json, &one_line_json)
    ));
    assert!(!is_minified(Language::Json, "{\"a\": 1}"));
    assert!(!is_minified(Language::Json, ""));
    // A normal JSON file with one long string value.
    let mut doc = String::from("{\n");
    for i in 0..30 {
        doc.push_str(&format!("  \"key{i}\": \"value {i}\",\n"));
    }
    doc.push_str(&format!("  \"blob\": \"{}\"\n}}\n", "QUJD".repeat(2000)));
    assert!(!is_minified(Language::Json, &doc));
    // Normal source with a few long lines.
    let mut src = String::new();
    for i in 0..200 {
        src.push_str(&format!(
            "function f{i}(a, b) {{\n  return a + b * {i};\n}}\n"
        ));
    }
    src.push_str(&format!("const table = [{}];\n", "1, ".repeat(400)));
    assert!(!is_minified(Language::JavaScript, &src));
    // License header then one long line is minified.
    let bundle = format!("/*!\n * lib\n * MIT\n */\n{}", "a=b+c;".repeat(400));
    assert!(is_minified(Language::JavaScript, &bundle));
}

fn big_input(target: usize) -> String {
    let body = UMD.lines().nth(1).unwrap_or(UMD);
    let mut s = String::with_capacity(target + body.len());
    while s.len() < target {
        s.push_str(body);
        s.push(';');
    }
    s
}

#[test]
fn speed() {
    // Debug build budget; release is far faster.
    let js = big_input(300 * 1024);
    let start = Instant::now();
    let out = pretty(Language::JavaScript, &js);
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
    assert!(out.len() > js.len());

    let css = CSS.repeat(300 * 1024 / CSS.len());
    let start = Instant::now();
    pretty(Language::Css, &css);
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );

    let json = format!(
        "[{}]",
        vec!["{\"a\":[1,2,{\"b\":\"c\"}],\"d\":null}"; 10_000].join(",")
    );
    let start = Instant::now();
    pretty(Language::Json, &json);
    let html = "<div class=\"a\"><span>x</span><br><p>t</p></div>".repeat(6000);
    pretty(Language::Html, &html);
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn deep_nesting_is_bounded() {
    let deep = "[".repeat(100_000) + &"]".repeat(100_000);
    for lang in [
        Language::Json,
        Language::JavaScript,
        Language::Css,
        Language::Html,
    ] {
        let out = pretty(lang, &deep);
        assert!(out.len() < deep.len() * 100, "{lang:?}");
    }
    let tags = "<a>".repeat(50_000);
    let out = pretty(Language::Xml, &tags);
    assert!(out.len() < tags.len() * 30);
    let templates = "`${".repeat(50_000);
    assert_eq!(strip(&pretty(Language::JavaScript, &templates)), templates);
}

#[test]
fn random_inputs_never_lose_text() {
    let alphabet: Vec<&str> = vec![
        "{",
        "}",
        "[",
        "]",
        "(",
        ")",
        "<",
        ">",
        "/",
        "*",
        "\"",
        "'",
        "`",
        "${",
        "\\",
        ";",
        ",",
        ":",
        "=",
        "?",
        ".",
        "!",
        "-",
        "+",
        "#",
        "@",
        "<!--",
        "-->",
        "<![CDATA[",
        "]]>",
        "<?",
        "?>",
        "</",
        "/>",
        "url(",
        "//",
        "/*",
        "*/",
        "\n",
        " ",
        "\t",
        "a",
        "div",
        "script",
        "pre",
        "0",
        "1e",
        "é",
        "日本",
        "\u{2028}",
        "\u{feff}",
        "return",
        "case",
        "x",
        "for",
        "true",
    ];
    let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
    for _ in 0..1500 {
        let mut s = String::new();
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let len = (seed % 60) as usize;
        for _ in 0..len {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            s.push_str(alphabet[(seed % alphabet.len() as u64) as usize]);
        }
        for lang in [
            Language::Json,
            Language::JavaScript,
            Language::TypeScript,
            Language::Css,
            Language::Less,
            Language::Html,
            Language::Xml,
        ] {
            let out = pretty(lang, &s);
            // U+2028 and U+FEFF are whitespace to the JavaScript tokenizer
            // only; compare without them too.
            let clean = |t: &str| strip(t).replace(['\u{2028}', '\u{feff}'], "");
            assert_eq!(clean(&out), clean(&s), "{lang:?} {s:?}");
        }
    }
}
