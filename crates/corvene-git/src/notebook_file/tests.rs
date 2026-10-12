use corvene_models::{Diff, FileStatusKind, NbCellKind, NbData, NbOutputKind};

use super::json_spans::{cell_spans, line_ranges};
use super::*;

/// nbformat 4.5 as Jupyter writes it (one-space indent, sources as lines).
const V45: &str = r##"{
 "cells": [
  {
   "cell_type": "markdown",
   "id": "a1",
   "metadata": {},
   "source": [
    "# Title é ☃ 😀 é\n",
    "![plot](img/plot.png) and <img src=\"logo.svg\" width=10>"
   ]
  },
  {
   "cell_type": "code",
   "execution_count": 1,
   "id": "b2",
   "metadata": {
    "tags": [
     "x"
    ],
    "collapsed": false
   },
   "outputs": [
    {
     "name": "stdout",
     "output_type": "stream",
     "text": [
      "hello\n"
     ]
    },
    {
     "name": "stdout",
     "output_type": "stream",
     "text": "world\n"
    },
    {
     "name": "stderr",
     "output_type": "stream",
     "text": "warn\n"
    },
    {
     "data": {
      "text/plain": [
       "<Figure>"
      ],
      "image/png": "iVBORw0KGgo\nAAAA\n"
     },
     "metadata": {
      "needs_background": "light"
     },
     "output_type": "display_data"
    },
    {
     "data": {
      "text/plain": "3",
      "application/json": {"b": 1, "a": [1, 2]}
     },
     "execution_count": 1,
     "metadata": {},
     "output_type": "execute_result"
    },
    {
     "ename": "ValueError",
     "evalue": "bad",
     "output_type": "error",
     "traceback": [
      "line one",
      "line two"
     ]
    }
   ],
   "source": [
    "print('hello')\n",
    "x = \"a \\\" { ]\""
   ]
  },
  {
   "attachments": {
    "z.png": {
     "image/png": "AAAA"
    },
    "a.png": {
     "image/png": "BBBB"
    }
   },
   "cell_type": "raw",
   "id": "c3",
   "metadata": {},
   "source": "raw text"
  }
 ],
 "metadata": {
  "kernelspec": {
   "display_name": "Python 3 (ipykernel)",
   "language": "python",
   "name": "python3"
  },
  "language_info": {
   "name": "python",
   "file_extension": ".py"
  }
 },
 "nbformat": 4,
 "nbformat_minor": 5
}
"##;

/// nbformat 4.4: no cell ids, a Julia kernel without a file extension.
const V44: &str = r##"{
 "cells": [
  {
   "cell_type": "code",
   "execution_count": null,
   "metadata": {},
   "outputs": [],
   "source": [
    "println(1)"
   ]
  },
  {
   "cell_type": "unknown",
   "metadata": {},
   "source": []
  }
 ],
 "metadata": {
  "kernelspec": {
   "display_name": "Julia 1.10",
   "language": "julia",
   "name": "julia-1.10"
  }
 },
 "nbformat": 4,
 "nbformat_minor": 4
}
"##;

/// nbformat 3 with a heading cell and v3 outputs.
const V3: &str = r##"{
 "metadata": {
  "name": "old"
 },
 "nbformat": 3,
 "nbformat_minor": 0,
 "worksheets": [
  {
   "cells": [
    {
     "cell_type": "heading",
     "level": 2,
     "metadata": {},
     "source": [
      "Intro"
     ]
    },
    {
     "cell_type": "code",
     "collapsed": false,
     "input": [
      "1 + 1"
     ],
     "language": "python",
     "metadata": {},
     "outputs": [
      {
       "metadata": {},
       "output_type": "pyout",
       "prompt_number": 3,
       "text": [
        "2"
       ],
       "png": "AA\nAA"
      },
      {
       "output_type": "stream",
       "stream": "stdout",
       "text": [
        "hi\n"
       ]
      },
      {
       "ename": "NameError",
       "evalue": "x",
       "output_type": "pyerr",
       "traceback": [
        "tb"
       ]
      }
     ],
     "prompt_number": 3
    }
   ],
   "metadata": {}
  }
 ]
}
"##;

/// Every cell's line range starts on its `{` and ends on its `}` / `},`.
fn assert_spans(text: &str, blob: &NotebookBlob) {
    let lines: Vec<&str> = text.split('\n').collect();
    for cell in &blob.cells {
        let (first, last) = cell.lines;
        assert!(first >= 1 && first <= last, "{:?}", cell.lines);
        assert_eq!(lines[first as usize - 1].trim(), "{", "{:?}", cell.lines);
        let end = lines[last as usize - 1].trim();
        assert!(end == "}" || end == "},", "{end:?} at {:?}", cell.lines);
    }
}

#[test]
fn reads_nbformat_4_5() {
    let blob = read(V45.as_bytes()).unwrap();
    assert_eq!(blob.format, (4, 5));
    assert_eq!(blob.language, "python");
    assert_eq!(blob.file_extension, ".py");
    assert_eq!(blob.kernel.as_deref(), Some("Python 3 (ipykernel)"));
    assert_eq!(&*blob.raw, V45);
    let keys: Vec<&str> = blob.metadata.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, ["kernelspec", "language_info"]);
    assert_eq!(blob.cells.len(), 3);

    let md = &blob.cells[0];
    assert_eq!(md.kind, NbCellKind::Markdown);
    assert_eq!(md.id.as_deref(), Some("a1"));
    assert!(md.source.starts_with("# Title é ☃ 😀 é\n![plot]"));
    assert_eq!(md.lines, (3, 11));

    let code = &blob.cells[1];
    assert_eq!(code.kind, NbCellKind::Code);
    assert_eq!(code.id.as_deref(), Some("b2"));
    assert_eq!(code.execution_count, Some(1));
    assert_eq!(code.source, "print('hello')\nx = \"a \\\" { ]\"");
    assert_eq!(code.lines, (12, 75));
    // metadata sorted by key, values pretty-printed
    assert_eq!(
        code.metadata,
        [
            ("collapsed".to_string(), "false".to_string()),
            ("tags".to_string(), "[\n  \"x\"\n]".to_string()),
        ]
    );

    let raw = &blob.cells[2];
    assert_eq!(raw.kind, NbCellKind::Raw);
    assert_eq!(raw.source, "raw text");
    assert_eq!(raw.lines, (76, 89));
    let names: Vec<&str> = raw.attachments.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["a.png", "z.png"]);
    assert_eq!(
        raw.attachments[0].data[0].data,
        NbData::Base64("BBBB".into())
    );

    assert_spans(V45, &blob);
}

#[test]
fn reads_every_output_kind() {
    let blob = read(V45.as_bytes()).unwrap();
    let outputs = &blob.cells[1].outputs;
    let kinds: Vec<NbOutputKind> = outputs.iter().map(|o| o.kind).collect();
    assert_eq!(
        kinds,
        [
            NbOutputKind::Stream,
            NbOutputKind::Stream,
            NbOutputKind::DisplayData,
            NbOutputKind::ExecuteResult,
            NbOutputKind::Error,
        ]
    );
    // consecutive stdout streams merged, stderr kept apart
    assert_eq!(outputs[0].name, "stdout");
    assert_eq!(outputs[0].text, "hello\nworld\n");
    assert_eq!(outputs[1].name, "stderr");
    assert_eq!(outputs[1].text, "warn\n");

    let display = &outputs[2];
    assert_eq!(display.data[0].mime, "text/plain");
    assert_eq!(display.data[0].data, NbData::Text("<Figure>".into()));
    assert_eq!(display.data[1].mime, "image/png");
    assert_eq!(
        display.data[1].data,
        NbData::Base64("iVBORw0KGgoAAAA".into())
    );
    assert_eq!(
        display.metadata,
        [("needs_background".to_string(), "\"light\"".to_string())]
    );

    let result = &outputs[3];
    assert_eq!(result.execution_count, Some(1));
    // file order kept, JSON pretty with sorted keys
    assert_eq!(result.data[0].mime, "text/plain");
    assert_eq!(result.data[1].mime, "application/json");
    assert_eq!(
        result.data[1].data,
        NbData::Json("{\n  \"a\": [\n    1,\n    2\n  ],\n  \"b\": 1\n}".into())
    );

    let error = &outputs[4];
    assert_eq!(error.name, "ValueError");
    assert_eq!(error.evalue, "bad");
    assert_eq!(error.text, "line one\nline two");
}

#[test]
fn reads_nbformat_4_4() {
    let blob = read(V44.as_bytes()).unwrap();
    assert_eq!(blob.format, (4, 4));
    assert_eq!(blob.language, "julia");
    assert_eq!(blob.file_extension, ".jl");
    assert_eq!(blob.kernel.as_deref(), Some("Julia 1.10"));
    assert_eq!(blob.cells[0].id, None);
    assert_eq!(blob.cells[0].execution_count, None);
    assert_eq!(blob.cells[0].source, "println(1)");
    assert_eq!(blob.cells[0].lines, (3, 11));
    assert_eq!(blob.cells[1].kind, NbCellKind::Raw);
    assert_eq!(blob.cells[1].source, "");
    assert_eq!(blob.cells[1].lines, (12, 16));
    assert_spans(V44, &blob);
}

#[test]
fn upgrades_nbformat_3() {
    let blob = read(V3.as_bytes()).unwrap();
    assert_eq!(blob.format, (3, 0));
    assert_eq!(blob.language, "python");
    assert_eq!(blob.cells.len(), 2);
    assert_eq!(blob.cells[0].kind, NbCellKind::Markdown);
    assert_eq!(blob.cells[0].source, "## Intro");
    assert_eq!(blob.cells[0].lines, (10, 17));
    let code = &blob.cells[1];
    assert_eq!(code.kind, NbCellKind::Code);
    assert_eq!(code.source, "1 + 1");
    assert_eq!(code.execution_count, Some(3));
    assert_eq!(code.lines, (18, 53));
    let out = &code.outputs;
    assert_eq!(out[0].kind, NbOutputKind::ExecuteResult);
    assert_eq!(out[0].execution_count, Some(3));
    let mimes: Vec<&str> = out[0].data.iter().map(|m| m.mime.as_str()).collect();
    assert_eq!(mimes, ["text/plain", "image/png"]);
    assert_eq!(out[0].data[1].data, NbData::Base64("AAAA".into()));
    assert_eq!(out[1].kind, NbOutputKind::Stream);
    assert_eq!(out[1].name, "stdout");
    assert_eq!(out[1].text, "hi\n");
    assert_eq!(out[2].kind, NbOutputKind::Error);
    assert_eq!(out[2].name, "NameError");
    assert_eq!(out[2].text, "tb");
    assert_spans(V3, &blob);
}

#[test]
fn byte_order_mark_and_crlf_read() {
    let crlf = V44.replace('\n', "\r\n");
    let mut bytes = "\u{feff}".as_bytes().to_vec();
    bytes.extend_from_slice(crlf.as_bytes());
    let blob = read(&bytes).unwrap();
    assert_eq!(blob.cells[0].lines, (3, 11));
}

#[test]
fn malformed_notebooks_do_not_read() {
    assert!(read(br#"{"cells": [ {"cell_type": "code"} "#).is_err());
    assert!(read(b"[1, 2]").is_err());
    assert!(read(br#"{"nbformat": 4}"#).is_err());
    assert!(read(br#"{"cells": 3, "nbformat": 4}"#).is_err());
    assert!(read(b"\xff\xfe{}").is_err());
}

#[test]
fn scanner_skips_tricky_strings() {
    let text = r#"{"a": "x \" { ] }", "cells": [ {"s": "}\\", "t": "\u007b"}, [1, {"b": "]"}], "q" ],
        "z": {"cells": [1]}}"#;
    let spans = cell_spans(text).unwrap();
    let cells = spans.cells.unwrap();
    assert_eq!(cells.len(), 3);
    assert_eq!(
        &text[cells[0].0..cells[0].1],
        r#"{"s": "}\\", "t": "\u007b"}"#
    );
    assert_eq!(&text[cells[1].0..cells[1].1], r#"[1, {"b": "]"}]"#);
    assert_eq!(&text[cells[2].0..cells[2].1], r#""q""#);
    assert_eq!(spans.worksheet_cells, None);
    assert_eq!(line_ranges(text, &cells), [(1, 1), (1, 1), (1, 1)]);
    // the last of a repeated key counts, as serde_json reads it
    let spans = cell_spans(r#"{"cells": [1], "cells": [1, 2]}"#).unwrap();
    assert_eq!(spans.cells.map(|c| c.len()), Some(2));
    assert!(cell_spans(r#"{"cells": ["unterminated]}"#).is_err());
    assert!(cell_spans(r#"{"cells": []} x"#).is_err());
}

#[test]
fn finds_image_links() {
    let md = "![a](x.png \"title\") ![b](<y z.png>)\n<IMG SRC='q.gif'> <img alt=1 src=u.png>\n\
              ![c](http://x/y.png) <imgx src=\"no.png\">";
    assert_eq!(
        image_links(md),
        ["x.png", "y z.png", "http://x/y.png", "q.gif", "u.png"]
    );
}

#[test]
fn notebook_diff_reads_both_sides_and_linked_images() {
    set_read_notebooks(true);
    let old = V45.replace("print('hello')", "print('bye')");
    let diff = notebook_diff(
        "nb/a.ipynb",
        FileStatusKind::Modified,
        || Some(V45.as_bytes().to_vec()),
        || Some(old.into_bytes()),
        Diff::Empty,
        |side, rel| match (side, rel) {
            (Side::Current, "nb/img/plot.png") => Some(b"\x89PNG".to_vec()),
            (_, "nb/logo.svg") => Some(b"<svg/>".to_vec()),
            _ => None,
        },
    );
    let Some(Diff::Notebook {
        previous: Some(previous),
        current: Some(current),
        source,
    }) = diff
    else {
        panic!("{diff:?}");
    };
    assert!(matches!(*source, Diff::Empty));
    assert!(previous.cells[1].source.contains("bye"));
    let linked: Vec<(&str, &str)> = current
        .linked_images
        .iter()
        .map(|(p, i)| (p.as_str(), i.media_type.as_str()))
        .collect();
    assert_eq!(
        linked,
        [("img/plot.png", "image/png"), ("logo.svg", "image/svg+xml")]
    );
    assert_eq!(previous.linked_images.len(), 1);

    // a new file has no previous side; a side that does not read keeps git's diff
    let diff = notebook_diff(
        "a.ipynb",
        FileStatusKind::New,
        || Some(V44.as_bytes().to_vec()),
        || panic!("not read"),
        Diff::Empty,
        |_, _| None,
    );
    assert!(matches!(
        diff,
        Some(Diff::Notebook {
            previous: None,
            current: Some(_),
            ..
        })
    ));
    let diff = notebook_diff(
        "a.ipynb",
        FileStatusKind::Modified,
        || Some(b"{".to_vec()),
        || Some(V44.as_bytes().to_vec()),
        Diff::Empty,
        |_, _| None,
    );
    assert!(diff.is_none());
    assert!(
        notebook_diff(
            "a.py",
            FileStatusKind::Modified,
            || None,
            || None,
            Diff::Empty,
            |_, _| None
        )
        .is_none()
    );
}
