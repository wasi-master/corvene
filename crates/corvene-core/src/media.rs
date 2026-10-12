//! Corvene `1335-video-diff` / `1336-audio-diff` / `1337-media-ffmpeg`:
//! installs `corvene_media`'s probe in `corvene_git` while a flag is on.

use crate::flags::{Flags, ids};

pub fn set_media_diffs(flags: &Flags) {
    corvene_media::set_ffmpeg_enabled(flags.bool(ids::MEDIA_FFMPEG));
    let video = flags.bool(ids::VIDEO_DIFF);
    let audio = flags.bool(ids::AUDIO_DIFF);
    let probe: corvene_git::media_file::MediaProber = corvene_media::probe;
    corvene_git::media_file::set_media_diffs((video || audio).then_some(probe), video, audio);
}
