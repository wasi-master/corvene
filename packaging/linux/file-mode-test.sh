#!/usr/bin/env bash
# desktop#9443 on drives without executable bits: a repository made where
# file modes are kept, copied onto such a drive, shows files whose only
# change is the mode until Corvene's core.fileMode probe (flag
# 1119-file-mode-probe) runs. Two FUSE mounts, no root or kernel module:
#
#  - a FAT16 image through fusefat: every file reads as 0700 and chmod
#    fails. The probe must see that. (fusefat refuses even git's own chmod
#    of config.lock, so no one can write the config there; the kernel's
#    vfat driver allows that one.)
#  - a bindfs view that reports 0755 for everything and ignores chmod,
#    like a CIFS share mounted with file_mode=0755,noperm or WSL's 9p:
#    the probe sets core.fileMode=false and the status comes out clean.
#
#   packaging/linux/file-mode-test.sh
#
# Needs mkfs.vfat (dosfstools), fusefat, bindfs and fusermount (fuse3).
set -euo pipefail

scratch="$(mktemp -d)"
mounts=()
cleanup() {
    for mnt in "${mounts[@]}"; do
        fusermount3 -u "$mnt" 2>/dev/null || fusermount -u "$mnt" 2>/dev/null || true
    done
    rm -rf "$scratch"
}
trap cleanup EXIT

export GIT_CONFIG_NOSYSTEM=1 GIT_AUTHOR_NAME=T GIT_AUTHOR_EMAIL=t@example.com \
    GIT_COMMITTER_NAME=T GIT_COMMITTER_EMAIL=t@example.com

# the repository, made where file modes are kept
src="$scratch/src"
git init -q -b main "$src"
printf 'text\n' >"$src/readme.txt"
printf '#!/bin/sh\necho hi\n' >"$src/run.sh"
chmod +x "$src/run.sh"
git -C "$src" add .
git -C "$src" -c commit.gpgsign=false commit -q -m init
[[ "$(git -C "$src" config core.fileMode)" == true ]]

check() {
    # $1: the copy, $2: "detect" when the drive cannot be written by git
    local repo="$1"
    echo "  git status on the copy:"
    git -C "$repo" status --porcelain | sed 's/^/    /'
    [[ -n "$(git -C "$repo" status --porcelain)" ]] || {
        echo "expected mode-only changes on the copy" >&2
        exit 1
    }
    CORVENE_FILE_MODE_TEST_REPO="$repo" CORVENE_FILE_MODE_TEST_DETECT_ONLY="${2:+1}" \
        cargo test -q -p corvene-git --lib \
        file_mode::tests::a_drive_without_executable_bits_loses_its_mode_only_changes \
        -- --exact --nocapture 2>&1 | grep -E "^(before|after):|test result|panicked|assert" |
        sed 's/^/  /'
}

echo "1. FAT16 image through fusefat"
truncate -s 32M "$scratch/vfat.img"
mkfs.vfat -n CORVENE "$scratch/vfat.img" >/dev/null
fat="$scratch/fat"
mkdir "$fat"
fusefat -o rw+ "$scratch/vfat.img" "$fat" >/dev/null
mounts+=("$fat")
cp -r "$src" "$fat/repo"
check "$fat/repo" detect
echo "   ok: the probe finds no executable bit"

echo "2. a share that reports 0755 and ignores chmod (bindfs)"
backing="$scratch/backing"
share="$scratch/share"
mkdir "$backing" "$share"
cp -r "$src" "$backing/repo"
bindfs --perms=a+x --chmod-ignore "$backing" "$share"
mounts+=("$share")
check "$share/repo"
[[ "$(git -C "$share/repo" config core.fileMode)" == false ]]
[[ -z "$(git -C "$share/repo" status --porcelain)" ]]
echo "   ok: core.fileMode=false, git status is clean"
