//! The diff3 here against gix-merge's port of git's `xdl_merge`.

use corvene_merge::{Labels, MergeInput, Resolution, markers, merge_clean, reconcile};
use gix_merge::blob::builtin_driver::text as merge;
use gix_merge::blob::builtin_driver::text::{
    Conflict, ConflictStyle, Labels as GixLabels, Options,
};
use proptest::prelude::*;
use proptest::strategy::ValueTree;

fn gix(base: &str, ours: &str, theirs: &str) -> (bool, String) {
    let mut out = Vec::new();
    let mut input = gix_imara_diff::InternedInput::new(&[][..], &[][..]);
    let labels = GixLabels {
        ancestor: None,
        current: Some("ours".into()),
        other: Some("theirs".into()),
    };
    let options = Options {
        conflict: Conflict::Keep {
            style: ConflictStyle::Merge,
            marker_size: std::num::NonZeroU8::new(7).expect("non-zero"),
        },
        ..Options::default()
    };
    let res = merge(
        &mut out,
        &mut input,
        labels,
        ours.as_bytes(),
        base.as_bytes(),
        theirs.as_bytes(),
        options,
    );
    (
        res == gix_merge::blob::Resolution::Complete,
        String::from_utf8(out).expect("utf8"),
    )
}

/// A file of a few lines drawn from a small alphabet, so edits collide.
fn file() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(vec!["a\n", "b\n", "c\n", "d\n"]), 0..8)
        .prop_map(|lines| lines.concat())
}

fn edit(base: String) -> impl Strategy<Value = String> {
    let n = base.lines().count();
    (
        prop::collection::vec(any::<u8>(), n + 1),
        prop::collection::vec(prop::sample::select(vec!["x\n", "y\n", "a\n"]), 0..3),
    )
        .prop_map(move |(ops, ins)| {
            let mut out = String::new();
            let mut ins = ins.into_iter();
            for (i, line) in base.split_inclusive('\n').enumerate() {
                match ops[i] % 4 {
                    0 => {}
                    1 => out.push_str(ins.next().unwrap_or("z\n")),
                    2 => {
                        out.push_str(line);
                        out.push_str(ins.next().unwrap_or("z\n"));
                    }
                    _ => out.push_str(line),
                }
            }
            if ops[n] % 3 == 0 {
                out.push_str(ins.next().unwrap_or("w\n"));
            }
            out
        })
}

fn triple() -> impl Strategy<Value = (String, String, String)> {
    file().prop_flat_map(|base| (Just(base.clone()), edit(base.clone()), edit(base)))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    /// When both call a merge clean they agree on its text.
    #[test]
    fn clean_merges_agree((base, ours, theirs) in triple()) {
        let (gix_clean, gix_text) = gix(&base, &ours, &theirs);
        if let (true, Ok(ours_text)) = (gix_clean, merge_clean(&base, &ours, &theirs)) {
            prop_assert_eq!(ours_text, gix_text);
        }
    }

    /// Whatever git wrote, the editor opens it with every block as an
    /// unresolved conflict and accepting ours everywhere leaves no markers.
    #[test]
    fn git_output_reconciles((base, ours, theirs) in triple()) {
        let (_, working) = gix(&base, &ours, &theirs);
        let doc = reconcile(&MergeInput {
            base: Some(&base),
            ours: &ours,
            theirs: &theirs,
            working: &working,
            marker_size: 7,
            labels: Labels::default(),
            ..MergeInput::default()
        });
        let blocks = markers::parse(&working, 7).expect("git's markers parse");
        let unresolved = doc.chunks.iter().filter(|c| c.resolution == Resolution::Unresolved).count();
        prop_assert_eq!(unresolved, blocks.len());
        let mut text = working.clone();
        let mut anchors = corvene_merge::anchors::Anchors::new(doc.result_ranges());
        loop {
            let live = corvene_merge::scan(&doc, &text, anchors.ranges());
            let Some(next) = live.unresolved_chunks().next().cloned() else { break };
            let edit = corvene_merge::resolve(&doc, &text, &next, Resolution::Ours).expect("edit");
            text.replace_range(edit.range.clone(), &edit.text);
            anchors.edit_chunk(next.id as usize, edit.range, edit.text.len());
        }
        prop_assert!(markers::parse(&text, 7).expect("parses").is_empty());
    }
}

/// How often the two disagree on whether a merge is clean, for the record:
/// both run Myers, but tie-breaking differs.
#[test]
fn clean_or_not_mostly_agrees() {
    let mut runner = proptest::test_runner::TestRunner::deterministic();
    let mut differ = 0;
    let total = 2000;
    for _ in 0..total {
        let (base, ours, theirs) = triple().new_tree(&mut runner).expect("tree").current();
        let (gix_clean, _) = gix(&base, &ours, &theirs);
        if gix_clean != merge_clean(&base, &ours, &theirs).is_ok() {
            differ += 1;
        }
    }
    assert!(differ * 20 < total, "{differ} of {total} differ");
}
