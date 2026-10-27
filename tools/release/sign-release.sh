#!/usr/bin/env bash
# Sign release checksums locally, using a file key or an SSH agent public key.
set -euo pipefail
umask 077

fail() { printf 'sign-release: %s\n' "$*" >&2; exit 1; }
usage() {
  echo 'usage: sign-release.sh DIRECTORY KEY[.pub] [--tag TAG --commit COMMIT]'
}
[[ ${1:-} != --help ]] || { usage; exit 0; }
[[ $# -ge 2 ]] || { usage >&2; exit 2; }
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
directory=$(cd "$1" && pwd)
key=$2
[[ $key == /* ]] || key="$PWD/$key"
shift 2
tag='' commit=''
while [[ $# -gt 0 ]]; do
  case $1 in
    --tag|--commit)
      [[ $# -ge 2 && -n $2 ]] || fail "missing value for $1"
      if [[ $1 == --tag ]]; then tag=$2; else commit=$2; fi
      shift 2 ;;
    *) usage >&2; exit 2 ;;
  esac
done
[[ -z $tag && -z $commit || -n $tag && -n $commit ]] ||
  fail '--tag and --commit must be supplied together'
for command in sha256sum ssh-keygen; do
  command -v "$command" >/dev/null || fail "required command not found: $command"
done
manifest="$directory/SHA256SUMS"
signers="$repo/docs/release/allowed_signers"
[[ -f $manifest ]] || fail 'SHA256SUMS is missing'
[[ ! -e $manifest.sig ]] || fail 'SHA256SUMS.sig already exists; move it aside first'
snapshot=$(mktemp "$directory/.SHA256SUMS.sign.XXXXXXXX")
trap 'rm -f "$snapshot" "$snapshot.sig"' EXIT
cp "$manifest" "$snapshot"
# Release manifests contain simple archive basenames, not paths or escaped names.
pattern='^[[:xdigit:]]{64} [ *]([[:alnum:]][[:alnum:]._-]*\.tar\.gz)$'
entries=0 present=0
while IFS= read -r line || [[ -n $line ]]; do
  [[ $line =~ $pattern ]] || fail 'invalid SHA256SUMS archive entry'
  entries=$((entries + 1))
  [[ ! -f $directory/${BASH_REMATCH[1]} ]] || present=$((present + 1))
done < "$snapshot"
[[ $entries -gt 0 ]] || fail 'SHA256SUMS is empty'
# Always run the checksum check first. GNU sha256sum returns 1 when no file exists.
if (cd "$directory" && sha256sum --check --ignore-missing "${snapshot##*/}"); then
  printf 'Checked %s of %s listed archives.\n' "$present" "$entries"
else
  checksum_status=$?
  [[ $present -eq 0 && $checksum_status -eq 1 ]] || fail 'archive checksum check failed'
  for archive in "$directory"/*.tar.gz; do
    [[ ! -e $archive ]] || fail 'archives are present but none match SHA256SUMS'
  done
  echo 'Checksum-list-only signing: no archives supplied; archive contents were not checked.' >&2
fi
[[ -f $signers ]] || fail 'docs/release/allowed_signers is missing'
! grep -Eq 'RELEASE_KEY_PENDING|PLACEHOLDER' "$signers" ||
  fail 'replace the allowed_signers placeholder before signing'
if [[ $key == *.pub ]]; then
  command -v ssh-add >/dev/null || fail 'ssh-add is needed for an agent key'
  agent_keys=$(ssh-add -L 2>/dev/null) ||
    fail 'SSH agent is unreachable or empty; set SSH_AUTH_SOCK and unlock/load the release key'
  fingerprint=$(ssh-keygen -lf "$key" -E sha256 | awk '{print $2}')
  [[ -n $fingerprint ]] || fail 'cannot read public key fingerprint'
  agent_fingerprints=$(printf '%s\n' "$agent_keys" | ssh-keygen -lf /dev/stdin -E sha256 | awk '{print $2}')
  grep -Fxq "$fingerprint" <<< "$agent_fingerprints" ||
    fail 'SSH agent does not list this public key; unlock/load the release key'
fi
if [[ -n $tag ]]; then
  git -C "$repo" check-ref-format "refs/tags/$tag" >/dev/null || fail 'invalid tag name'
  commit=$(git -C "$repo" rev-parse --verify "$commit^{commit}")
  ! git -C "$repo" show-ref --verify --quiet "refs/tags/$tag" || fail 'tag already exists'
fi
ssh-keygen -Y sign -f "$key" -n file "$snapshot"
ssh-keygen -Y verify -f "$signers" -I release@minanode.org \
  -n file -s "$snapshot.sig" < "$snapshot"
cmp -s "$snapshot" "$manifest" || fail 'SHA256SUMS changed during signing'
mv "$snapshot.sig" "$manifest.sig"
if [[ -n $tag ]]; then
  git -C "$repo" -c gpg.format=ssh -c user.signingkey="$key" \
    tag -s "$tag" "$commit" -m "Release $tag"
  git -C "$repo" -c gpg.ssh.allowedSignersFile="$signers" verify-tag "$tag"
fi
printf 'Verified signature written to %s\n' "$manifest.sig"
echo 'Next: check the signed tag targets the build commit, push that tag explicitly,'
echo 'upload SHA256SUMS.sig to the draft release, verify on a clean machine, then publish.'
echo 'See docs/release/RELEASING.md. This script does not push, upload or publish.'
