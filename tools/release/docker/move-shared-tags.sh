#!/usr/bin/env bash
# Point the shared tags, MAJOR.MINOR and latest, at an already pushed image,
# but only where VERSION is the newest published full release for that tag:
# MAJOR.MINOR for the newest release in its minor series, latest for the
# newest overall. The releases are read from GitHub just before the tags
# move, so a slow run for an older release never moves them back.
#
# The tags are moved with `docker buildx imagetools create`, which copies no
# layers and builds nothing: the shared tag gets the same digest (and so the
# same attestations) as VERSION's.
#
# Run after the VERSION tag is pushed, and never two at once (the workflow
# runs it in one concurrency group). Needs gh (GH_TOKEN or `gh auth login`),
# and docker buildx logged in to the registry. Runs on Linux and macOS.
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
usage: move-shared-tags.sh VERSION --digest DIGEST [options]

  --digest DIGEST   the digest VERSION was pushed with (sha256:...), from
                    build-image.sh --metadata-file (containerimage.digest)

  --image NAME      the image (default ghcr.io/minanode/mina-rust)
  --repo OWNER/NAME the GitHub repository (default minanode/mina-rust)
  --dry-run         say what would move, move nothing
EOF
  exit 2
}

[ $# -ge 1 ] || usage
version=$1
shift
image=ghcr.io/minanode/mina-rust repo=minanode/mina-rust digest='' dry_run=''
while [ $# -gt 0 ]; do
  case $1 in
    --image) [ $# -ge 2 ] || usage; image=$2; shift 2 ;;
    --repo) [ $# -ge 2 ] || usage; repo=$2; shift 2 ;;
    --digest) [ $# -ge 2 ] || usage; digest=$2; shift 2 ;;
    --dry-run) dry_run=1; shift ;;
    *) usage ;;
  esac
done

echo "$digest" | grep -Eq '^sha256:[0-9a-f]{64}$' ||
  { echo "--digest sha256:... is required" >&2; exit 2; }

if ! echo "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
  echo "$version is not a full release (X.Y.Z): no shared tags to move"
  exit 0
fi

# The published full releases, as X.Y.Z, newest last.
releases=$(gh release list --repo "$repo" --limit 1000 --exclude-drafts \
  --exclude-pre-releases --json tagName --jq '.[].tagName' |
  sed -n 's/^v\([0-9]*\.[0-9]*\.[0-9]*\)$/\1/p' |
  sort -t. -k1,1n -k2,2n -k3,3n)
if ! printf '%s\n' "$releases" | grep -Fqx "$version"; then
  echo "v$version is not a published full release of $repo" >&2
  exit 1
fi
minor=$(echo "$version" | cut -d. -f1,2)
newest=$(printf '%s\n' "$releases" | tail -n 1)
newest_minor=$(printf '%s\n' "$releases" |
  awk -F. -v m="$minor" '$1 "." $2 == m' | tail -n 1)

move() {
  if [ -n "$dry_run" ]; then
    echo "would move $image:$1 to $image@$digest ($version)"
  else
    docker buildx imagetools create --tag "$image:$1" "$image@$digest"
    echo "moved $image:$1 to $image@$digest ($version)"
  fi
}
moved=0
if [ "$version" = "$newest_minor" ]; then
  move "$minor"; moved=$((moved + 1))
else
  echo "left $image:$minor: v$newest_minor is the newest $minor release"
fi
if [ "$version" = "$newest" ]; then
  move latest; moved=$((moved + 1))
else
  echo "left $image:latest: v$newest is the newest release"
fi
echo "$moved shared tag(s) moved for $version"
