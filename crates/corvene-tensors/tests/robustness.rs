//! Truncated and corrupted inputs: every reader returns `Err` or `Ok`,
//! never panics, and value reads of whatever was listed stay safe.
#![allow(clippy::ptr_arg)] // `&Vec<u8>` coerces to `&dyn Source`; `&[u8]` does not.

mod common;

use std::sync::atomic::AtomicBool;

use common::*;
use corvene_tensors::{compare_values, read};

fn fixtures() -> Vec<(&'static str, Vec<u8>)> {
    let q8 = [q8_0(0.5, &[1; 32]), q8_0(1.0, &[2; 32])].concat();
    vec![
        (
            "m.safetensors",
            safetensors(
                &[
                    ("a", "F32", &[2, 2], f32s(&[1.0; 4])),
                    ("b", "BF16", &[2], bf16s(&[1.0; 2])),
                ],
                &[("k", "v")],
            ),
        ),
        (
            "m.gguf",
            gguf(&[
                ("t", &[4, 2], 0, f32s(&[0.0; 8])),
                ("q", &[32, 2], 8, q8),
                ("k", &[256], 12, vec![5; 144]),
            ]),
        ),
        ("x.npy", npy("<f4", false, &[2, 3], &f32s(&[1.0; 6]))),
        (
            "x.npz",
            zip(&[
                ("a.npy", &npy("<f4", false, &[2], &f32s(&[1.0, 2.0])), false),
                ("b.npy", &npy("<i2", false, &[2], &[1, 0, 2, 0]), true),
            ]),
        ),
        (
            "m.pt",
            torch_zip(
                &torch_state_dict_pickle(),
                &[("0", f32s(&[1.0; 6])), ("1", f16s(&[1.0; 3]))],
                false,
            ),
        ),
        (
            "m.pth",
            torch_zip(
                &torch_state_dict_pickle(),
                &[("0", f32s(&[1.0; 6])), ("1", f16s(&[1.0; 3]))],
                true,
            ),
        ),
        ("m.onnx", onnx_model()),
        ("m.tflite", tflite_model()),
        ("m.mlmodel", coreml_model()),
        ("g.pb", tf_graph()),
        ("w.h5", fixture("keras2.h5")),
        ("w.hdf5", fixture("latest.h5")),
        (
            "m.keras",
            zip(&[("model.weights.h5", &fixture("latest.h5"), true)]),
        ),
    ]
}

fn exercise(path: &str, bytes: &Vec<u8>) {
    if let Ok(f) = read(path, bytes) {
        let _ = compare_values(
            Some((&f, bytes)),
            Some((&f, bytes)),
            &[],
            1 << 20,
            &AtomicBool::new(false),
        );
    }
}

#[test]
fn every_prefix() {
    for (path, bytes) in fixtures() {
        assert!(read(path, &bytes).is_ok(), "{path} reads whole");
        // Every prefix for small files, a stride past 4 KiB for large ones.
        let mut n = 0usize;
        while n < bytes.len() {
            let prefix = bytes[..n].to_vec();
            exercise(path, &prefix);
            n += if n < 4096 { 1 } else { 13 };
        }
    }
}

#[test]
fn corrupted_bytes() {
    for (path, bytes) in fixtures() {
        let step = (bytes.len() / 600).max(1);
        for at in (0..bytes.len()).step_by(step) {
            for v in [0x00u8, 0xff, 0x7f, 0x80] {
                let mut b = bytes.clone();
                b[at] = v;
                exercise(path, &b);
            }
        }
    }
}

#[test]
fn absurd_sizes() {
    // safetensors header length near u64::MAX.
    let mut b = u64::MAX.to_le_bytes().to_vec();
    b.extend_from_slice(b"{}");
    assert!(read("m.safetensors", &b).is_err());
    // GGUF counts far larger than the file.
    let mut g = b"GGUF".to_vec();
    g.extend_from_slice(&3u32.to_le_bytes());
    g.extend_from_slice(&u64::MAX.to_le_bytes());
    g.extend_from_slice(&u64::MAX.to_le_bytes());
    assert!(read("m.gguf", &g).is_err());
    // Deeply nested pickle lists.
    let mut p = Pickle::new();
    for _ in 0..100_000 {
        p.op(b"]");
    }
    for _ in 0..99_999 {
        p.op(b"a");
    }
    p.op(b".");
    let z = torch_zip(&p.0, &[], false);
    let _ = read("m.pt", &z);
    // A self-referencing list.
    let mut p = Pickle::new();
    p.op(b"]").put(0).get(0).op(b"a").get(0).op(b"a.");
    let z = torch_zip(&p.0, &[], false);
    assert!(read("m.pt", &z).is_ok());
    // Deeply nested protobuf messages are only walked where expected.
    let mut m = Vec::new();
    for _ in 0..1000 {
        m = pb(7, &m);
    }
    let _ = read("m.onnx", &m);
}
