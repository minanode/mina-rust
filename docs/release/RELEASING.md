# Release signing and publication

Use the release owner's machine for signing. The release private key must never
be on the build machine. This procedure uses SSH signatures, namespace `file`
for checksums and Git's `git` namespace for tags. Use OpenSSH 8.2 or newer and
Git with SSH signing support (2.34 or newer). The scripts also need Bash and GNU
`sha256sum`.

## Set up and back up the keys

Recommended: generate the release key in 1Password as an **SSH Key** item, using
Ed25519. Sign through the
[1Password SSH agent](https://developer.1password.com/docs/ssh/agent/). The
private key is never exported to a key file. Enable the agent in 1Password's
**Settings > Developer**. Find the socket path for your platform in the linked
agent setup instructions; set `SSH_AUTH_SOCK` to that path in the signing shell.
Export only the public key to `~/.ssh/minanode-release.pub`. Unlock 1Password
and approve each signing request.

Alternatively, generate a file-based key locally. Enter a strong passphrase when
prompted; do not leave it empty:

```sh
ssh-keygen -t ed25519 -C "minanode release signing" -f ~/.ssh/minanode-release
```

**Blocking checklist, before any signing or GitHub key registration:**

- [ ] Confirm that the release key is backed up, for example in 1Password, and
      that the backup can be recovered.
- [ ] Confirm that the automation commit-signing key is backed up, for example
      in 1Password, and that the backup can be recovered.

Stop if either backup is unconfirmed. Do not use the release key or register any
key on GitHub until both items are complete. Keep these keys separate. Only the
release public key belongs in this repository.

Replace the single pending entry in [allowed_signers](allowed_signers) with the
release public key, keeping `release@minanode.org` as its identity. Record its
SHA-256 fingerprint in `releaseSigningFingerprint` in
`ui/minanode-site/content/release.ts`. Obtain the public key and confirm its
fingerprint through a trusted channel before committing it:

```sh
ssh-keygen -lf ~/.ssh/minanode-release.pub -E sha256
```

The site's `public/allowed_signers` symlink serves that same tracked file. Do
not maintain a second copy. For GitHub's Verified tag badge, the release owner
also adds this public key to their account as a **Signing key**. Local tag
verification below uses the tracked signer file.

## 1. Build on the build machine

Build the reviewed release commit. Retain its full commit ID and the
`build-info.json` files for comparison:

```sh
tools/release/build.sh 0.20.0
```

Build the site with the final-release gate enabled. This is a blocking check: it
fails until the pending signer entry and fingerprint have been replaced. It also
checks that the fingerprint matches the public key:

```sh
RELEASE_FINAL=1 npm run build --prefix ui/minanode-site
```

- [ ] The final-release build passes with the real public key and fingerprint.
- [ ] Archive build metadata identifies the approved commit and a clean tree.

## 2. Create a draft GitHub release

An agent may upload the unsigned artifacts to a **draft** release. Set `commit`
to the full approved build commit ID first. Do not publish the draft:

```sh
gh release create v0.20.0 --repo minanode/mina-rust \
  --draft --target "$commit" --title "0.20.0" \
  --notes-file docs/release/0.20.0.md \
  dist/mina-0.20.0-linux-x86_64.tar.gz \
  dist/mina-0.20.0-linux-arm64.tar.gz dist/SHA256SUMS
```

## 3. Sign on the release owner's machine

Confirm the two backup checklist items above again before signing. Check out the
reviewed tooling and signer file. Authenticated access is needed to download
draft assets. Download the checksum list and, preferably, both archives:

```sh
mkdir -p release-0.20.0
gh release download v0.20.0 --repo minanode/mina-rust \
  --dir release-0.20.0 --pattern SHA256SUMS --pattern '*.tar.gz'
```

For 1Password, pass the exported public key. The script checks that the agent
lists it, verifies all supplied archives, signs the list, verifies that
signature, creates a signed tag at the explicit build commit and verifies it:

```sh
tools/release/sign-release.sh release-0.20.0 \
  ~/.ssh/minanode-release.pub --tag v0.20.0 --commit "$commit"
```

For a file key, pass `~/.ssh/minanode-release` instead. To sign only the
checksum list, omit the archives. The script still runs the checksum check, but
reports that no archive contents were checked. Confirm the list against the
reviewed build output. Omit both tag options if a signed tag already exists.
Existing signature files and tags are never overwritten.

Push only the signed tag, then upload the verified checksum signature to the
existing draft. Do not let publication create an unsigned tag:

```sh
git push https://github.com/minanode/mina-rust refs/tags/v0.20.0
gh release upload v0.20.0 release-0.20.0/SHA256SUMS.sig \
  --repo minanode/mina-rust
```

## 4. Verify from a clean machine

Before publishing, use authenticated access to download the draft's archive,
`SHA256SUMS` and `SHA256SUMS.sig`. Obtain `allowed_signers` from the reviewed
site build and compare its fingerprint through a trusted channel. A draft's
public release-download URL will not work yet. Fetch the tag into a clean
checkout of the public repository and verify it:

```sh
git fetch origin tag v0.20.0
git -c gpg.ssh.allowedSignersFile=docs/release/allowed_signers \
  verify-tag v0.20.0
git rev-parse 'v0.20.0^{commit}'
```

Confirm that this is the build commit. In the downloaded assets directory:

```sh
ssh-keygen -lf allowed_signers -E sha256
ssh-keygen -Y verify -f allowed_signers -I release@minanode.org \
  -n file -s SHA256SUMS.sig < SHA256SUMS &&
sha256sum --check --ignore-missing SHA256SUMS
```

- [ ] The real signer and fingerprint match the reviewed source.
- [ ] The tag signature is valid and targets the build commit.
- [ ] The checksum signature is valid and every downloaded archive reports OK.
- [ ] Re-run the final-release site gate; neither signing marker remains.

Stop on any failure. Never replace a failed check with an unsigned artifact.

## 5. Publish

Only the release owner or coordinator approves publication after the checks:

```sh
gh release edit v0.20.0 --repo minanode/mina-rust --draft=false
```

Check the public release downloads and `https://minanode.org/allowed_signers`
from a clean machine using the
[install guide](https://minanode.org/docs/install/#check).

The public export omits the Docker image workflow. Publishing a release does not
publish an image automatically. The release owner can use the
[manual image procedure](DOCKER.md#manual-image-publication) to build
`ghcr.io/minanode/mina-rust` from these signed archives. Installing and enabling
the proposed workflow requires a separate owner decision.

Signature formats and configuration:
[OpenSSH](https://man.openbsd.org/ssh-keygen#ALLOWED_SIGNERS) and
[Git](https://git-scm.com/docs/git-config#Documentation/git-config.txt-gpgformat).
