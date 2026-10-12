//! The Info tab: what each version holds, side by side, with the rows that
//! differ marked and the cover art of both.

use std::sync::Arc;

use corvene_core::{MediaDecoder, MediaInfo, MediaSide};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{MediaDiffView, format_time};
use crate::format::format_bytes;
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;

fn rate(bits: u64) -> String {
    if bits >= 1_000_000 {
        format!("{:.2} Mb/s", bits as f64 / 1e6)
    } else {
        format!("{:.0} kb/s", bits as f64 / 1e3)
    }
}

fn decoder(d: MediaDecoder) -> &'static str {
    match d {
        MediaDecoder::None => "None",
        MediaDecoder::System if cfg!(target_os = "macos") => "AVFoundation",
        MediaDecoder::System if cfg!(windows) => "Media Foundation",
        MediaDecoder::System => "System",
        MediaDecoder::Symphonia => "Symphonia",
        MediaDecoder::Ffmpeg => "ffmpeg",
    }
}

/// The rows of one version, by label.
fn rows(side: &MediaSide) -> Vec<(String, String)> {
    let i: &MediaInfo = &side.info;
    let mut rows: Vec<(String, String)> = vec![
        ("Container".into(), i.container.clone()),
        (
            "Duration".into(),
            i.duration_us
                .map(|us| format_time(us as f64 / 1e6))
                .unwrap_or_default(),
        ),
        ("File size".into(), format_bytes(side.bytes as i64, 2)),
        ("Bit rate".into(), i.bit_rate.map(rate).unwrap_or_default()),
    ];
    if let Some(v) = &i.video {
        let (w, h) = v.display_size();
        rows.extend([
            ("Video codec".into(), v.codec.clone()),
            ("Resolution".into(), format!("{w} \u{00d7} {h}")),
            (
                "Frame rate".into(),
                v.fps()
                    .map(|f| {
                        let text = format!("{f:.3}");
                        format!("{} fps", text.trim_end_matches('0').trim_end_matches('.'))
                    })
                    .unwrap_or_default(),
            ),
            (
                "Frames".into(),
                v.frames.map(|f| f.to_string()).unwrap_or_default(),
            ),
            (
                "Rotation".into(),
                if v.rotation == 0 {
                    String::new()
                } else {
                    format!("{}\u{00b0}", v.rotation)
                },
            ),
            (
                "Pixel format".into(),
                v.pixel_format.clone().unwrap_or_default(),
            ),
            (
                "Video bit rate".into(),
                v.bit_rate.map(rate).unwrap_or_default(),
            ),
            ("Video decoder".into(), decoder(i.video_decoder).into()),
        ]);
    }
    if let Some(a) = &i.audio {
        rows.extend([
            ("Audio codec".into(), a.codec.clone()),
            ("Sample rate".into(), format!("{} Hz", a.sample_rate)),
            (
                "Channels".into(),
                match a.channels {
                    1 => "Mono".to_string(),
                    2 => "Stereo".to_string(),
                    n => n.to_string(),
                },
            ),
            (
                "Bit depth".into(),
                a.bits_per_sample
                    .map(|b| format!("{b} bit"))
                    .unwrap_or_default(),
            ),
            (
                "Audio bit rate".into(),
                a.bit_rate.map(rate).unwrap_or_default(),
            ),
            ("Audio decoder".into(), decoder(i.audio_decoder).into()),
        ]);
    }
    for (k, v) in &i.tags {
        let mut label = k.replace('_', " ");
        if let Some(first) = label.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        rows.push((label, v.clone()));
    }
    rows.retain(|(_, v)| !v.is_empty());
    rows
}

pub(super) fn info(view: &mut MediaDiffView, cx: &mut Context<MediaDiffView>) -> AnyElement {
    let t = cx.ghd().clone();
    let old = view.diff.previous.as_ref().map(rows).unwrap_or_default();
    let new = view.diff.current.as_ref().map(rows).unwrap_or_default();
    // labels in the order they first appear
    let mut labels: Vec<String> = Vec::new();
    for (k, _) in old.iter().chain(&new) {
        if !labels.contains(k) {
            labels.push(k.clone());
        }
    }
    let value = |rows: &[(String, String)], k: &str| {
        rows.iter().find(|(l, _)| l == k).map(|(_, v)| v.clone())
    };
    let both = view.compares();
    let header = |text: &str, color: Hsla| {
        div()
            .flex_1()
            .min_w_0()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(color)
            .child(text.to_string())
    };
    let table = div()
        .flex()
        .flex_col()
        .w_full()
        .max_w(zpx(820.))
        .child(
            div()
                .flex()
                .flex_row()
                .gap(SPACING())
                .py(zpx(6.))
                .border_b_1()
                .border_color(t.box_border)
                .child(div().w(zpx(150.)).flex_none())
                .when(view.diff.previous.is_some(), |d| {
                    d.child(header("Old", t.color_deleted))
                })
                .when(view.diff.current.is_some(), |d| {
                    d.child(header("New", t.color_new))
                }),
        )
        .children(labels.iter().enumerate().map(|(ix, label)| {
            let a = value(&old, label);
            let b = value(&new, label);
            let changed = both && a != b;
            let cell = |v: Option<String>, color: Hsla| {
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(color)
                    .child(v.unwrap_or_else(|| "\u{2014}".into()))
            };
            div()
                .id(("media-info-row", ix))
                .flex()
                .flex_row()
                .gap(SPACING())
                .py(zpx(4.))
                .border_b_1()
                .border_color(t.box_border)
                .when(changed, |d| d.bg(t.color_modified.opacity(0.08)))
                .child(
                    div()
                        .w(zpx(150.))
                        .flex_none()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(zpx(6.))
                        .text_color(t.text_secondary)
                        .child(
                            div()
                                .size(zpx(6.))
                                .rounded_full()
                                .when(changed, |d| d.bg(t.color_modified)),
                        )
                        .child(label.clone()),
                )
                .when(view.diff.previous.is_some(), |d| {
                    d.child(cell(a, if changed { t.color_deleted } else { t.text }))
                })
                .when(view.diff.current.is_some(), |d| {
                    d.child(cell(b, if changed { t.color_new } else { t.text }))
                })
        }));
    let covers: Vec<(Option<Arc<Image>>, &str, Hsla)> = [
        (view.diff.previous.as_ref(), "Old cover", t.color_deleted),
        (view.diff.current.as_ref(), "New cover", t.color_new),
    ]
    .into_iter()
    .filter_map(|(side, label, color)| {
        let side = side?;
        let cover = side.info.cover.as_ref().map(|c| {
            let format = match c.media_type.as_str() {
                "image/png" => ImageFormat::Png,
                "image/gif" => ImageFormat::Gif,
                "image/webp" => ImageFormat::Webp,
                "image/bmp" => ImageFormat::Bmp,
                _ => ImageFormat::Jpeg,
            };
            Arc::new(Image::from_bytes(format, c.bytes.clone()))
        });
        Some((cover, label, color))
    })
    .collect();
    let any_cover = covers.iter().any(|(c, _, _)| c.is_some());
    div()
        .id("media-info")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .px(SPACING_DOUBLE())
        .py(SPACING())
        .flex()
        .flex_col()
        .items_center()
        .gap(SPACING_DOUBLE())
        .child(table)
        .when(any_cover, |d| {
            d.child(
                div()
                    .flex()
                    .flex_row()
                    .gap(SPACING_DOUBLE())
                    .children(covers.into_iter().map(|(cover, label, color)| {
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(zpx(4.))
                            .child(
                                div()
                                    .size(zpx(140.))
                                    .border_1()
                                    .border_color(color)
                                    .bg(t.box_alt_background)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(FONT_SIZE_SM())
                                    .text_color(t.text_secondary)
                                    .child(match cover {
                                        Some(image) => img(image)
                                            .size_full()
                                            .object_fit(ObjectFit::Contain)
                                            .into_any_element(),
                                        None => "No cover".into_any_element(),
                                    }),
                            )
                            .child(div().text_color(color).child(label))
                    })),
            )
        })
        .into_any_element()
}
