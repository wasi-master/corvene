#![allow(clippy::ptr_arg)] // `&Vec<u8>` coerces to `&dyn Source`; `&[u8]` does not.
mod common;

use std::sync::atomic::AtomicBool;

use common::*;
use corvene_models::TensorFile;
use corvene_models::tensors::TensorValues;
use corvene_tensors::{Source, compare_values, read};

fn one(path: &str, bytes: &Vec<u8>, name: &str) -> TensorValues {
    let f = read(path, bytes).unwrap();
    let v = compare_values(
        None,
        Some((&f, bytes)),
        &[name.to_string()],
        u64::MAX,
        &AtomicBool::new(false),
    );
    v.into_iter().next().unwrap()
}

fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-6, "{a} != {b}");
}

#[test]
fn stats_of_float_types() {
    let bytes = safetensors(
        &[
            ("f32", "F32", &[4], f32s(&[1.0, 2.0, 3.0, 0.0])),
            ("f16", "F16", &[3], f16s(&[0.5, -1.5, 2.0])),
            ("bf16", "BF16", &[2], bf16s(&[1.0, -3.0])),
            ("nan", "F32", &[3], f32s(&[f32::NAN, f32::INFINITY, 1.0])),
            ("e4m3", "F8_E4M3", &[3], vec![0x38, 0xb8, 0x00]),
        ],
        &[],
    );
    let v = one("m.safetensors", &bytes, "f32");
    let s = v.new.unwrap();
    assert_eq!((s.count, s.zeros), (4, 1));
    close(s.min, 0.0);
    close(s.max, 3.0);
    close(s.mean, 1.5);
    close(s.std, 1.25f64.sqrt());
    assert_eq!(s.histogram.iter().sum::<u64>(), 4);
    assert_eq!(s.histogram.len(), 16);

    let s = one("m.safetensors", &bytes, "f16").new.unwrap();
    close(s.min, -1.5);
    close(s.mean, 1.0 / 3.0);
    let s = one("m.safetensors", &bytes, "bf16").new.unwrap();
    close(s.mean, -1.0);
    let s = one("m.safetensors", &bytes, "nan").new.unwrap();
    assert_eq!((s.nan, s.inf, s.count), (1, 1, 3));
    close(s.mean, 1.0);
    let s = one("m.safetensors", &bytes, "e4m3").new.unwrap();
    close(s.max, 1.0);
    close(s.min, -1.0);
}

#[test]
fn compare_two_versions() {
    let old = safetensors(
        &[
            ("w", "F32", &[4], f32s(&[1.0, 2.0, 3.0, 4.0])),
            ("same", "F16", &[2], f16s(&[1.0, 2.0])),
        ],
        &[],
    );
    let new = safetensors(
        &[
            ("w", "F32", &[2, 2], f32s(&[1.0, 2.5, 3.0, 2.0])),
            ("same", "F16", &[2], f16s(&[1.0, 2.0])),
        ],
        &[],
    );
    let o = read("m.safetensors", &old).unwrap();
    let n = read("m.safetensors", &new).unwrap();
    let v = compare_values(
        Some((&o, &old)),
        Some((&n, &new)),
        &[],
        u64::MAX,
        &AtomicBool::new(false),
    );
    assert_eq!(v.len(), 2);
    let w = &v[0];
    assert_eq!(w.name, "w");
    let d = w.diff.as_ref().unwrap();
    assert_eq!(d.changed, 2);
    assert!(!d.identical);
    close(d.max_abs_diff, 2.0);
    close(d.mean_abs_diff, 2.5 / 4.0);
    close(d.rms_diff, (4.25f64 / 4.0).sqrt());
    assert_eq!(d.samples[0].index, 3);
    assert_eq!(d.samples[1].index, 1);
    let cos = (1.0 + 5.0 + 9.0 + 8.0) / (30f64.sqrt() * (1.0f64 + 6.25 + 9.0 + 4.0).sqrt());
    close(d.cosine.unwrap(), cos);
    assert!(w.note.as_deref().unwrap().contains("shapes differ"));
    assert!(v[1].diff.as_ref().unwrap().identical);
}

#[test]
fn added_and_removed_tensors_have_one_side() {
    let old = safetensors(&[("a", "F32", &[1], f32s(&[1.0]))], &[]);
    let new = safetensors(
        &[(
            "b",
            "I32",
            &[2],
            [5i32, 7].iter().flat_map(|v| v.to_le_bytes()).collect(),
        )],
        &[],
    );
    let o = read("m.safetensors", &old).unwrap();
    let n = read("m.safetensors", &new).unwrap();
    let v = compare_values(
        Some((&o, &old)),
        Some((&n, &new)),
        &[],
        u64::MAX,
        &AtomicBool::new(false),
    );
    let b = v.iter().find(|v| v.name == "b").unwrap();
    assert!(b.old.is_none() && b.diff.is_none());
    close(b.new.as_ref().unwrap().mean, 6.0);
    let a = v.iter().find(|v| v.name == "a").unwrap();
    assert!(a.new.is_none() && a.old.is_some());
    // A deleted file: only the old side.
    let v = compare_values(
        Some((&o, &old)),
        None,
        &[],
        u64::MAX,
        &AtomicBool::new(false),
    );
    assert_eq!(v.len(), 1);
}

#[test]
fn budget_and_cancel() {
    let bytes = safetensors(
        &[
            ("a", "F32", &[4], f32s(&[1.0; 4])),
            ("b", "F32", &[4], f32s(&[2.0; 4])),
            ("c", "F32", &[1], f32s(&[3.0])),
        ],
        &[],
    );
    let f = read("m.safetensors", &bytes).unwrap();
    let v = compare_values(None, Some((&f, &bytes)), &[], 20, &AtomicBool::new(false));
    assert!(v[0].new.is_some());
    assert!(v[1].new.is_none());
    assert!(
        v[1].note.as_deref().unwrap().contains("read budget"),
        "{:?}",
        v[1].note
    );
    assert!(v[2].new.is_some(), "smaller tensors still fit");

    let v = compare_values(
        None,
        Some((&f, &bytes)),
        &[],
        u64::MAX,
        &AtomicBool::new(true),
    );
    assert!(v.is_empty());
}

#[test]
fn big_tensor_streams_in_chunks() {
    let n = 3_000_000usize;
    let data: Vec<f32> = (0..n).map(|i| (i % 7) as f32).collect();
    let mut changed = data.clone();
    changed[n - 1] = 100.0;
    let old = safetensors(&[("w", "F32", &[n as u64], f32s(&data))], &[]);
    let new = safetensors(&[("w", "F32", &[n as u64], f32s(&changed))], &[]);
    let o = read("m.safetensors", &old).unwrap();
    let nf = read("m.safetensors", &new).unwrap();
    let v = compare_values(
        Some((&o, &old)),
        Some((&nf, &new)),
        &[],
        u64::MAX,
        &AtomicBool::new(false),
    );
    let d = v[0].diff.as_ref().unwrap();
    assert_eq!(d.changed, 1);
    assert_eq!(d.samples[0].index, n as u64 - 1);
    assert_eq!(v[0].new.as_ref().unwrap().count, n as u64);
}

#[test]
fn gguf_dequantization() {
    // Q8_0: d = 0.5, quants -16..16.
    let mut q = [0i8; 32];
    for (i, v) in q.iter_mut().enumerate() {
        *v = i as i8 - 16;
    }
    // Q4_0: d = 1, low nibbles j, high nibbles 15 - j.
    let mut qs = [0u8; 16];
    for (j, v) in qs.iter_mut().enumerate() {
        *v = j as u8 | ((15 - j as u8) << 4);
    }
    // Q4_K: d = 1, dmin = 0, every scale 1, nibbles 1 (low) and 2 (high).
    let mut q4k = half::f16::from_f32(1.0).to_le_bytes().to_vec();
    q4k.extend_from_slice(&half::f16::from_f32(0.0).to_le_bytes());
    q4k.extend_from_slice(&[1, 1, 1, 1, 0, 0, 0, 0, 1, 1, 1, 1]);
    q4k.extend_from_slice(&[0x21; 128]);
    // Q6_K: all quants zero, scales 1, d = 1: every value -32.
    let mut q6k = vec![0u8; 192];
    q6k.extend_from_slice(&[1; 16]);
    q6k.extend_from_slice(&half::f16::from_f32(1.0).to_le_bytes());
    let bytes = gguf(&[
        ("q8", &[32], 8, q8_0(0.5, &q)),
        ("q4", &[32], 2, q4_0(1.0, &qs)),
        ("q4k", &[256], 12, q4k),
        ("q6k", &[256], 14, q6k),
        ("q2k", &[256], 10, vec![7; 84]),
        ("q3k", &[256], 11, vec![9; 110]),
        ("q5k", &[256], 13, vec![3; 176]),
        ("iq2", &[256], 16, vec![0; 66]),
        ("f16", &[2], 1, f16s(&[1.0, 3.0])),
    ]);
    let s = one("m.gguf", &bytes, "q8").new.unwrap();
    close(s.min, -8.0);
    close(s.max, 7.5);
    assert_eq!(s.count, 32);
    let s = one("m.gguf", &bytes, "q4").new.unwrap();
    close(s.min, -8.0);
    close(s.max, 7.0);
    close(s.mean, -0.5);
    let s = one("m.gguf", &bytes, "q4k").new.unwrap();
    close(s.mean, 1.5);
    assert_eq!(s.count, 256);
    let s = one("m.gguf", &bytes, "q6k").new.unwrap();
    close(s.min, -32.0);
    close(s.max, -32.0);
    for name in ["q2k", "q3k", "q5k"] {
        assert_eq!(
            one("m.gguf", &bytes, name).new.unwrap().count,
            256,
            "{name}"
        );
    }

    let v = one("m.gguf", &bytes, "iq2");
    assert!(v.new.is_none());
    assert!(v.note.unwrap().contains("values not decoded for IQ2_XXS"));
    close(one("m.gguf", &bytes, "f16").new.unwrap().mean, 2.0);

    let h = |v: f32| half::f16::from_f32(v).to_le_bytes().to_vec();
    // Q5_0: d = 1, high bits set for the first 16 elements: 0 x16, -16 x16.
    let q5_0 = [h(1.0), 0x0000_ffffu32.to_le_bytes().to_vec(), vec![0; 16]].concat();
    // Q5_1: d = 1, m = 1, nibbles 15: every value 16.
    let q5_1 = [h(1.0), h(1.0), vec![0; 4], vec![0xff; 16]].concat();
    // Q2_K: scale 1 and min 1 everywhere, quants 3: every value 2.
    let q2k = [vec![0x11; 16], vec![0xff; 64], h(1.0), h(1.0)].concat();
    // Q3_K: no high-bit subtraction, quants 3, 6-bit scales 33: every value 3.
    let q3k = [
        vec![0xff; 32],
        vec![0xff; 64],
        vec![0x11; 8],
        vec![0xaa; 4],
        h(1.0),
    ]
    .concat();
    // Q5_K: scales 1, mins 0, high bits set, nibbles 0: every value 16.
    let q5k = [
        h(1.0),
        h(0.0),
        vec![1, 1, 1, 1, 0, 0, 0, 0, 1, 1, 1, 1],
        vec![0xff; 32],
        vec![0; 128],
    ]
    .concat();
    let more = gguf(&[
        ("q5_0", &[32], 6, q5_0),
        ("q5_1", &[32], 7, q5_1),
        ("q2k", &[256], 10, q2k),
        ("q3k", &[256], 11, q3k),
        ("q5k", &[256], 13, q5k),
    ]);
    let s = one("m.gguf", &more, "q5_0").new.unwrap();
    close(s.mean, -8.0);
    assert_eq!(s.zeros, 16);
    for (name, want) in [("q5_1", 16.0), ("q2k", 2.0), ("q3k", 3.0), ("q5k", 16.0)] {
        let s = one("m.gguf", &more, name).new.unwrap();
        close(s.min, want);
        close(s.max, want);
    }
}

#[test]
fn numpy_values() {
    let be: Vec<u8> = [1.5f64, -2.0]
        .iter()
        .flat_map(|v| v.to_be_bytes())
        .collect();
    let bytes = npy(">f8", false, &[2], &be);
    let s = one("x.npy", &bytes, "array").new.unwrap();
    close(s.min, -2.0);
    close(s.max, 1.5);

    let a = npy("<f4", false, &[3], &f32s(&[1.0, 2.0, 3.0]));
    let z = zip(&[("big.npy", &a, true)]);
    let s = one("x.npz", &z, "big").new.unwrap();
    close(s.mean, 2.0);
}

#[test]
fn torch_values_including_strided_views() {
    for deflate in [false, true] {
        let bytes = torch_zip(
            &torch_state_dict_pickle(),
            &[
                ("0", f32s(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0])),
                ("1", f16s(&[0.5, 1.5, -2.0])),
            ],
            deflate,
        );
        let f = read("m.pt", &bytes).unwrap();
        let v = compare_values(
            Some((&f, &bytes)),
            Some((&f, &bytes)),
            &[],
            u64::MAX,
            &AtomicBool::new(false),
        );
        assert_eq!(v.len(), 3);
        close(v[0].new.as_ref().unwrap().mean, 3.5);
        close(v[1].new.as_ref().unwrap().min, -2.0);
        assert!(
            v.iter()
                .all(|t| t.diff.as_ref().is_some_and(|d| d.identical)),
            "{v:?}"
        );
        // weight_t is the transpose: [1, 4, 2, 5, 3, 6] in order.
        let w = &v[0];
        let t = &v[2];
        close(t.new.as_ref().unwrap().mean, w.new.as_ref().unwrap().mean);
    }
    // Element order of the transposed view.
    let bytes = torch_zip(
        &torch_state_dict_pickle(),
        &[
            ("0", f32s(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0])),
            ("1", f16s(&[0.5, 1.5, -2.0])),
        ],
        false,
    );
    let f = read("m.pt", &bytes).unwrap();
    let mut g: TensorFile = f.clone();
    let wt = g
        .tensors
        .iter()
        .position(|t| t.name == "fc.weight_t")
        .unwrap();
    g.tensors[wt].name = "fc.weight".into();
    g.tensors.retain(|t| t.name == "fc.weight");
    g.tensors.remove(0);
    let v = compare_values(
        Some((&f, &bytes)),
        Some((&g, &bytes)),
        &["fc.weight".into()],
        u64::MAX,
        &AtomicBool::new(false),
    );
    let d = v[0].diff.as_ref().unwrap();
    // [1,2,3,4,5,6] vs [1,4,2,5,3,6]: four elements differ.
    assert_eq!(d.changed, 4);
}

#[test]
fn onnx_tflite_coreml_tf_values() {
    let onnx = onnx_model();
    let s = one("m.onnx", &onnx, "conv.weight").new.unwrap();
    close(s.mean, 3.5);
    let s = one("m.onnx", &onnx, "shape").new.unwrap();
    close(s.min, -2.0);
    close(s.max, 3.0);
    let v = one("m.onnx", &onnx, "big");
    assert!(v.note.unwrap().contains("external file weights.bin"));

    let tfl = tflite_model();
    close(one("m.tflite", &tfl, "weights").new.unwrap().mean, 2.5);
    close(one("m.tflite", &tfl, "bias").new.unwrap().min, -1.0);

    let ml = coreml_model();
    close(one("m.mlmodel", &ml, "fc.weights").new.unwrap().mean, 2.5);
    close(one("m.mlmodel", &ml, "fc.bias").new.unwrap().max, 0.5);

    let tf = tf_graph();
    close(one("g.pb", &tf, "w").new.unwrap().mean, 2.5);
}

#[test]
fn hdf5_values() {
    let bytes = fixture("keras2.h5");
    let s = one("w.h5", &bytes, "/model_weights/dense/dense/kernel:0")
        .new
        .unwrap();
    close(s.mean, 5.5);
    // Chunked, shuffled and deflated: arange(70) in row-major order.
    let f = read("w.h5", &bytes).unwrap();
    let mut g = f.clone();
    let name = "/model_weights/dense_1/dense_1/kernel:0";
    let v = compare_values(
        Some((&f, &bytes)),
        Some((&g, &bytes)),
        &[name.into()],
        u64::MAX,
        &AtomicBool::new(false),
    );
    let s = v[0].new.as_ref().unwrap();
    close(s.mean, 34.5);
    close(s.max, 69.0);
    // Same values as a dense copy of arange(70).
    let dense = safetensors(
        &[(
            name,
            "F32",
            &[10, 7],
            f32s(&(0..70).map(|i| i as f32).collect::<Vec<_>>()),
        )],
        &[],
    );
    let d = read("d.safetensors", &dense).unwrap();
    g = d;
    let v = compare_values(
        Some((&f, &bytes)),
        Some((&g, &dense)),
        &[name.into()],
        u64::MAX,
        &AtomicBool::new(false),
    );
    assert!(v[0].diff.as_ref().unwrap().identical, "{:?}", v[0]);

    close(one("w.h5", &bytes, "/big").new.unwrap().min, -2.25);
    close(one("w.h5", &bytes, "/half").new.unwrap().mean, 2.5);
    close(one("w.h5", &bytes, "/flags").new.unwrap().mean, 2.0 / 3.0);
    close(one("w.h5", &bytes, "/ints").new.unwrap().min, -3.0);

    let latest = fixture("latest.h5");
    close(one("w.h5", &latest, "/chunked").new.unwrap().mean, 11.5);
    close(one("w.h5", &latest, "/deflated").new.unwrap().max, 29.0);
    close(one("w.h5", &latest, "/plain").new.unwrap().min, -1.0);
    close(one("w.h5", &latest, "/many/w07").new.unwrap().mean, 7.0);
}

#[test]
fn keras_zip_values() {
    let h5 = fixture("keras2.h5");
    for deflate in [false, true] {
        let bytes = zip(&[("model.weights.h5", &h5, deflate)]);
        let s = one("m.keras", &bytes, "/model_weights/dense_1/dense_1/kernel:0")
            .new
            .unwrap();
        close(s.mean, 34.5);
        close(
            one("m.keras", &bytes, "/model_weights/dense/dense/bias:0")
                .new
                .unwrap()
                .max,
            1.0,
        );
    }
}

#[test]
fn file_source() {
    let bytes = safetensors(&[("a", "F32", &[2], f32s(&[1.0, 3.0]))], &[]);
    let dir = std::env::temp_dir().join(format!("corvene-tensors-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("m.safetensors");
    std::fs::write(&path, &bytes).unwrap();
    let file = std::fs::File::open(&path).unwrap();
    assert_eq!(Source::len(&file), bytes.len() as u64);
    let f = read("m.safetensors", &file).unwrap();
    let v = compare_values(
        None,
        Some((&f, &file)),
        &[],
        u64::MAX,
        &AtomicBool::new(false),
    );
    close(v[0].new.as_ref().unwrap().mean, 2.0);
    std::fs::remove_dir_all(&dir).unwrap();
}
