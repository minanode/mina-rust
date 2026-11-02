#!/usr/bin/env bash
# Validate each OCaml reference at its pinned commit, not a moving release.
# --drift reports whole-file changes against --branch (4.0.4 by default).
# --update explicitly repins only byte-identical files, after validation passes.
set -euo pipefail

OCAML_REPO="${OCAML_REPO:-https://github.com/MinaProtocol/mina.git}"
OCAML_BRANCH="${OCAML_BRANCH:-c9e2eac5344da621828c1e96c074b8bdce665b51}"
UPDATE_MODE="${UPDATE_MODE:-false}"
DRIFT_MODE=false
RUST_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --repo) OCAML_REPO="${2:?--repo requires a URL}"; shift 2 ;;
        --branch) OCAML_BRANCH="${2:?--branch requires a revision}"; shift 2 ;;
        --update) UPDATE_MODE=true; shift ;;
        --drift) DRIFT_MODE=true; shift ;;
        *) echo "Usage: $0 [--repo URL] [--branch REV] [--drift | --update]" >&2; exit 2 ;;
    esac
done
if [[ "$DRIFT_MODE" == true && "$UPDATE_MODE" == true ]]; then
    echo "--drift is read-only and cannot be combined with --update" >&2
    exit 2
fi
if [[ ! "$OCAML_REPO" =~ ^https://github.com/([^/]+)/([^/]+)$ ]]; then
    echo "Error: Repository URL must be a GitHub URL" >&2
    exit 2
fi
GITHUB_OWNER="${BASH_REMATCH[1]}"
GITHUB_REPO="${BASH_REMATCH[2]%.git}"
TEMP_DIR=$(mktemp -d)
trap 'rm -rf "$TEMP_DIR"' EXIT

# Cache by revision and upstream path; repeated references need one download.
fetch_file() {
    local revision="$1" path="$2" destination="$3"
    local key cached
    key=$(printf '%s/%s' "$revision" "$path" | sha256sum | cut -d' ' -f1)
    cached="$TEMP_DIR/$key"
    if [[ ! -f "$cached" ]]; then
        if ! curl -fsS --retry 2 --connect-timeout 10 --max-time 60 \
            "https://raw.githubusercontent.com/$GITHUB_OWNER/$GITHUB_REPO/$revision/$path" \
            -o "$cached.download"; then
            return 1
        fi
        mv "$cached.download" "$cached"
    fi
    cp "$cached" "$destination"
}

TARGET_COMMIT="$OCAML_BRANCH"
if [[ "$UPDATE_MODE" == true ]]; then
    # Updates must write a resolved commit hash, never a branch name.
    curl -fsS --retry 2 --connect-timeout 10 --max-time 60 \
        "https://api.github.com/repos/$GITHUB_OWNER/$GITHUB_REPO/commits/$OCAML_BRANCH" \
        -o "$TEMP_DIR/target.json"
    TARGET_COMMIT=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["sha"])' "$TEMP_DIR/target.json")
    [[ "$TARGET_COMMIT" =~ ^[a-f0-9]{40}$ ]] || exit 1
fi

echo "Validating OCaml references at their pinned commits"
if [[ "$DRIFT_MODE" == true || "$UPDATE_MODE" == true ]]; then
    echo "Drift target: $OCAML_REPO (branch: $OCAML_BRANCH)"
    echo "Changed sources require review: OCAML-404-CHANGED-REFS"
fi
cd "$RUST_ROOT"
TOTAL=0 VALID=0 INVALID=0 CHANGED=0 UNAVAILABLE=0 UPDATED=0
# A grep miss is an empty inventory; repository/read failures must fail the gate.
git grep -l -E '^/// OCaml: <https://github.com/MinaProtocol/mina/blob/' -- '*.rs' '**/*.rs' > "$TEMP_DIR/rust-files" || {
    status=$?
    [[ "$status" == 1 ]] || exit "$status"
}
touch "$TEMP_DIR/updates"
URL_PATTERN='^https://github.com/MinaProtocol/mina/blob/([a-f0-9]+)/([^#[:space:]]+)(#L([0-9]+)(-L([0-9]+))?)?$'
# Keep the established unindented /// scope; do not silently broaden the gate.
while IFS= read -r rust_file; do
    while IFS=: read -r line_num line_content; do
        TOTAL=$((TOTAL + 1))
        URL=$(printf '%s\n' "$line_content" | sed -n 's/.*<\([^>]*\)>.*/\1/p')
        if [[ ! "$URL" =~ $URL_PATTERN ]]; then
            echo "[ERROR] INVALID: $rust_file:$line_num MALFORMED_URL"
            INVALID=$((INVALID + 1)); continue
        fi
        COMMIT="${BASH_REMATCH[1]}" OCAML_PATH="${BASH_REMATCH[2]}"
        START_LINE="${BASH_REMATCH[4]}" END_LINE="${BASH_REMATCH[6]}"
        END_LINE="${END_LINE:-$START_LINE}"
        if ! fetch_file "$COMMIT" "$OCAML_PATH" "$TEMP_DIR/pinned"; then
            echo "[ERROR] INVALID: $rust_file:$line_num PIN_OR_FILE_UNAVAILABLE $URL"
            INVALID=$((INVALID + 1)); continue
        fi
        FILE_LINES=$(awk 'END { print NR }' "$TEMP_DIR/pinned")
        if [[ -n "$START_LINE" ]] &&
            (( 10#$START_LINE < 1 || 10#$END_LINE < 10#$START_LINE || 10#$END_LINE > FILE_LINES )); then
            echo "[ERROR] INVALID: $rust_file:$line_num INVALID_LINE_RANGE $URL ($FILE_LINES lines)"
            INVALID=$((INVALID + 1)); continue
        fi
        VALID=$((VALID + 1))
        echo "[OK] VALID: $rust_file:$line_num -> $COMMIT/$OCAML_PATH"
        if [[ "$DRIFT_MODE" == false && "$UPDATE_MODE" == false ]]; then
            continue
        fi
        if ! fetch_file "$TARGET_COMMIT" "$OCAML_PATH" "$TEMP_DIR/target"; then
            echo "[DRIFT] TARGET_UNAVAILABLE: $rust_file:$line_num $OCAML_PATH"
            UNAVAILABLE=$((UNAVAILABLE + 1))
        elif ! cmp -s "$TEMP_DIR/pinned" "$TEMP_DIR/target"; then
            echo "[DRIFT] CHANGED: $rust_file:$line_num $OCAML_PATH (pin $COMMIT)"
            CHANGED=$((CHANGED + 1))
        elif [[ "$UPDATE_MODE" == true && "$COMMIT" != "$TARGET_COMMIT" ]]; then
            printf '%s|%s|%s\n' "$rust_file" "$line_num" "$COMMIT" >> "$TEMP_DIR/updates"
        fi
    done < <(grep -n '^/// OCaml: <https://github.com/MinaProtocol/mina/blob/' "$rust_file")
done < "$TEMP_DIR/rust-files"

echo ""
echo "Total references found: $TOTAL"
echo "Valid references: $VALID"
echo "Invalid references: $INVALID"
if [[ "$DRIFT_MODE" == true || "$UPDATE_MODE" == true ]]; then
    echo "Changed upstream references: $CHANGED"
    echo "Unavailable upstream references (deleted path or fetch failure): $UNAVAILABLE"
fi
if [[ "$DRIFT_MODE" == true ]]; then
    echo "[INFO] Advisory drift report only; exit 0 is not a validation result."
    echo "[INFO] Run without --drift for the enforcing pinned-reference check."
    exit 0
fi
if (( INVALID > 0 || UNAVAILABLE > 0 )); then
    echo "[ERROR] Validation failed; no pins updated"
    exit 1
fi
if [[ "$UPDATE_MODE" == true ]]; then
    while IFS='|' read -r rust_file line_num old_commit; do
        sed -i "${line_num}s@blob/${old_commit}/@blob/${TARGET_COMMIT}/@" "$rust_file"
        UPDATED=$((UPDATED + 1))
    done < "$TEMP_DIR/updates"
    echo "Updated $UPDATED reference(s); changed files retained for review"
fi
echo "[OK] All OCaml references are valid!"
