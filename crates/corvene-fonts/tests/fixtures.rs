//! The fixtures of `tests/fixtures` (see its README for what differs
//! between each v1 and v2) read, compared and drawn.

use corvene_fonts::compare::{GlyphChanges, Status};
use corvene_fonts::layout::LayoutTable;
use corvene_fonts::render::{Ink, TextBlock, TextOptions, render_glyph, render_text, tile};
use corvene_fonts::{Format, compare, decode};

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn face_diff(old: &str, new: &str) -> corvene_fonts::FaceDiff {
    let diff = compare(Some(&fixture(old)), Some(&fixture(new))).expect("fonts");
    diff.face(0).expect("face 0")
}

fn glyph<'a>(d: &'a corvene_fonts::FaceDiff, name: &str) -> &'a corvene_fonts::GlyphRow {
    d.glyphs
        .iter()
        .find(|g| g.name == name)
        .unwrap_or_else(|| panic!("no glyph {name}"))
}

#[test]
fn woff_and_woff2_decode_to_the_same_tables_as_the_ttf() {
    for (packed, plain, format) in [
        ("sans-v1.woff", "sans-v1.ttf", Format::Woff),
        ("sans-v2.woff2", "sans-v2.ttf", Format::Woff2),
    ] {
        let a = decode(&fixture(packed)).expect(packed);
        let b = decode(&fixture(plain)).expect(plain);
        assert_eq!(a.format, format);
        assert!(a.file_len < a.decoded_len());
        let d = corvene_fonts::FaceDiff::new(
            Some(std::sync::Arc::new(corvene_fonts::FaceReport::read(
                &a.face(0).unwrap(),
                0,
            ))),
            Some(std::sync::Arc::new(corvene_fonts::FaceReport::read(
                &b.face(0).unwrap(),
                0,
            ))),
        );
        assert_eq!(d.summary.glyphs_modified, 0, "{packed}");
        assert_eq!(
            d.summary.glyphs_added + d.summary.glyphs_removed,
            0,
            "{packed}"
        );
        // only head (checksum adjustment, flags) and the transformed
        // glyf/loca may differ in bytes
        for t in &d.tables {
            if t.status.is_change() {
                assert!(
                    ["head", "glyf", "loca", "DSIG"].contains(&t.tag.as_str()),
                    "{packed}: {}",
                    t.tag
                );
            }
        }
    }
}

#[test]
fn sans_changes_are_found() {
    let d = face_diff("sans-v1.ttf", "sans-v2.ttf");
    let a = glyph(&d, "a");
    assert_eq!(a.status, Status::Modified);
    assert!(a.changes.has(GlyphChanges::OUTLINE));
    let n = glyph(&d, "n");
    assert!(n.changes.has(GlyphChanges::ADVANCE));
    assert!(!n.changes.has(GlyphChanges::OUTLINE));
    assert_eq!(glyph(&d, "Euro").status, Status::Added);
    assert_eq!(glyph(&d, "Omega").status, Status::Added);
    assert_eq!(glyph(&d, "asciitilde").status, Status::Removed);
    let sharp = glyph(&d, "germandbls");
    assert_eq!(sharp.old_name.as_deref(), Some("uni00DF"));
    assert!(sharp.changes.has(GlyphChanges::NAME));
    assert_eq!(glyph(&d, "b").status, Status::Unchanged);

    let feature = |tag: &str| {
        d.features
            .iter()
            .find(|f| f.table == LayoutTable::Gsub && f.tag == tag)
            .unwrap_or_else(|| panic!("no feature {tag}"))
    };
    assert_eq!(feature("ss02").status, Status::Removed);
    assert_eq!(feature("ss03").status, Status::Added);
    assert_eq!(feature("ss03").ui_name.as_deref(), Some("Open g"));
    assert!(
        feature("ss03")
            .added_rules
            .iter()
            .any(|r| r.input == ["g"] && r.output == ["g.ss03"])
    );

    let to = d
        .kerning
        .iter()
        .find(|k| k.left == "T" && k.right == "o")
        .expect("T o kerning changed");
    assert_eq!((to.old, to.new), (Some(-224), Some(-254)));
    assert!(!d.kerning.iter().any(|k| k.left == "A" && k.right == "V"));

    let field = |label: &str| d.overview.iter().find(|f| f.label == label).expect(label);
    assert_eq!(field("Version").status(), Status::Modified);
    assert_eq!(field("Vendor").new.as_deref(), Some("CRVN"));
    assert_eq!(field("Weight").status(), Status::Unchanged);
    let euro = d
        .coverage
        .iter()
        .find(|b| b.name == "Currency Symbols")
        .expect("block");
    assert_eq!(euro.added, [0x20AC]);
    assert!(
        d.table_fields["OS/2"]
            .iter()
            .any(|f| f.status() == Status::Modified)
    );
}

#[test]
fn cff_outlines_compare_like_truetype() {
    let d = face_diff("sans-cff-v1.otf", "sans-cff-v2.otf");
    assert!(glyph(&d, "a").changes.has(GlyphChanges::OUTLINE));
    assert_eq!(glyph(&d, "b").status, Status::Unchanged);
    assert_eq!(glyph(&d, "Euro").status, Status::Added);
    assert_eq!(
        d.old.as_ref().unwrap().outline,
        corvene_fonts::report::OutlineKind::Cff
    );
}

#[test]
fn collection_faces_pair_by_name() {
    let diff = compare(
        Some(&fixture("family-v1.ttc")),
        Some(&fixture("family-v2.ttc")),
    )
    .unwrap();
    let got: Vec<Status> = diff.faces.iter().map(|f| f.status).collect();
    assert_eq!(got, [Status::Modified, Status::Modified, Status::Added]);
    let noto = diff.face(1).unwrap();
    assert!(glyph(&noto, "A").changes.has(GlyphChanges::OUTLINE));
    assert_eq!(noto.summary.glyphs_modified, 1);
}

#[test]
fn variable_changes_show_without_outline_changes() {
    let d = face_diff("var-v1.ttf", "var-v2.ttf");
    let o = glyph(&d, "o");
    assert!(o.changes.has(GlyphChanges::VARIATIONS));
    assert!(!o.changes.has(GlyphChanges::OUTLINE));
    assert_eq!(glyph(&d, "a").status, Status::Unchanged);
    assert!(
        d.instances
            .iter()
            .any(|i| i.status == Status::Removed && i.name == "Thin")
    );
    assert!(d.instances.iter().any(|i| i.status == Status::Modified));
    assert!(d.axes.iter().all(|a| a.status == Status::Unchanged));
}

#[test]
fn a_new_font_lists_everything_as_added() {
    let diff = compare(None, Some(&fixture("sans-v1.ttf"))).unwrap();
    let d = diff.face(0).unwrap();
    assert!(d.glyphs.iter().all(|g| g.status == Status::Added));
    assert!(d.coverage.iter().all(|b| b.added.is_empty()));
}

#[test]
fn renders_glyphs_text_and_colour() {
    let ink = Ink::rgba(0, 0, 0, 255);
    let font_bytes = decode(&fixture("sans-v2.woff2")).unwrap();
    let font = font_bytes.face(0).unwrap();
    let t = tile(&font, &[], 64, 80, 0.8);
    let report = corvene_fonts::FaceReport::read(&font, 0);
    let a = report.glyph_by_name("a").unwrap();
    assert!(!render_glyph(&font, a, &[], t, ink).is_blank());
    let layout = render_text(
        &font,
        &[TextBlock {
            text: "Toggle fig ss03 Ωmega €".into(),
            size: 32.0,
        }],
        &TextOptions {
            features: vec![(corvene_fonts::Tag::new(b"ss03"), 1)],
            variations: Vec::new(),
            width: 200.0,
            line_height: 1.2,
            block_gap: 0.0,
            padding: 4.0,
            ink,
        },
    );
    assert!(!layout.raster.is_blank());
    // wrapped onto more than one line
    assert!(layout.raster.height > 80);
    let open_g = report.glyph_by_name("g.ss03").unwrap();
    assert!(layout.glyphs.iter().any(|g| g.gid == open_g));

    let colr = decode(&fixture("colr.ttf")).unwrap();
    let colr = colr.face(0).unwrap();
    let report = corvene_fonts::FaceReport::read(&colr, 0);
    let coloured = report
        .glyphs
        .iter()
        .position(|g| g.color)
        .expect("a colour glyph") as u32;
    let r = render_glyph(&colr, coloured, &[], tile(&colr, &[], 64, 64, 0.9), ink);
    // something other than the ink colour was painted
    assert!(
        r.data
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[3] > 0 && (p[0] > 0 || p[1] > 0 || p[2] > 0))
    );
}

#[test]
fn damaged_files_fail_cleanly() {
    let mut bytes = fixture("sans-v2.woff2");
    bytes.truncate(bytes.len() / 2);
    assert!(decode(&bytes).is_err());
    assert!(compare(Some(b"not a font"), None).is_err());
    // one readable side is enough
    let diff = compare(Some(b"wOFFbroken"), Some(&fixture("sans-v1.ttf"))).unwrap();
    assert!(diff.old_error.is_some());
    assert_eq!(diff.faces.len(), 1);
}
