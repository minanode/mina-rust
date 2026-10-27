# Docker image release

The image `ghcr.io/minanode/mina-rust` is built from the signed release
archives, never from source. The public export omits the Docker image workflow.
Until the release owner approves, installs and enables it, use the
[manual image procedure](#manual-image-publication). Publishing a GitHub release
alone does not publish an image.

The workflow sections below describe the proposed automation and apply only
after it is installed and enabled. The release key never goes to CI: the
workflow only verifies `SHA256SUMS.sig` against
[allowed_signers](allowed_signers) from the release commit. The image files are
in [tools/release/docker/](../../tools/release/docker/), and users' instructions
are at <https://minanode.org/docs/docker/>.

## What the proposed workflow does

| Run                                   | Result                                                    |
| ------------------------------------- | --------------------------------------------------------- |
| A release is published                | Verify, build both arches, push, attest, shared tags      |
| A pre-release is published            | Dry run: verify and build both arches, no push            |
| By hand, `push: false`                | Dry run of that release                                   |
| By hand, `push: true`                 | Verify, build, push, attest, shared tags                  |
| The tag no longer on the commit       | Refused before any download                               |
| A commit outside `main`'s history     | Refused before any download                               |
| Any push, pull request or draft event | Nothing: those events don't trigger it                    |
| A draft release                       | Refused: its assets aren't readable with `contents: read` |

First, the plan job fixes the release commit. For a release event, it is the
commit GitHub recorded for the release when it was published; a rerun keeps it.
By hand, it is the `commit` input, the release's full commit ID from its
verified tag (`git rev-parse 'v0.20.0^{commit}'` after `git verify-tag`, as in
[RELEASING.md](RELEASING.md)). The tag must still point to that commit, and the
commit must be in the history of the default branch, so the trust file comes
from reviewed history. The verify steps check the tag again before they
download.

Then it downloads both archives, `SHA256SUMS` and `SHA256SUMS.sig` from the
release. It runs
`ssh-keygen -Y verify -f docs/release/allowed_signers -I release@minanode.org -n file -s SHA256SUMS.sig < SHA256SUMS`
and `sha256sum -c`, and it checks that each archive's `build-info.json` names
exactly the release commit, the version and its target. Any mismatch stops the
run before the build. `allowed_signers` comes from the release commit; the
Dockerfile and build script come from the workflow's own commit.

GitHub runs a release event's workflow from the tagged commit. If the tag is
older than this workflow (as `v0.20.0` will be if it is tagged on a commit
before the Docker files), publishing starts nothing: run the workflow by hand
from `main` with the version, the release commit and `push` ticked.

The publish job pushes and attests only the `VERSION` tag (for example
`0.20.0`). A final job, `shared-tags`, then moves `MAJOR.MINOR` (`0.20`) and
`latest` to that same digest with `docker buildx imagetools create`, which
rebuilds nothing, using
[move-shared-tags.sh](../../tools/release/docker/move-shared-tags.sh):

- `MAJOR.MINOR` moves only if this is the newest published full release in that
  series, and `latest` only if it is the newest overall. The script reads the
  releases from GitHub just before it moves the tags.
- Every run's `shared-tags` job is in one concurrency group, across versions,
  and is never cancelled once started. So a slow run for `0.20.1` that finishes
  after `0.20.2`'s finds `0.20.2` newer and leaves the tags alone; they never
  move back to an older release.
- A pre-release gets only its `VERSION` tag.
- GitHub keeps one waiting run per group: if a third run arrives while one
  waits, the waiting one is cancelled. That can only leave the shared tags where
  they were, never older. If it was the newest release's, run the workflow by
  hand for that version and its commit with `push` ticked.

The publish job has `packages: write`, `id-token: write` and
`attestations: write`, and the `shared-tags` job `packages: write`; the rest of
the workflow has `contents: read`. One run per version at a time, and a push is
never cancelled.

## One-time settings

For the `minanode` organization and the `minanode/mina-rust` repository:

- [ ] **Actions enabled** for the repository: Settings, Actions, General. If the
      organization allows only selected actions, allow `actions/checkout`,
      `actions/attest-build-provenance`, `actions/attest` (which it calls) and
      `docker/setup-buildx-action`.
- [ ] **Package creation allowed**: organization Settings, Packages, Package
      creation, with public packages allowed. The first push creates the
      package, as private.
- [ ] Workflow permissions can stay read-only by default. The push job asks for
      its own write permissions.

## Dry run before release day

A draft release can't be read by the workflow, so rehearse on a published
pre-release, for example `v0.20.0-rc.5`. Sign its `SHA256SUMS` and tag as in
[RELEASING.md](RELEASING.md), then publish it as a pre-release. Publishing runs
the workflow as a dry run. Check that the plan job's summary says `push: false`
and that the dry-run job passes: signature, checksums, build commit, and both
arches built. To repeat a dry run, run the workflow by hand with that version,
its commit and `push` unticked.

## Release day

1. Publish the GitHub release (step 5 of [RELEASING.md](RELEASING.md)). This
   starts the **Docker image** workflow.
2. Check the run: Actions, Docker image, the run for the tag. The publish and
   `shared-tags` jobs must pass. The publish job's summary has the digest, and
   the `shared-tags` summary says which shared tags moved; for the newest
   release, both `0.20` and `latest`.
3. The first time only, open the package (organization, Packages, `mina-rust`),
   then:
   - [ ] Package settings, Change visibility: **Public**.
   - [ ] Check that it is linked to `minanode/mina-rust` (the repository shows
         it under Packages). If not, use **Connect repository**, and give the
         repository's Actions **Write** under Manage Actions access.
4. Record the digest from the summary in the release notes, under the archive
   list:

   ```text
   Docker: ghcr.io/minanode/mina-rust:0.20.0
   (ghcr.io/minanode/mina-rust@sha256:...)
   ```

5. Verify from a clean machine, without logging in:

   ```sh
   gh attestation verify oci://ghcr.io/minanode/mina-rust:0.20.0 \
     -R minanode/mina-rust
   docker buildx imagetools inspect ghcr.io/minanode/mina-rust:0.20.0
   docker run --rm ghcr.io/minanode/mina-rust:0.20.0 --version
   ```

   `imagetools inspect` lists `linux/amd64` and `linux/arm64` and shows the
   digest from the summary. `--version` prints `mina 0.20.0`.

## If the run fails

Fix the cause, then run the workflow by hand with the version, the release
commit and `push` ticked. Never build from unsigned or unverified archives.

## Manual image publication

Without an installed image workflow, the release owner can push from their own
machine after `docker login ghcr.io` with a token that has `write:packages`, and
with `gh` logged in. Push only the `VERSION` tag, then let the same script as
the workflow decide the shared tags. Never pass `--tag ...:0.20` or
`--tag ...:latest` by hand: for an older release, that would move them back.

```sh
tools/release/docker/build-image.sh 0.20.0 release-0.20.0 \
  --push ghcr.io/minanode/mina-rust:0.20.0 --metadata-file meta.json
tools/release/docker/move-shared-tags.sh 0.20.0 \
  --digest "$(jq -r '."containerimage.digest"' meta.json)"
```

`build-image.sh` verifies `SHA256SUMS.sig` and refuses to push without it.
`move-shared-tags.sh --dry-run` shows what would move. Don't run it while the
workflow's `shared-tags` job runs. A push from a laptop has no GitHub build
attestation, so say so in the release notes.

## Rebuild an older release

`docs/release/allowed_signers` can change over time, for example when the
release key is replaced. A release's `SHA256SUMS.sig` is checked against the
file as it was at that release's tag, which is what the workflow does. To build
an older release locally, use the current checkout's tooling with the tag's
signer file:

```sh
git fetch origin tag v0.20.0
git show v0.20.0:docs/release/allowed_signers > allowed_signers-v0.20.0
ssh-keygen -lf allowed_signers-v0.20.0 -E sha256
git -c gpg.ssh.allowedSignersFile=allowed_signers-v0.20.0 verify-tag v0.20.0
tools/release/docker/build-image.sh 0.20.0 release-0.20.0 \
  --signers allowed_signers-v0.20.0
```

Compare the fingerprint with the one published for that release (its release
notes, or the install page at the time) before trusting it. The tag's signature
must verify with the same file. Without `--signers`, the script uses the current
checkout's file, which fails closed for a release signed with an older key.

## Bumping the pinned inputs

The base images, tini, jq (used only while building) and the actions are pinned
by digest, checksum or commit. The Dockerfile's header says how to take new base
image digests and tini and jq checksums. For an action, take the commit of its
new release tag and keep the version comment next to it.
