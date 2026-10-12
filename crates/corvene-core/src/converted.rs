//! Corvene `1350`-`1357`: the flags of the converted diffs
//! (`corvene_git::converted`) and of the archive diff's package and image
//! formats, handed to the git layer.

use corvene_models::ConvertedKind;

use crate::flags::{Flags, ids};

/// The flag of each kind of converted diff.
pub fn flag_of(kind: ConvertedKind) -> crate::flags::FlagId {
    match kind {
        ConvertedKind::Compiled => ids::COMPILED_CODE_DIFF,
        ConvertedKind::Serialized => ids::SERIALIZED_DATA_DIFF,
        ConvertedKind::Translation => ids::TRANSLATION_DIFF,
        ConvertedKind::Diagram => ids::DIAGRAM_DIFF,
        ConvertedKind::Map => ids::MAP_DIFF,
        ConvertedKind::Email => ids::EMAIL_DIFF,
    }
}

/// Hands the flags to the git layer.
pub(crate) fn sync_flags(flags: &Flags) {
    let kinds: Vec<ConvertedKind> = ConvertedKind::ALL
        .into_iter()
        .filter(|k| flags.bool(flag_of(*k)))
        .collect();
    corvene_git::converted::set_converted_kinds(&kinds);
    corvene_git::archive::set_read_more_archives(
        flags.bool(ids::PACKAGE_ARCHIVE_DIFF),
        flags.bool(ids::DISK_IMAGE_DIFF),
    );
}

/// The flags whose change means the diffs on screen read again.
pub(crate) const FLAGS: [crate::flags::FlagId; 8] = [
    ids::COMPILED_CODE_DIFF,
    ids::SERIALIZED_DATA_DIFF,
    ids::TRANSLATION_DIFF,
    ids::DIAGRAM_DIFF,
    ids::MAP_DIFF,
    ids::EMAIL_DIFF,
    ids::PACKAGE_ARCHIVE_DIFF,
    ids::DISK_IMAGE_DIFF,
];
