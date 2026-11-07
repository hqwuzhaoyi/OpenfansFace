#!/usr/bin/env bash
# Run on the Linux NFS client after loading the locally built acceptance image.
# All arguments must identify a NEW, explicitly marked isolated run, never a media directory.
set -euo pipefail
local_root=${1:?local /tmp/face-admin-person-fileops-* path}
media_fixture=${2:?marked NFS .face-admin-rust-fixtures child path}
run_identity=${3:-1000:1000}
injected_errno=${4:-ESTALE}
case "$local_root" in /tmp/face-admin-person-fileops-*) ;; *) exit 2;; esac
case "$media_fixture" in */.face-admin-rust-fixtures/*) ;; *) exit 2;; esac
[[ $(realpath -m "$local_root") == "$local_root" && $(realpath "$media_fixture") == "$media_fixture" ]]
[[ $(cat "$media_fixture/.rust-nfs-fixture") == 'rust nfs acceptance' ]]
[[ $(findmnt -n -T "$media_fixture" -o FSTYPE) == nfs* ]]
[[ ! -e "$local_root/base" && ! -e "$media_fixture/pikpak" ]]
mkdir -p "$local_root"
# Local test DB ownership, never NFS media ownership.
sudo -n chown 1000:1000 "$local_root"
chmod 775 "$local_root"
probe_root=/acceptance-nfs/.face-admin-rust-fixtures/run
sudo -n docker run --rm --network none --read-only \
  --name "face-rust-nfs-${local_root##*/}" --user "$run_identity" \
  --mount "type=bind,src=$local_root,dst=$local_root" \
  --mount "type=bind,src=$media_fixture,dst=$local_root/media" \
  --mount "type=bind,src=$media_fixture,dst=$probe_root" \
  -e "NFS_FIXTURE_ERRNO=$injected_errno" --entrypoint python3 \
  "${NFS_ACCEPTANCE_IMAGE:-face-file-operations:nfs-ticket04}" \
  /acceptance/http-rust-nfs.py "$local_root" "$probe_root"
