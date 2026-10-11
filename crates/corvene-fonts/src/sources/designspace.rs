//! `.designspace` documents (formats 4 and 5): axes (continuous and
//! discrete), sources, instances, rules and variable fonts.

use super::plist::{Element, parse_xml};
use super::{Designspace, DsAxis, DsInstance, DsRule, DsSource};
use crate::FontError;

const FORMAT: &str = "designspace";

/// Reads a `.designspace` document. Locations are design values; a
/// format 5 `uservalue` is mapped through the axis map.
pub fn parse_designspace(bytes: &[u8]) -> Result<Designspace, FontError> {
    let root = parse_xml(bytes, FORMAT)?;
    if root.name != "designspace" {
        return Err(FontError::malformed(
            FORMAT,
            "root element is not <designspace>",
        ));
    }
    let axes: Vec<DsAxis> = root
        .child("axes")
        .map(|a| a.children_named("axis").map(axis).collect())
        .unwrap_or_default();

    let sources = root
        .child("sources")
        .map(|s| {
            s.children_named("source")
                .enumerate()
                .map(|(i, s)| {
                    let filename = s.attr("filename").unwrap_or_default().to_string();
                    let layer = s.attr("layer").map(str::to_string);
                    let name = match s.attr("name") {
                        Some(n) => n.to_string(),
                        None if !filename.is_empty() => match &layer {
                            Some(l) => format!("{filename} ({l})"),
                            None => filename.clone(),
                        },
                        None => format!("Source {}", i + 1),
                    };
                    DsSource {
                        name,
                        filename,
                        family: s.attr("familyname").unwrap_or_default().to_string(),
                        style: s.attr("stylename").unwrap_or_default().to_string(),
                        layer,
                        location: location(s, &axes),
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let instances = root
        .child("instances")
        .map(|s| {
            s.children_named("instance")
                .enumerate()
                .map(|(i, el)| {
                    let family = el.attr("familyname").unwrap_or_default().to_string();
                    let style = el.attr("stylename").unwrap_or_default().to_string();
                    let filename = el.attr("filename").unwrap_or_default().to_string();
                    let name = match el.attr("name") {
                        Some(n) => n.to_string(),
                        None if !family.is_empty() || !style.is_empty() => {
                            format!("{family} {style}").trim().to_string()
                        }
                        None if !filename.is_empty() => filename.clone(),
                        None => format!("Instance {}", i + 1),
                    };
                    DsInstance {
                        name,
                        family,
                        style,
                        filename,
                        location: location(el, &axes),
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let rules = root
        .child("rules")
        .map(|r| {
            r.children_named("rule")
                .enumerate()
                .map(|(i, r)| rule(r, i))
                .collect()
        })
        .unwrap_or_default();

    let variable_fonts = root
        .child("variable-fonts")
        .map(|v| {
            v.children_named("variable-font")
                .filter_map(|f| f.attr("name").or(f.attr("filename")).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    Ok(Designspace {
        format: root.attr("format").unwrap_or_default().to_string(),
        axes,
        sources,
        instances,
        rules,
        variable_fonts,
    })
}

fn axis(a: &Element) -> DsAxis {
    let values: Vec<f32> = a
        .attr("values")
        .map(|v| {
            v.split(|c: char| c.is_whitespace() || c == ',')
                .filter_map(|n| n.parse().ok())
                .collect()
        })
        .unwrap_or_default();
    let default = a.num("default").unwrap_or(0.0);
    let min = a
        .num("minimum")
        .or_else(|| values.iter().copied().reduce(f32::min))
        .unwrap_or(default);
    let max = a
        .num("maximum")
        .or_else(|| values.iter().copied().reduce(f32::max))
        .unwrap_or(default);
    DsAxis {
        tag: a.attr("tag").unwrap_or_default().to_string(),
        name: a.attr("name").unwrap_or_default().to_string(),
        min,
        default,
        max,
        map: a
            .children_named("map")
            .filter_map(|m| Some((m.num("input")?, m.num("output")?)))
            .collect(),
        hidden: matches!(a.attr("hidden"), Some("1" | "true")),
        values,
    }
}

/// A `<location>`'s dimensions as design values.
fn location(el: &Element, axes: &[DsAxis]) -> Vec<(String, f32)> {
    let Some(loc) = el.child("location") else {
        return Vec::new();
    };
    loc.children_named("dimension")
        .filter_map(|d| {
            let name = d.attr("name")?;
            let value = match d.num("xvalue") {
                Some(v) => v,
                None => {
                    let user = d.num("uservalue")?;
                    axes.iter()
                        .find(|a| a.name == name)
                        .map_or(user, |a| user_to_design(&a.map, user))
                }
            };
            Some((name.to_string(), value))
        })
        .collect()
}

/// fontTools' `piecewiseLinearMap`: interpolates between map points and
/// shifts by the nearest end's offset outside them.
fn user_to_design(map: &[(f32, f32)], v: f32) -> f32 {
    let mut map = map.to_vec();
    map.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (Some(&first), Some(&last)) = (map.first(), map.last()) else {
        return v;
    };
    if let Some(&(_, out)) = map.iter().find(|(i, _)| *i == v) {
        return out;
    }
    if v < first.0 {
        return v + first.1 - first.0;
    }
    if v > last.0 {
        return v + last.1 - last.0;
    }
    for w in map.windows(2) {
        let ((a, va), (b, vb)) = (w[0], w[1]);
        if a < v && v < b {
            return va + (vb - va) * (v - a) / (b - a);
        }
    }
    v
}

fn rule(r: &Element, i: usize) -> DsRule {
    let condition = |c: &Element| {
        (
            c.attr("name").unwrap_or_default().to_string(),
            c.num("minimum"),
            c.num("maximum"),
        )
    };
    // format 3 put conditions straight in the rule
    let mut conditions: Vec<_> = r.children_named("condition").map(condition).collect();
    for set in r.children_named("conditionset") {
        conditions.extend(set.children_named("condition").map(condition));
    }
    DsRule {
        name: r
            .attr("name")
            .map_or_else(|| format!("Rule {}", i + 1), str::to_string),
        conditions,
        subs: r
            .children_named("sub")
            .filter_map(|s| Some((s.attr("name")?.to_string(), s.attr("with")?.to_string())))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compare::Status;
    use crate::sources::diff_designspace;

    const V4: &str = r#"<?xml version='1.0' encoding='UTF-8'?>
<designspace format="4.1">
  <axes>
    <axis tag="wght" name="Weight" minimum="100" maximum="900" default="400">
      <map input="100" output="20"/>
      <map input="400" output="80"/>
      <map input="900" output="200"/>
    </axis>
    <axis tag="ital" name="Italic" minimum="0" maximum="1" default="0" hidden="1"/>
  </axes>
  <rules processing="last">
    <rule name="dollar">
      <conditionset>
        <condition name="Weight" minimum="600" maximum="900"/>
      </conditionset>
      <sub name="dollar" with="dollar.bold"/>
    </rule>
    <rule>
      <condition name="Italic" minimum="1"/>
      <sub name="a" with="a.italic"/>
    </rule>
  </rules>
  <sources>
    <source filename="Light.ufo" name="Light" familyname="Test" stylename="Light">
      <location><dimension name="Weight" xvalue="20"/><dimension name="Italic" xvalue="0"/></location>
    </source>
    <source filename="Light.ufo" layer="support">
      <location><dimension name="Weight" xvalue="50"/></location>
    </source>
    <source filename="Bold.ufo" name="Bold" familyname="Test" stylename="Bold">
      <location><dimension name="Weight" xvalue="200"/></location>
    </source>
  </sources>
  <instances>
    <instance name="Regular" familyname="Test" stylename="Regular" filename="instances/Test-Regular.ufo">
      <location><dimension name="Weight" xvalue="80"/></location>
    </instance>
  </instances>
</designspace>"#;

    const V5: &str = r#"<?xml version='1.0' encoding='UTF-8'?>
<designspace format="5.0">
  <axes elidedfallbackname="Regular">
    <axis tag="wght" name="Weight" minimum="100" maximum="900" default="400">
      <map input="100" output="20"/>
      <map input="400" output="80"/>
      <map input="900" output="200"/>
      <labels><label uservalue="400" name="Regular" elidable="true"/></labels>
    </axis>
    <axis tag="ital" name="Italic" values="0 1" default="0"/>
  </axes>
  <sources>
    <source filename="Light.ufo" name="Light">
      <location><dimension name="Weight" xvalue="20"/><dimension name="Italic" xvalue="0"/></location>
    </source>
  </sources>
  <variable-fonts>
    <variable-font name="Test-VF"><axis-subsets><axis-subset name="Weight"/></axis-subsets></variable-font>
  </variable-fonts>
  <instances>
    <instance familyname="Test" stylename="Semi">
      <location><dimension name="Weight" uservalue="650"/><dimension name="Italic" uservalue="1"/></location>
    </instance>
  </instances>
</designspace>"#;

    #[test]
    fn reads_format_4() {
        let d = parse_designspace(V4.as_bytes()).unwrap();
        assert_eq!(d.format, "4.1");
        assert_eq!(d.axes.len(), 2);
        assert_eq!(d.axes[0].map.len(), 3);
        assert!(d.axes[1].hidden);
        assert_eq!(d.sources.len(), 3);
        assert_eq!(d.sources[1].name, "Light.ufo (support)");
        assert_eq!(d.sources[1].layer.as_deref(), Some("support"));
        assert_eq!(
            d.sources[0].location,
            vec![("Weight".to_string(), 20.0), ("Italic".to_string(), 0.0)]
        );
        assert_eq!(d.instances[0].filename, "instances/Test-Regular.ufo");
        assert_eq!(
            d.rules[0].conditions,
            vec![("Weight".to_string(), Some(600.0), Some(900.0))]
        );
        assert_eq!(d.rules[1].name, "Rule 2");
        assert_eq!(
            d.rules[1].subs,
            vec![("a".to_string(), "a.italic".to_string())]
        );
    }

    #[test]
    fn reads_format_5() {
        let d = parse_designspace(V5.as_bytes()).unwrap();
        assert_eq!(d.axes[1].values, vec![0.0, 1.0]);
        assert_eq!((d.axes[1].min, d.axes[1].max), (0.0, 1.0));
        assert_eq!(d.variable_fonts, vec!["Test-VF"]);
        let i = &d.instances[0];
        assert_eq!(i.name, "Test Semi");
        // 650 sits halfway between 400→80 and 900→200
        assert_eq!(
            i.location,
            vec![("Weight".to_string(), 140.0), ("Italic".to_string(), 1.0)]
        );
    }

    #[test]
    fn user_values_extrapolate_by_offset() {
        let map = [(100.0, 20.0), (900.0, 200.0)];
        assert_eq!(user_to_design(&map, 50.0), -30.0);
        assert_eq!(user_to_design(&map, 1000.0), 300.0);
        assert_eq!(user_to_design(&[], 7.0), 7.0);
    }

    #[test]
    fn diffs_two_revisions() {
        let old = parse_designspace(V4.as_bytes()).unwrap();
        let new = parse_designspace(
            V4.replace(
                "maximum=\"900\" default=\"400\"",
                "maximum=\"1000\" default=\"400\"",
            )
            .replace("dollar.bold", "dollar.heavy")
            .replace(
                r#"<source filename="Bold.ufo" name="Bold""#,
                r#"<source filename="Black.ufo" name="Black""#,
            )
            .as_bytes(),
        )
        .unwrap();
        let d = diff_designspace(Some(&old), Some(&new));
        assert_eq!(d.axes[0].1, Status::Modified);
        assert_eq!(d.axes[1].1, Status::Unchanged);
        let statuses: Vec<(&str, Status)> =
            d.sources.iter().map(|(n, s, _)| (n.as_str(), *s)).collect();
        assert_eq!(
            statuses,
            vec![
                ("Light", Status::Unchanged),
                ("Light.ufo (support)", Status::Unchanged),
                ("Bold", Status::Removed),
                ("Black", Status::Added),
            ]
        );
        assert_eq!(d.rules[0].1, Status::Modified);
        assert_eq!(d.instances[0].1, Status::Unchanged);
        let all_added = diff_designspace(None, Some(&new));
        assert!(
            all_added
                .sources
                .iter()
                .all(|(_, s, _)| *s == Status::Added)
        );
    }

    #[test]
    fn malformed_input_is_an_error() {
        assert!(parse_designspace(b"<glyph/>").is_err());
        assert!(parse_designspace(b"<designspace><axes>").is_err());
        let d = parse_designspace(b"<designspace/>").unwrap();
        assert!(d.axes.is_empty() && d.sources.is_empty());
    }
}
