//! Real files made by an installed `ffmpeg` (the tests pass without
//! checking anything when there is none).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;

use corvene_media::{MediaDecoder, MediaKind, Source, analysis, probe};

fn ffmpeg() -> Option<PathBuf> {
    [
        "/opt/homebrew/bin/ffmpeg",
        "/usr/local/bin/ffmpeg",
        "/usr/bin/ffmpeg",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|p| p.is_file())
}

fn make(dir: &Path, name: &str, args: &[&str]) -> Option<PathBuf> {
    let out = dir.join(name);
    let ok = Command::new(ffmpeg()?)
        .args(["-v", "error", "-y"])
        .args(args)
        .arg(&out)
        .status()
        .ok()?
        .success();
    ok.then_some(out)
}

/// A 2 s 320×240 test pattern with a 440 Hz tone, then the same with the
/// last second's picture inverted.
fn videos(dir: &Path, ext: &str, codec: &[&str]) -> Option<(PathBuf, PathBuf)> {
    let base = [
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=320x240:rate=25:duration=2",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=2",
    ];
    let mut a: Vec<&str> = base.to_vec();
    a.extend_from_slice(codec);
    let old = make(dir, &format!("old.{ext}"), &a)?;
    let mut b: Vec<&str> = base.to_vec();
    b.extend_from_slice(&["-vf", "negate=enable='gte(t,1)'"]);
    b.extend_from_slice(codec);
    let new = make(dir, &format!("new.{ext}"), &b)?;
    Some((old, new))
}

#[test]
fn mp4_probes_reads_and_compares() {
    let dir = tempfile::tempdir().unwrap();
    let Some((old, new)) = videos(
        dir.path(),
        "mp4",
        &[
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
        ],
    ) else {
        eprintln!("no ffmpeg");
        return;
    };
    let info = probe(&old, MediaKind::Video).expect("probe");
    let video = info.video.as_ref().expect("video stream");
    assert_eq!((video.width, video.height), (320, 240));
    assert!(video.codec.contains("264"), "{}", video.codec);
    assert!(info.audio.is_some());
    let d = info.duration_us.unwrap() as f64 / 1e6;
    assert!((1.9..2.2).contains(&d), "{d}");
    if cfg!(target_os = "macos") {
        assert_eq!(info.video_decoder, MediaDecoder::System);
    }
    let src = Source::new(&old, info.clone());
    let frame = src.frame_at(0.5, (160, 160)).expect("frame");
    assert_eq!((frame.width, frame.height), (160, 120));
    let mut reader = src.video_from(1.0, (160, 160)).expect("reader");
    let first = reader.next_frame().unwrap().expect("first frame");
    assert!((first.time - 1.0).abs() < 0.05, "{}", first.time);
    let second = reader.next_frame().unwrap().expect("second frame");
    assert!(second.time > first.time);
    let mut sound = src.audio_from(0.5).expect("sound");
    let chunk = sound.next_chunk().unwrap().expect("chunk");
    assert!((chunk.time - 0.5).abs() < 0.05, "{}", chunk.time);
    assert!(chunk.samples.iter().any(|s| s.abs() > 0.1));

    // the pictures differ in the second half only
    let new_src = Source::new(&new, probe(&new, MediaKind::Video).unwrap());
    let mut last = None;
    analysis::VideoComparison::sample(
        src.duration(),
        Some(&src),
        Some(&new_src),
        &AtomicBool::new(false),
        &mut |c| last = Some(c.clone()),
    );
    let cmp = last.unwrap();
    assert_eq!(cmp.done, cmp.len());
    let changes = cmp.changes();
    assert_eq!(changes.len(), 1, "{changes:?} {:?}", cmp.difference);
    assert!((0.9..=1.3).contains(&changes[0].0), "{changes:?}");

    // the same sound in both
    let cancel = AtomicBool::new(false);
    let a = analysis::summarize_audio(&src, &cancel, &mut |_| {})
        .unwrap()
        .unwrap();
    let b = analysis::summarize_audio(&new_src, &cancel, &mut |_| {})
        .unwrap()
        .unwrap();
    let audio = analysis::compare_audio(Some(&a), Some(&b), 200);
    assert!(audio.changes.is_empty(), "{:?}", audio.changes);
}

#[test]
fn webm_falls_back_to_ffmpeg() {
    let dir = tempfile::tempdir().unwrap();
    let Some((old, _)) = videos(
        dir.path(),
        "webm",
        &[
            "-c:v",
            "libvpx-vp9",
            "-b:v",
            "200k",
            "-c:a",
            "libopus",
            "-shortest",
        ],
    ) else {
        eprintln!("no ffmpeg or no vp9/opus");
        return;
    };
    let info = probe(&old, MediaKind::Video).expect("probe");
    assert_eq!(info.video_decoder, MediaDecoder::Ffmpeg);
    assert_eq!(info.audio_decoder, MediaDecoder::Ffmpeg);
    let src = Source::new(&old, info);
    let frame = src.frame_at(1.5, (320, 320)).expect("frame");
    assert_eq!((frame.width, frame.height), (320, 240));
    let mut reader = src.video_from(0.0, (64, 64)).unwrap();
    let mut n = 0;
    while reader.next_frame().unwrap().is_some() {
        n += 1;
    }
    assert!((45..=55).contains(&n), "{n}");
}

#[test]
fn audio_files_by_symphonia_with_tags_and_a_found_edit() {
    let dir = tempfile::tempdir().unwrap();
    let Some(old) = make(
        dir.path(),
        "old.flac",
        &[
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=3",
            "-metadata",
            "title=Tone",
            "-metadata",
            "artist=Corvene",
        ],
    ) else {
        eprintln!("no ffmpeg");
        return;
    };
    // 1 s to 1.5 s at 880 Hz
    let new = make(
        dir.path(),
        "new.mp3",
        &[
            "-filter_complex",
            "sine=f=440:d=1[a];sine=f=880:d=0.5[b];sine=f=440:d=1.5[c];[a][b][c]concat=n=3:v=0:a=1[out]",
            "-map",
            "[out]",
            "-b:a",
            "192k",
        ],
    )
    .unwrap();
    let info = probe(&old, MediaKind::Audio).expect("probe flac");
    assert_eq!(info.audio_decoder, MediaDecoder::Symphonia);
    assert!(
        info.tags.iter().any(|(k, v)| k == "title" && v == "Tone"),
        "{:?}",
        info.tags
    );
    let new_info = probe(&new, MediaKind::Audio).expect("probe mp3");
    assert_eq!(new_info.audio_decoder, MediaDecoder::Symphonia);
    let d = new_info.duration_us.unwrap() as f64 / 1e6;
    assert!((2.9..3.2).contains(&d), "{d}");
    let cancel = AtomicBool::new(false);
    let a = analysis::summarize_audio(&Source::new(&old, info), &cancel, &mut |_| {})
        .unwrap()
        .unwrap();
    let b = analysis::summarize_audio(&Source::new(&new, new_info.clone()), &cancel, &mut |_| {})
        .unwrap()
        .unwrap();
    let cmp = analysis::compare_audio(Some(&a), Some(&b), 300);
    assert!(
        cmp.changes.iter().any(|(s, e)| *s < 1.1 && *e > 1.4),
        "{:?}",
        cmp.changes
    );
    // seeking lands where asked
    let mut r = Source::new(&new, new_info).audio_from(2.0).unwrap();
    let c = r.next_chunk().unwrap().unwrap();
    assert!((c.time - 2.0).abs() < 0.05, "{}", c.time);
}
