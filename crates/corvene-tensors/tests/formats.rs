mod common;

use common::*;
use corvene_models::tensors::{DType, TensorFormat, TensorLocation};
use corvene_tensors::read;

fn meta<'a>(f: &'a corvene_models::TensorFile, key: &str) -> Option<&'a str> {
    f.metadata
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn section<'a>(f: &'a corvene_models::TensorFile, title: &str) -> &'a [(String, String)] {
    f.sections
        .iter()
        .find(|s| s.title == title)
        .map(|s| s.entries.as_slice())
        .unwrap_or(&[])
}

#[test]
fn extensions() {
    for p in [
        "a.safetensors",
        "dir/m.GGUF",
        "x.npy",
        "x.npz",
        "w.pt",
        "w.pth",
        "pytorch_model.bin",
        "m.ckpt",
        "m.onnx",
        "m.tflite",
        "w.h5",
        "w.hdf5",
        "m.keras",
        "m.mlmodel",
        "saved_model.pb",
    ] {
        assert!(corvene_tensors::is_tensor_file(p), "{p}");
    }
    for p in ["a.txt", "safetensors", "x.json", "dir.gguf/readme"] {
        assert!(!corvene_tensors::is_tensor_file(p), "{p}");
    }
}

#[test]
fn safetensors_lists_tensors_in_data_order() {
    let bytes = safetensors(
        &[
            ("z.weight", "F32", &[2, 2], f32s(&[1.0, 2.0, 3.0, 4.0])),
            ("a.bias", "BF16", &[2], bf16s(&[1.0, -1.0])),
        ],
        &[("format", "pt")],
    );
    let f = read("model.safetensors", &bytes).unwrap();
    assert_eq!(f.format, TensorFormat::Safetensors);
    assert_eq!(f.tensors.len(), 2);
    assert_eq!(f.tensors[0].name, "z.weight");
    assert_eq!(f.tensors[0].shape_label(), "[2, 2]");
    assert_eq!(f.tensors[1].dtype, DType::BF16);
    assert_eq!(f.parameters, 6);
    assert_eq!(f.tensor_bytes, 20);
    assert_eq!(f.file_size, bytes.len() as u64);
    assert_eq!(meta(&f, "format"), Some("pt"));
}

#[test]
fn safetensors_rejects_bad_offsets() {
    let mut bytes = safetensors(&[("a", "F32", &[4], f32s(&[1.0; 4]))], &[]);
    bytes.truncate(bytes.len() - 4);
    assert!(read("m.safetensors", &bytes).is_err());
}

#[test]
fn gguf_metadata_and_quant_tensors() {
    let q8 = [q8_0(0.5, &[1; 32]), q8_0(1.0, &[2; 32])].concat();
    let bytes = gguf(&[
        ("token_embd.weight", &[4, 2], 0, f32s(&[0.0; 8])),
        ("blk.0.attn_q.weight", &[32, 2], 8, q8),
        ("blk.0.ffn.weight", &[256], 12, vec![0; 144]),
        ("blk.0.odd", &[256], 16, vec![0; 66]),
    ]);
    let f = read("m.gguf", &bytes).unwrap();
    assert_eq!(f.format, TensorFormat::Gguf);
    assert_eq!(meta(&f, "general.architecture"), Some("llama"));
    assert_eq!(meta(&f, "llama.rope.freq_base"), Some("10000.5"));
    assert_eq!(meta(&f, "general.flag"), Some("true"));
    let tok = section(&f, "Tokenizer");
    assert!(tok[0].1.starts_with("[10 items] \"tok0\""), "{}", tok[0].1);
    assert_eq!(tok[1].1, "[0.5, 1, 1.5]");
    assert_eq!(f.tensors.len(), 4);
    assert_eq!(f.tensors[1].dtype, DType::Quant("Q8_0".into()));
    assert_eq!(f.tensors[1].bytes, 68);
    assert_eq!(f.tensors[2].dtype.name(), "Q4_K");
    assert_eq!(f.tensors[2].bytes, 144);
    assert_eq!(f.tensors[3].dtype.name(), "IQ2_XXS");
    assert!(matches!(
        f.tensors[1].location,
        TensorLocation::Bytes { .. }
    ));
}

#[test]
fn npy_versions_and_orders() {
    let bytes = npy(
        "<f4",
        false,
        &[2, 3],
        &f32s(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
    );
    let f = read("x.npy", &bytes).unwrap();
    assert_eq!(f.tensors[0].shape, vec![2, 3]);
    assert_eq!(f.tensors[0].dtype, DType::F32);

    let be: Vec<u8> = [1.5f64, -2.0]
        .iter()
        .flat_map(|v| v.to_be_bytes())
        .collect();
    let f = read("x.npy", &npy(">f8", true, &[2], &be)).unwrap();
    assert!(f.tensors[0].big_endian);
    assert!(f.tensors[0].column_major);

    let f = read("x.npy", &npy("|b1", false, &[3], &[1, 0, 1])).unwrap();
    assert_eq!(f.tensors[0].dtype, DType::Bool);
}

#[test]
fn npz_stored_and_deflated_members() {
    let a = npy("<f4", false, &[2], &f32s(&[1.0, 2.0]));
    let b = npy(
        "<i8",
        false,
        &[3],
        &[1i64, 2, 3]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>(),
    );
    let bytes = zip(&[("a.npy", &a, false), ("b.npy", &b, true)]);
    let f = read("x.npz", &bytes).unwrap();
    assert_eq!(f.format, TensorFormat::Npz);
    assert_eq!(f.tensors.len(), 2);
    assert_eq!(f.tensors[0].name, "a");
    assert_eq!(f.tensors[0].member, None);
    assert_eq!(f.tensors[1].member.as_deref(), Some("b.npy"));
    assert_eq!(f.tensors[1].dtype, DType::I64);
}

fn torch_bytes(deflate: bool) -> Vec<u8> {
    torch_zip(
        &torch_state_dict_pickle(),
        &[
            ("0", f32s(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0])),
            ("1", f16s(&[0.5, 1.5, -2.0])),
        ],
        deflate,
    )
}

#[test]
fn torch_state_dict() {
    let f = read("pytorch_model.bin", &torch_bytes(false)).unwrap();
    assert_eq!(f.format, TensorFormat::PyTorch);
    let names: Vec<&str> = f.tensors.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["fc.weight", "fc.bias", "fc.weight_t"]);
    assert_eq!(f.tensors[0].shape, vec![2, 3]);
    assert_eq!(f.tensors[1].dtype, DType::F16);
    assert!(matches!(
        f.tensors[0].location,
        TensorLocation::Bytes { len: 24, .. }
    ));
    assert!(matches!(
        f.tensors[2].location,
        TensorLocation::Strided { .. }
    ));
    assert_eq!(meta(&f, "Serialization version"), Some("3"));
    assert!(f.notes.is_empty(), "{:?}", f.notes);
}

#[test]
fn torch_checkpoint_nesting_and_scalars() {
    let mut p = Pickle::new();
    p.op(b"}").op(b"(");
    p.str("epoch").int(3);
    p.str("lr").op(b"G").op(&0.25f64.to_be_bytes());
    p.str("state_dict")
        .op(b"}")
        .str("w")
        .tensor("FloatStorage", "0", 2, 0, &[2], &[1]);
    p.op(b"s");
    p.str("callbacks")
        .global("lightning.pytorch.callbacks", "ModelCheckpoint")
        .op(b")")
        .op(&[0x81]);
    p.op(b"}").str("monitor").str("val_loss").op(b"s").op(b"b");
    p.op(b"u.");
    let bytes = torch_zip(&p.0, &[("0", f32s(&[1.0, 2.0]))], false);
    let f = read("last.ckpt", &bytes).unwrap();
    assert_eq!(f.tensors.len(), 1);
    assert_eq!(f.tensors[0].name, "state_dict.w");
    assert_eq!(meta(&f, "epoch"), Some("3"));
    assert_eq!(meta(&f, "lr"), Some("0.25"));
    assert_eq!(meta(&f, "callbacks.monitor"), Some("val_loss"));
}

#[test]
fn torch_non_archives() {
    assert!(read("pytorch_model.bin", &b"not a zip at all".to_vec()).is_err());
    assert!(read("m.ckpt", &zip(&[("a.txt", b"hi", false)])).is_err());
    let mut legacy = b"\x80\x02\x8a\x0a\x6c\xfc\x9c\x46\xf9\x20\x6a\xa8\x50\x19.".to_vec();
    legacy.extend_from_slice(&[0; 32]);
    let f = read("old.pt", &legacy).unwrap();
    assert_eq!(f.format, TensorFormat::PyTorchLegacy);
    assert!(f.tensors.is_empty());
    assert!(f.notes[0].contains("legacy PyTorch format"));
}

#[test]
fn torch_pickle_never_runs_unknown_globals() {
    let mut p = Pickle::new();
    p.global("os", "system")
        .str("rm -rf /")
        .op(&[0x85])
        .op(b"R.");
    let f = read("evil.pt", &torch_zip(&p.0, &[], false)).unwrap();
    assert!(f.tensors.is_empty());
    assert!(
        f.notes.iter().any(|n| n.contains("os.system")),
        "{:?}",
        f.notes
    );
}

#[test]
fn onnx_model_fields() {
    let f = read("m.onnx", &onnx_model()).unwrap();
    assert_eq!(f.format, TensorFormat::Onnx);
    assert_eq!(meta(&f, "IR version"), Some("8"));
    assert_eq!(meta(&f, "Opset"), Some("ai.onnx 17"));
    assert_eq!(meta(&f, "Producer"), Some("pytorch"));
    assert_eq!(meta(&f, "Graph"), Some("main"));
    assert_eq!(meta(&f, "author"), Some("me"));
    assert_eq!(
        section(&f, "Operators"),
        &[
            ("Relu".to_string(), "×2".to_string()),
            ("Conv".into(), "×1".into())
        ]
    );
    // The initializer listed as an input is dropped.
    assert_eq!(
        section(&f, "Inputs"),
        &[("x".to_string(), "f32 [batch, 3, 224]".to_string())]
    );
    assert_eq!(section(&f, "Outputs")[0].1, "f32");
    assert_eq!(f.tensors.len(), 3);
    assert_eq!(f.tensors[0].shape, vec![2, 3]);
    assert!(matches!(
        f.tensors[0].location,
        TensorLocation::Bytes { len: 24, .. }
    ));
    assert_eq!(f.tensors[1].dtype, DType::I64);
    assert!(matches!(
        f.tensors[1].location,
        TensorLocation::Varints { .. }
    ));
    assert!(
        matches!(&f.tensors[2].location, TensorLocation::External { file, len: 16, .. } if file == "weights.bin")
    );
}

#[test]
fn tflite_model_fields() {
    let f = read("m.tflite", &tflite_model()).unwrap();
    assert_eq!(f.format, TensorFormat::TfLite);
    assert_eq!(meta(&f, "Schema version"), Some("3"));
    assert_eq!(meta(&f, "Description"), Some("converted"));
    assert_eq!(meta(&f, "min_runtime_version"), Some("1.5.0"));
    let names: Vec<&str> = f.tensors.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["weights", "bias"]);
    assert_eq!(f.tensors[1].dtype, DType::I8);
    assert_eq!(
        section(&f, "Inputs"),
        &[("input".to_string(), "f32 [1, 4]".to_string())]
    );
    assert_eq!(section(&f, "Outputs")[0].0, "bias");
    assert_eq!(
        section(&f, "Operators"),
        &[("FULLY_CONNECTED".to_string(), "×2".to_string())]
    );
    assert_eq!(section(&f, "Quantization")[0].1, "scale 0.5, zero point 0");
    assert!(read("m.tflite", &b"\x10\0\0\0XXXXjunkjunk".to_vec()).is_err());
}

#[test]
fn hdf5_keras2_file() {
    let bytes = fixture("keras2.h5");
    let f = read("weights.h5", &bytes).unwrap();
    assert_eq!(f.format, TensorFormat::Hdf5);
    assert!(f.notes.is_empty(), "{:?}", f.notes);
    assert_eq!(meta(&f, "keras_version"), Some("2.15.0"));
    assert_eq!(meta(&f, "backend"), Some("tensorflow"));
    assert_eq!(meta(&f, "note"), Some("variable length text"));
    assert_eq!(meta(&f, "epochs"), Some("12"));
    assert_eq!(meta(&f, "lr"), Some("0.5"));
    let attrs = section(&f, "Attributes");
    assert!(
        attrs.contains(&(
            "/model_weights @layer_names".to_string(),
            "[dense, dense_1]".to_string()
        )),
        "{attrs:?}"
    );
    let t = |n: &str| {
        f.tensors
            .iter()
            .find(|t| t.name == n)
            .unwrap_or_else(|| panic!("{n} in {:?}", f.tensors))
    };
    let k = t("/model_weights/dense/dense/kernel:0");
    assert_eq!(k.shape, vec![4, 3]);
    assert_eq!(k.dtype, DType::F32);
    let c = t("/model_weights/dense_1/dense_1/kernel:0");
    assert!(
        matches!(&c.location, TensorLocation::Chunked { filters, chunks, .. } if filters == &vec![2, 1] && chunks.len() == 9)
    );
    assert_eq!(t("/ints").dtype, DType::I16);
    assert_eq!(t("/flags").dtype, DType::Bool);
    assert!(t("/big").big_endian);
    assert_eq!(t("/half").dtype, DType::F16);
    assert_eq!(t("/empty").elements, 0);
}

#[test]
fn hdf5_latest_file_format() {
    let bytes = fixture("latest.h5");
    let f = read("w.hdf5", &bytes).unwrap();
    assert!(f.notes.is_empty(), "{:?}", f.notes);
    assert_eq!(meta(&f, "Superblock version"), Some("3"));
    let many: Vec<&str> = f
        .tensors
        .iter()
        .filter(|t| t.name.starts_with("/many/"))
        .map(|t| t.name.as_str())
        .collect();
    assert_eq!(many.len(), 20, "{many:?}");
    let attrs = section(&f, "Attributes");
    assert_eq!(
        attrs
            .iter()
            .filter(|(k, _)| k.starts_with("/many @a"))
            .count(),
        12,
        "{attrs:?}"
    );
    let chunked = f.tensors.iter().find(|t| t.name == "/chunked").unwrap();
    assert!(
        matches!(&chunked.location, TensorLocation::Chunked { chunks, .. } if chunks.len() == 4)
    );
}

#[test]
fn keras3_zip() {
    let h5 = fixture("keras2.h5");
    let meta_json = br#"{"keras_version": "3.3.3", "date_saved": "2024-05-01@10:00:00"}"#;
    let config = br#"{"class_name": "Sequential", "config": {"name": "seq", "layers": [{}, {}]}}"#;
    for deflate in [false, true] {
        let bytes = zip(&[
            ("metadata.json", meta_json, false),
            ("config.json", config, true),
            ("model.weights.h5", &h5, deflate),
        ]);
        let f = read("model.keras", &bytes).unwrap();
        assert_eq!(f.format, TensorFormat::Keras);
        assert_eq!(meta(&f, "keras_version"), Some("3.3.3"));
        assert_eq!(meta(&f, "Model class"), Some("Sequential"));
        assert_eq!(meta(&f, "Layers"), Some("2"));
        assert_eq!(f.tensors.len(), 8);
        assert_eq!(f.tensors[0].member.is_some(), deflate);
    }
}

#[test]
fn coreml_model_fields() {
    let f = read("m.mlmodel", &coreml_model()).unwrap();
    assert_eq!(f.format, TensorFormat::CoreMl);
    assert_eq!(meta(&f, "Specification version"), Some("4"));
    assert_eq!(meta(&f, "Author"), Some("me"));
    assert_eq!(meta(&f, "k"), Some("v"));
    assert_eq!(
        section(&f, "Inputs"),
        &[("x".to_string(), "multiarray".to_string())]
    );
    assert_eq!(
        section(&f, "Layers"),
        &[("innerProduct".to_string(), "×1".to_string())]
    );
    let names: Vec<&str> = f.tensors.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["fc.weights", "fc.bias"]);
    assert_eq!(f.tensors[1].dtype, DType::F16);
}

#[test]
fn tf_graph_consts() {
    let f = read("frozen.pb", &tf_graph()).unwrap();
    assert_eq!(f.format, TensorFormat::TfGraph);
    assert_eq!(f.tensors.len(), 2);
    assert_eq!(f.tensors[0].shape, vec![2, 2]);
    assert!(matches!(
        f.tensors[0].location,
        TensorLocation::Bytes { len: 16, .. }
    ));
    assert_eq!(f.tensors[1].location, TensorLocation::Missing);
    assert_eq!(meta(&f, "Producer"), Some("1000"));
    assert_eq!(section(&f, "Operators").len(), 3);
}

#[test]
fn align_rows() {
    use corvene_models::tensors::{TensorChange, align, align_metadata};
    let old = safetensors(
        &[
            ("keep", "F32", &[2], f32s(&[1.0, 2.0])),
            ("gone", "F32", &[1], f32s(&[1.0])),
            ("reshape", "F32", &[2], f32s(&[1.0, 2.0])),
            ("old_name", "F16", &[3], f16s(&[1.0, 2.0, 3.0])),
        ],
        &[("a", "1"), ("b", "2")],
    );
    let new = safetensors(
        &[
            ("keep", "F32", &[2], f32s(&[1.0, 2.5])),
            ("added", "BF16", &[4], bf16s(&[0.0; 4])),
            ("reshape", "F32", &[1, 2], f32s(&[1.0, 2.0])),
            ("new_name", "F16", &[3], f16s(&[1.0, 2.0, 3.0])),
        ],
        &[("a", "1"), ("c", "3")],
    );
    let o = read("a.safetensors", &old).unwrap();
    let n = read("a.safetensors", &new).unwrap();
    let rows = align(Some(&o), Some(&n));
    let got: Vec<(&str, TensorChange)> = rows.iter().map(|r| (r.name(), r.change)).collect();
    assert_eq!(
        got,
        [
            ("keep", TensorChange::Unchanged),
            ("added", TensorChange::Added),
            ("gone", TensorChange::Removed),
            ("reshape", TensorChange::Changed),
            ("new_name", TensorChange::Renamed),
        ]
    );
    assert!(rows[3].shape_changed && !rows[3].dtype_changed);
    let only_new = align(None, Some(&n));
    assert!(only_new.iter().all(|r| r.change == TensorChange::Added));

    let meta = align_metadata(Some(&o), Some(&n));
    let m: Vec<(&str, Option<&str>, Option<&str>)> = meta
        .iter()
        .map(|r| (r.key.as_str(), r.old.as_deref(), r.new.as_deref()))
        .collect();
    assert_eq!(
        m,
        [
            ("a", Some("1"), Some("1")),
            ("c", None, Some("3")),
            ("b", Some("2"), None)
        ]
    );
    assert!(n.weight() > 0);
}
