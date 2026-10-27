#!/usr/bin/env bash
# All signing material and the agent in this test are disposable.
set -euo pipefail
umask 077
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
mkdir -p "$repo/.work/SIGN1"
test_dir=$(mktemp -d "$repo/.work/SIGN1/sign-test.XXXXXXXX")
agent_pid=''
cleanup() {
  if [[ -n $agent_pid ]]; then kill "$agent_pid" 2>/dev/null || true; wait "$agent_pid" 2>/dev/null || true; fi
  rm -rf "$test_dir"
}
trap cleanup EXIT
mkdir -p "$test_dir/repo/tools/release" "$test_dir/repo/docs/release" "$test_dir/files"
cp "$repo/tools/release/sign-release.sh" "$test_dir/repo/tools/release/"
ssh-keygen -q -t ed25519 -N '' -C 'release signing test' -f "$test_dir/key"
{ printf 'release@minanode.org '; cat "$test_dir/key.pub"; } > "$test_dir/repo/docs/release/allowed_signers"
ssh-keygen -lf "$test_dir/repo/docs/release/allowed_signers" -E sha256
printf 'release archive fixture\n' > "$test_dir/files/archive.tar.gz"
(cd "$test_dir/files" && sha256sum archive.tar.gz > SHA256SUMS)
sign="$test_dir/repo/tools/release/sign-release.sh"
"$sign" "$test_dir/files" "$test_dir/key"
printf 'tampered\n' >> "$test_dir/files/SHA256SUMS"
if ssh-keygen -Y verify -f "$test_dir/repo/docs/release/allowed_signers" \
  -I release@minanode.org -n file -s "$test_dir/files/SHA256SUMS.sig" < "$test_dir/files/SHA256SUMS"; then
  echo 'accepted a tampered manifest' >&2; exit 1
fi
(cd "$test_dir/files" && sha256sum archive.tar.gz > SHA256SUMS)
rm "$test_dir/files/SHA256SUMS.sig"
printf 'tampered archive\n' >> "$test_dir/files/archive.tar.gz"
if "$sign" "$test_dir/files" "$test_dir/key"; then echo 'accepted a corrupt archive' >&2; exit 1; fi
[[ ! -f $test_dir/files/SHA256SUMS.sig ]]
rm "$test_dir/files/archive.tar.gz"
"$sign" "$test_dir/files" "$test_dir/key"
rm "$test_dir/files/SHA256SUMS.sig"
# Isolate the test agent: no access to the caller's agent or signing identity.
export SSH_AUTH_SOCK="$test_dir/agent.sock"
ssh-agent -D -a "$SSH_AUTH_SOCK" > "$test_dir/agent.log" 2>&1 &
agent_pid=$!
for _ in {1..30}; do [[ ! -S $SSH_AUTH_SOCK ]] || break; sleep 0.1; done
if "$sign" "$test_dir/files" "$test_dir/key.pub"; then echo 'accepted an empty agent' >&2; exit 1; fi
ssh-add "$test_dir/key"
ssh-keygen -q -t ed25519 -N '' -C 'other signing test' -f "$test_dir/other"
if "$sign" "$test_dir/files" "$test_dir/other.pub"; then echo 'accepted the wrong agent key' >&2; exit 1; fi
# The test commit and tag are also signed, in a separate throwaway repository.
git -C "$test_dir/repo" init -q
git -C "$test_dir/repo" config user.name 'Signing test'
git -C "$test_dir/repo" config user.email 'release@minanode.org'
git -C "$test_dir/repo" -c gpg.format=ssh -c user.signingkey="$test_dir/key" \
  -c commit.gpgsign=true commit -q --allow-empty -m 'Signing test fixture'
rm "$test_dir/key"
"$sign" "$test_dir/files" "$test_dir/key.pub" --tag v0.0.0-test --commit HEAD
ssh-keygen -Y verify -f "$test_dir/repo/docs/release/allowed_signers" \
  -I release@minanode.org -n file -s "$test_dir/files/SHA256SUMS.sig" < "$test_dir/files/SHA256SUMS"
git -C "$test_dir/repo" -c gpg.ssh.allowedSignersFile="$test_dir/repo/docs/release/allowed_signers" \
  verify-tag v0.0.0-test
rm "$test_dir/files/SHA256SUMS.sig"
ssh-add -D
if "$sign" "$test_dir/files" "$test_dir/key.pub"; then echo 'accepted an unloaded agent key' >&2; exit 1; fi
printf '%s\n' 'release@minanode.org ssh-ed25519 RELEASE_KEY_PENDING' > "$test_dir/repo/docs/release/allowed_signers"
if "$sign" "$test_dir/files" "$test_dir/key.pub"; then echo 'accepted a signer placeholder' >&2; exit 1; fi
echo 'PASS: file/agent signatures, signed tag, checksum-only mode and negative cases'
