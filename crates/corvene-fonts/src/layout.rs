//! OpenType layout read for diffs: the scripts, languages and features of
//! `GSUB` / `GPOS`, the substitutions each `GSUB` feature makes, and the
//! kerning pairs of `GPOS` (`kern` feature) and the legacy `kern` table.
//!
//! Contextual lookups are only counted: listing them would mean replaying
//! the shaper. Glyphs are ids here; [`crate::compare`] matches them by name.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use skrifa::raw::TableProvider;
use skrifa::raw::tables::gpos::{PairPos, PositionSubtables};
use skrifa::raw::tables::gsub::{SingleSubst, SubstitutionSubtables};
use skrifa::raw::tables::layout::{FeatureList, FeatureParams, ScriptList};
use skrifa::raw::types::Tag;
use skrifa::string::StringId;
use skrifa::{FontRef, MetadataProvider};

/// At most this many substitution rules are kept per feature.
pub const MAX_RULES: usize = 20_000;
/// At most this many kerning pairs are kept per face.
pub const MAX_PAIRS: usize = 300_000;

/// Which layout table a feature lives in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LayoutTable {
    Gsub,
    Gpos,
}

impl LayoutTable {
    pub fn label(self) -> &'static str {
        match self {
            LayoutTable::Gsub => "GSUB",
            LayoutTable::Gpos => "GPOS",
        }
    }
}

/// How a substitution rule maps glyphs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RuleKind {
    /// One glyph for another.
    Single,
    /// One glyph for several.
    Multiple,
    /// One glyph with alternates to pick from.
    Alternate,
    /// Several glyphs for one.
    Ligature,
}

/// One substitution: `input` glyphs become `output` glyphs.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Rule {
    pub kind: RuleKind,
    pub input: Vec<u32>,
    pub output: Vec<u32>,
}

/// One feature tag of one layout table, merged over every script and
/// language that uses it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Feature {
    /// `ss01`'s "Single-storey a" and similar (`FeatureParams` name ids).
    pub ui_name: Option<String>,
    /// Lookup indexes, in feature order.
    pub lookups: Vec<u16>,
    /// Lookup types used (GSUB 1–8 / GPOS 1–9, extensions resolved).
    pub lookup_types: BTreeSet<u16>,
    /// GSUB only: the context-free substitutions, sorted.
    pub rules: Vec<Rule>,
    /// Lookups whose rules are contextual (counted, not listed).
    pub contextual: u32,
    /// Whether `rules` stopped at [`MAX_RULES`].
    pub truncated: bool,
    /// Hash of the lookups' fields (glyphs by name), so a change to lookups
    /// this module does not list (marks, contextual rules) still shows.
    pub data_hash: u64,
}

/// A script with its languages and the features each one turns on
/// (`dflt` is the script's default language system).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Script {
    pub tag: String,
    pub languages: Vec<(String, Vec<String>)>,
}

/// One layout table read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayoutReport {
    pub scripts: Vec<Script>,
    pub features: BTreeMap<String, Feature>,
    pub lookup_count: u32,
}

/// Kerning pairs: (left glyph, right glyph) → x advance adjustment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Kerning {
    pub pairs: HashMap<(u32, u32), i32>,
    /// Whether reading stopped at [`MAX_PAIRS`].
    pub truncated: bool,
}

/// Feature names and descriptions from the OpenType feature registry, for
/// tags a font does not name itself.
pub fn registered_name(tag: &str) -> Option<&'static str> {
    Some(match tag {
        "aalt" => "Access All Alternates",
        "abvf" => "Above-base Forms",
        "abvm" => "Above-base Mark Positioning",
        "abvs" => "Above-base Substitutions",
        "afrc" => "Alternative Fractions",
        "akhn" => "Akhand",
        "blwf" => "Below-base Forms",
        "blwm" => "Below-base Mark Positioning",
        "blws" => "Below-base Substitutions",
        "c2pc" => "Petite Capitals From Capitals",
        "c2sc" => "Small Capitals From Capitals",
        "calt" => "Contextual Alternates",
        "case" => "Case-Sensitive Forms",
        "ccmp" => "Glyph Composition / Decomposition",
        "cfar" => "Conjunct Form After Ro",
        "cjct" => "Conjunct Forms",
        "clig" => "Contextual Ligatures",
        "cpct" => "Centered CJK Punctuation",
        "cpsp" => "Capital Spacing",
        "cswh" => "Contextual Swash",
        "curs" => "Cursive Positioning",
        "dist" => "Distances",
        "dlig" => "Discretionary Ligatures",
        "dnom" => "Denominators",
        "dtls" => "Dotless Forms",
        "expt" => "Expert Forms",
        "falt" => "Final Glyph on Line Alternates",
        "fin2" => "Terminal Forms #2",
        "fin3" => "Terminal Forms #3",
        "fina" => "Terminal Forms",
        "flac" => "Flattened Accent Forms",
        "frac" => "Fractions",
        "fwid" => "Full Widths",
        "half" => "Half Forms",
        "haln" => "Halant Forms",
        "halt" => "Alternate Half Widths",
        "hist" => "Historical Forms",
        "hkna" => "Horizontal Kana Alternates",
        "hlig" => "Historical Ligatures",
        "hngl" => "Hangul",
        "hojo" => "Hojo Kanji Forms",
        "hwid" => "Half Widths",
        "init" => "Initial Forms",
        "isol" => "Isolated Forms",
        "ital" => "Italics",
        "jalt" => "Justification Alternates",
        "jp78" => "JIS78 Forms",
        "jp83" => "JIS83 Forms",
        "jp90" => "JIS90 Forms",
        "jp04" => "JIS2004 Forms",
        "kern" => "Kerning",
        "lfbd" => "Left Bounds",
        "liga" => "Standard Ligatures",
        "ljmo" => "Leading Jamo Forms",
        "lnum" => "Lining Figures",
        "locl" => "Localized Forms",
        "ltra" => "Left-to-right Alternates",
        "ltrm" => "Left-to-right Mirrored Forms",
        "mark" => "Mark Positioning",
        "med2" => "Medial Forms #2",
        "medi" => "Medial Forms",
        "mgrk" => "Mathematical Greek",
        "mkmk" => "Mark to Mark Positioning",
        "mset" => "Mark Positioning via Substitution",
        "nalt" => "Alternate Annotation Forms",
        "nlck" => "NLC Kanji Forms",
        "nukt" => "Nukta Forms",
        "numr" => "Numerators",
        "onum" => "Oldstyle Figures",
        "opbd" => "Optical Bounds",
        "ordn" => "Ordinals",
        "ornm" => "Ornaments",
        "palt" => "Proportional Alternate Widths",
        "pcap" => "Petite Capitals",
        "pkna" => "Proportional Kana",
        "pnum" => "Proportional Figures",
        "pref" => "Pre-base Forms",
        "pres" => "Pre-base Substitutions",
        "pstf" => "Post-base Forms",
        "psts" => "Post-base Substitutions",
        "pwid" => "Proportional Widths",
        "qwid" => "Quarter Widths",
        "rand" => "Randomize",
        "rclt" => "Required Contextual Alternates",
        "rkrf" => "Rakar Forms",
        "rlig" => "Required Ligatures",
        "rphf" => "Reph Form",
        "rtbd" => "Right Bounds",
        "rtla" => "Right-to-left Alternates",
        "rtlm" => "Right-to-left Mirrored Forms",
        "ruby" => "Ruby Notation Forms",
        "rvrn" => "Required Variation Alternates",
        "salt" => "Stylistic Alternates",
        "sinf" => "Scientific Inferiors",
        "size" => "Optical size",
        "smcp" => "Small Capitals",
        "smpl" => "Simplified Forms",
        "ssty" => "Math Script-style Alternates",
        "stch" => "Stretching Glyph Decomposition",
        "subs" => "Subscript",
        "sups" => "Superscript",
        "swsh" => "Swash",
        "titl" => "Titling",
        "tjmo" => "Trailing Jamo Forms",
        "tnam" => "Traditional Name Forms",
        "tnum" => "Tabular Figures",
        "trad" => "Traditional Forms",
        "twid" => "Third Widths",
        "unic" => "Unicase",
        "valt" => "Alternate Vertical Metrics",
        "vatu" => "Vattu Variants",
        "vchw" => "Vertical Contextual Half-width Spacing",
        "vert" => "Vertical Alternates",
        "vhal" => "Alternate Vertical Half Metrics",
        "vjmo" => "Vowel Jamo Forms",
        "vkna" => "Vertical Kana Alternates",
        "vkrn" => "Vertical Kerning",
        "vpal" => "Proportional Alternate Vertical Metrics",
        "vrt2" => "Vertical Alternates and Rotation",
        "vrtr" => "Vertical Alternates for Rotation",
        "zero" => "Slashed Zero",
        t if t.len() == 4 && t.starts_with("ss") && t[2..].parse::<u8>().is_ok() => "Stylistic Set",
        t if t.len() == 4 && t.starts_with("cv") && t[2..].parse::<u8>().is_ok() => {
            "Character Variant"
        }
        _ => return None,
    })
}

/// Features a shaper applies without being asked (so the playground shows
/// them as on by default).
pub fn on_by_default(tag: &str) -> bool {
    matches!(
        tag,
        "abvf"
            | "abvm"
            | "abvs"
            | "akhn"
            | "blwf"
            | "blwm"
            | "blws"
            | "calt"
            | "ccmp"
            | "cfar"
            | "cjct"
            | "clig"
            | "curs"
            | "dist"
            | "fin2"
            | "fin3"
            | "fina"
            | "half"
            | "haln"
            | "init"
            | "isol"
            | "kern"
            | "liga"
            | "ljmo"
            | "locl"
            | "ltra"
            | "ltrm"
            | "mark"
            | "med2"
            | "medi"
            | "mkmk"
            | "mset"
            | "nukt"
            | "pref"
            | "pres"
            | "pstf"
            | "psts"
            | "rclt"
            | "rkrf"
            | "rlig"
            | "rphf"
            | "rtla"
            | "rtlm"
            | "rvrn"
            | "tjmo"
            | "vatu"
            | "vjmo"
    )
}

fn tag_string(tag: Tag) -> String {
    tag.to_string().trim_end().to_string()
}

fn name_string(font: &FontRef, id: StringId) -> Option<String> {
    font.localized_strings(id)
        .english_or_first()
        .map(|s| s.to_string())
}

/// Scripts → languages → features of a script list.
fn scripts(list: &ScriptList, features: &FeatureList) -> Vec<Script> {
    let data = list.offset_data();
    let feature_tag = |index: u16| {
        features
            .feature_records()
            .get(index as usize)
            .map(|r| tag_string(r.feature_tag()))
    };
    let lang_features = |lang: &skrifa::raw::tables::layout::LangSys| {
        let mut tags: Vec<String> = lang
            .feature_indices()
            .iter()
            .filter_map(|i| feature_tag(i.get()))
            .collect();
        if lang.required_feature_index() != 0xFFFF {
            tags.extend(feature_tag(lang.required_feature_index()));
        }
        tags.sort();
        tags.dedup();
        tags
    };
    let mut out = Vec::new();
    for record in list.script_records() {
        let Ok(script) = record.script(data) else {
            continue;
        };
        let mut languages = Vec::new();
        if let Some(Ok(lang)) = script.default_lang_sys() {
            languages.push(("dflt".to_string(), lang_features(&lang)));
        }
        let sdata = script.offset_data();
        for lr in script.lang_sys_records() {
            if let Ok(lang) = lr.lang_sys(sdata) {
                languages.push((tag_string(lr.lang_sys_tag()), lang_features(&lang)));
            }
        }
        out.push(Script {
            tag: tag_string(record.script_tag()),
            languages,
        });
    }
    out
}

/// Feature tag → lookup indexes, merged over duplicate feature records
/// (one per script / language variant), plus the `FeatureParams` UI name.
fn feature_lookups(
    font: &FontRef,
    list: &FeatureList,
) -> BTreeMap<String, (Vec<u16>, Option<String>)> {
    let data = list.offset_data();
    let mut out: BTreeMap<String, (Vec<u16>, Option<String>)> = BTreeMap::new();
    for record in list.feature_records() {
        let tag = tag_string(record.feature_tag());
        let Ok(feature) = record.feature(data) else {
            continue;
        };
        let ui_name = match feature.feature_params() {
            Some(Ok(FeatureParams::StylisticSet(p))) => {
                name_string(font, StringId::new(p.ui_name_id().to_u16()))
            }
            Some(Ok(FeatureParams::CharacterVariant(p))) => {
                name_string(font, StringId::new(p.feat_ui_label_name_id().to_u16()))
            }
            _ => None,
        };
        let entry = out.entry(tag).or_default();
        for i in feature.lookup_list_indices() {
            if !entry.0.contains(&i.get()) {
                entry.0.push(i.get());
            }
        }
        if entry.1.is_none() {
            entry.1 = ui_name;
        }
    }
    out
}

/// What one GSUB lookup contributes to a feature.
#[derive(Default)]
struct LookupRead {
    lookup_type: u16,
    rules: Vec<Rule>,
    contextual: bool,
    hash: u64,
}

fn gsub_lookup(
    lookup: &skrifa::raw::tables::gsub::SubstitutionLookup,
    names: &[&str],
) -> LookupRead {
    let mut read = LookupRead {
        lookup_type: lookup.lookup_type(),
        hash: crate::report::hash_table(lookup, names),
        ..Default::default()
    };
    let Ok(subtables) = lookup.subtables() else {
        return read;
    };
    let rules = &mut read.rules;
    let mut push = |rule: Rule| {
        if rules.len() < MAX_RULES {
            rules.push(rule);
        }
    };
    match subtables {
        SubstitutionSubtables::Single(s) => {
            read.lookup_type = 1;
            for st in s.iter().flatten() {
                match st {
                    SingleSubst::Format1(f) => {
                        let Ok(cov) = f.coverage() else { continue };
                        let delta = i32::from(f.delta_glyph_id());
                        for g in cov.iter() {
                            let out = (i32::from(g.to_u16()) + delta).rem_euclid(65536) as u32;
                            push(Rule {
                                kind: RuleKind::Single,
                                input: vec![g.to_u32()],
                                output: vec![out],
                            });
                        }
                    }
                    SingleSubst::Format2(f) => {
                        let Ok(cov) = f.coverage() else { continue };
                        for (g, sub) in cov.iter().zip(f.substitute_glyph_ids()) {
                            push(Rule {
                                kind: RuleKind::Single,
                                input: vec![g.to_u32()],
                                output: vec![sub.get().to_u32()],
                            });
                        }
                    }
                }
            }
        }
        SubstitutionSubtables::Multiple(s) => {
            read.lookup_type = 2;
            for f in s.iter().flatten() {
                let Ok(cov) = f.coverage() else { continue };
                for (g, seq) in cov.iter().zip(f.sequences().iter()) {
                    let Ok(seq) = seq else { continue };
                    push(Rule {
                        kind: RuleKind::Multiple,
                        input: vec![g.to_u32()],
                        output: seq
                            .substitute_glyph_ids()
                            .iter()
                            .map(|g| g.get().to_u32())
                            .collect(),
                    });
                }
            }
        }
        SubstitutionSubtables::Alternate(s) => {
            read.lookup_type = 3;
            for f in s.iter().flatten() {
                let Ok(cov) = f.coverage() else { continue };
                for (g, set) in cov.iter().zip(f.alternate_sets().iter()) {
                    let Ok(set) = set else { continue };
                    push(Rule {
                        kind: RuleKind::Alternate,
                        input: vec![g.to_u32()],
                        output: set
                            .alternate_glyph_ids()
                            .iter()
                            .map(|g| g.get().to_u32())
                            .collect(),
                    });
                }
            }
        }
        SubstitutionSubtables::Ligature(s) => {
            read.lookup_type = 4;
            for f in s.iter().flatten() {
                let Ok(cov) = f.coverage() else { continue };
                for (g, set) in cov.iter().zip(f.ligature_sets().iter()) {
                    let Ok(set) = set else { continue };
                    for lig in set.ligatures().iter().flatten() {
                        let mut input = vec![g.to_u32()];
                        input.extend(lig.component_glyph_ids().iter().map(|g| g.get().to_u32()));
                        push(Rule {
                            kind: RuleKind::Ligature,
                            input,
                            output: vec![lig.ligature_glyph().to_u32()],
                        });
                    }
                }
            }
        }
        SubstitutionSubtables::Contextual(_) => {
            read.lookup_type = 5;
            read.contextual = true;
        }
        SubstitutionSubtables::ChainContextual(_) => {
            read.lookup_type = 6;
            read.contextual = true;
        }
        SubstitutionSubtables::Reverse(_) => {
            read.lookup_type = 8;
            read.contextual = true;
        }
    }
    read
}

/// The `GSUB` table read.
pub fn gsub(font: &FontRef, names: &[&str]) -> Option<LayoutReport> {
    let gsub = font.gsub().ok()?;
    let (scripts_list, features_list) = (gsub.script_list().ok()?, gsub.feature_list().ok()?);
    let lookups = gsub.lookup_list().ok()?;
    let lookup_count = lookups.lookup_count();
    let mut cache: HashMap<u16, LookupRead> = HashMap::new();
    let mut features = BTreeMap::new();
    for (tag, (indexes, ui_name)) in feature_lookups(font, &features_list) {
        let mut feature = Feature {
            ui_name,
            ..Default::default()
        };
        let mut hash = crate::outline::Fnv::new();
        for &i in &indexes {
            let read = cache.entry(i).or_insert_with(|| {
                lookups
                    .lookups()
                    .get(i as usize)
                    .map(|l| gsub_lookup(&l, names))
                    .unwrap_or_default()
            });
            feature.lookup_types.insert(read.lookup_type);
            if read.contextual {
                feature.contextual += 1;
            }
            hash.int(read.hash as i64);
            for rule in &read.rules {
                if feature.rules.len() >= MAX_RULES {
                    feature.truncated = true;
                    break;
                }
                feature.rules.push(rule.clone());
            }
        }
        feature.rules.sort();
        feature.rules.dedup();
        feature.lookups = indexes;
        feature.data_hash = hash.finish();
        features.insert(tag, feature);
    }
    Some(LayoutReport {
        scripts: scripts(&scripts_list, &features_list),
        features,
        lookup_count: u32::from(lookup_count),
    })
}

/// The `GPOS` table read, with the pairs of its `kern` feature.
pub fn gpos(font: &FontRef, names: &[&str]) -> Option<(LayoutReport, Kerning)> {
    let gpos = font.gpos().ok()?;
    let (scripts_list, features_list) = (gpos.script_list().ok()?, gpos.feature_list().ok()?);
    let lookups = gpos.lookup_list().ok()?;
    let mut features = BTreeMap::new();
    let mut kern_lookups: Vec<u16> = Vec::new();
    for (tag, (indexes, ui_name)) in feature_lookups(font, &features_list) {
        let mut feature = Feature {
            ui_name,
            ..Default::default()
        };
        let mut hash = crate::outline::Fnv::new();
        for &i in &indexes {
            let Ok(lookup) = lookups.lookups().get(i as usize) else {
                continue;
            };
            let kind = match lookup.subtables() {
                Ok(PositionSubtables::Single(_)) => 1,
                Ok(PositionSubtables::Pair(_)) => 2,
                Ok(PositionSubtables::Cursive(_)) => 3,
                Ok(PositionSubtables::MarkToBase(_)) => 4,
                Ok(PositionSubtables::MarkToLig(_)) => 5,
                Ok(PositionSubtables::MarkToMark(_)) => 6,
                Ok(PositionSubtables::Contextual(_)) => 7,
                Ok(PositionSubtables::ChainContextual(_)) => 8,
                Err(_) => lookup.lookup_type(),
            };
            if matches!(kind, 7 | 8) {
                feature.contextual += 1;
            }
            feature.lookup_types.insert(kind);
            hash.int(crate::report::hash_table(&lookup, names) as i64);
        }
        if tag == "kern" {
            kern_lookups.extend(&indexes);
        }
        feature.lookups = indexes;
        feature.data_hash = hash.finish();
        features.insert(tag, feature);
    }
    let mut kerning = Kerning::default();
    for i in kern_lookups {
        let Ok(lookup) = lookups.lookups().get(i as usize) else {
            continue;
        };
        if let Ok(PositionSubtables::Pair(subtables)) = lookup.subtables() {
            // within a lookup the first subtable that covers a pair wins;
            // separate lookups add up
            let mut seen: HashMap<(u32, u32), i32> = HashMap::new();
            for st in subtables.iter().flatten() {
                pair_pos(&st, &mut seen, &mut kerning.truncated);
            }
            for (pair, value) in seen {
                if kerning.pairs.len() >= MAX_PAIRS {
                    kerning.truncated = true;
                    break;
                }
                *kerning.pairs.entry(pair).or_default() += value;
            }
        }
    }
    kerning.pairs.retain(|_, v| *v != 0);
    Some((
        LayoutReport {
            scripts: scripts(&scripts_list, &features_list),
            features,
            lookup_count: u32::from(lookups.lookup_count()),
        },
        kerning,
    ))
}

/// The x advance adjustments of one `PairPos` subtable (pairs already set
/// by an earlier subtable are kept).
fn pair_pos(st: &PairPos, out: &mut HashMap<(u32, u32), i32>, truncated: &mut bool) {
    let full = |out: &HashMap<_, _>, truncated: &mut bool| {
        let full = out.len() >= MAX_PAIRS;
        *truncated |= full;
        full
    };
    match st {
        PairPos::Format1(f) => {
            let Ok(cov) = f.coverage() else { return };
            for (first, set) in cov.iter().zip(f.pair_sets().iter()) {
                let Ok(set) = set else { continue };
                for rec in set.pair_value_records().iter().flatten() {
                    if full(out, truncated) {
                        return;
                    }
                    let value = i32::from(rec.value_record1().x_advance().unwrap_or(0));
                    out.entry((first.to_u32(), rec.second_glyph().to_u32()))
                        .or_insert(value);
                }
            }
        }
        PairPos::Format2(f) => {
            let (Ok(cov), Ok(cd1), Ok(cd2)) = (f.coverage(), f.class_def1(), f.class_def2()) else {
                return;
            };
            // class 0 of the second glyph is "every other glyph": skipped,
            // its values are zero in practice
            let mut by_class: BTreeMap<u16, Vec<u32>> = BTreeMap::new();
            for (g, class) in cd2.iter() {
                if class != 0 {
                    by_class.entry(class).or_default().push(g.to_u32());
                }
            }
            let records = f.class1_records();
            for first in cov.iter() {
                let c1 = cd1.get(first);
                let Ok(rec1) = records.get(c1 as usize) else {
                    continue;
                };
                for (c2, rec2) in rec1.class2_records().iter().enumerate() {
                    let Ok(rec2) = rec2 else { continue };
                    let value = i32::from(rec2.value_record1().x_advance().unwrap_or(0));
                    if value == 0 {
                        continue;
                    }
                    for &second in by_class.get(&(c2 as u16)).into_iter().flatten() {
                        if full(out, truncated) {
                            return;
                        }
                        out.entry((first.to_u32(), second)).or_insert(value);
                    }
                }
            }
        }
    }
}

/// Pairs of a legacy `kern` table (format 0 subtables), for fonts without
/// a `GPOS` `kern` feature.
pub fn legacy_kern(font: &FontRef) -> Kerning {
    let mut kerning = Kerning::default();
    let Ok(kern) = font.kern() else {
        return kerning;
    };
    for st in kern.subtables().flatten() {
        if !st.is_horizontal() || st.is_cross_stream() {
            continue;
        }
        if let Ok(skrifa::raw::tables::kern::SubtableKind::Format0(f)) = st.kind() {
            for pair in f.pairs() {
                if kerning.pairs.len() >= MAX_PAIRS {
                    kerning.truncated = true;
                    return kerning;
                }
                let value = i32::from(pair.value());
                if value != 0 {
                    kerning
                        .pairs
                        .insert((pair.left().to_u32(), pair.right().to_u32()), value);
                }
            }
        }
    }
    kerning
}
