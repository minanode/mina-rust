#!/usr/bin/env bash
# Stage release licence material into an archive root.
set -euo pipefail
[[ $# -eq 1 && -d $1 ]] || { echo 'usage: stage-licenses.sh STAGE_DIR' >&2; exit 2; }
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
stage=$1
for file in LICENSE THIRD_PARTY_NOTICES THIRD_PARTY_LICENSES \
    SOURCES/option-ext-0.2.0.crate; do
  [[ -f $repo/$file ]] || { echo "missing release licence file: $file" >&2; exit 1; }
  mkdir -p "$stage/$(dirname "$file")"
  cp "$repo/$file" "$stage/$file"
done
