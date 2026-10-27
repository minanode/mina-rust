#!/usr/bin/env bash
# Take a turn on the shared machine: take a lock, then run a command that holds
# it until it exits. See README.md, "Taking turns".
#
#   tools/regression/lock.sh node [--log FILE] -- COMMAND [ARGS...]
#   tools/regression/lock.sh browser [--log FILE] -- COMMAND [ARGS...]
#   tools/regression/lock.sh build [--log FILE] -- COMMAND [ARGS...]
#   tools/regression/lock.sh status
#
# `node` takes whichever of the two node slots, node-1 and node-2, is free: one
# for each extra native node that joins mainnet. `browser` takes the browser
# lock, for a browser bootstrap; webnode/run.mjs takes it itself. `build`
# takes one of three build slots, build-1 to build-3, so that parallel agents
# don't run more than three compilations at once. All of them wait
# while what they need is taken, saying who has it, then exec COMMAND. It keeps
# the locked descriptor, and with it the lock, until it exits, and it keeps
# this script's PID, so `$!` after `lock.sh ... &` is the command's PID. With
# --log FILE the command's output, and only its, goes to FILE; messages about
# the wait stay on stderr. `status` says who has what.
#
# Environment:
#   MINA_LOCK_DIR  where the lock files are (default: the main checkout's
#                  .work/locks, shared by its worktrees, made when needed)
#   LOCK_WAIT      give up after this many seconds, exiting 75 (default: wait
#                  as long as it takes; 0: don't wait)
#   LOCK_POLL      seconds between attempts while waiting (default 15)
#   MINA_NO_LOCK   1 to run without a lock, on a machine you have to yourself
set -euo pipefail

usage() { sed -n '2,/^[^#]/s/^# \{0,1\}//p' "${BASH_SOURCE[0]}"; }
die() {
	echo "lock.sh: $*" >&2
	exit 2
}

lock_dir() {
	if [[ -n ${MINA_LOCK_DIR:-} ]]; then
		printf '%s\n' "$MINA_LOCK_DIR"
		return
	fi
	local here common
	here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
	# From a worktree too, git's common directory is the main checkout's .git.
	common=$(git -C "$here" rev-parse --path-format=absolute --git-common-dir 2>/dev/null) ||
		die "not in a git checkout: set MINA_LOCK_DIR"
	printf '%s/.work/locks\n' "$(dirname "$common")"
}

holder() {
	local text
	text=$(cat "$dir/$1.lock" 2>/dev/null || true)
	printf '%s\n' "${text:-(no holder recorded)}"
}

# Lock the first free one of the named locks on descriptor 9 and set `taken`.
take() {
	local name
	for name in "$@"; do
		exec 9>>"$dir/$name.lock"
		if flock -n 9; then
			taken=$name
			return 0
		fi
		exec 9>&-
	done
	return 1
}

status() {
	local name
	for name in browser node-1 node-2 build-1 build-2 build-3; do
		if [[ ! -e $dir/$name.lock ]]; then
			echo "$name: free, never taken"
		elif flock -n "$dir/$name.lock" true; then
			echo "$name: free; last taken by $(holder "$name")"
		else
			echo "$name: taken by $(holder "$name")"
		fi
	done
}

[[ $# -gt 0 ]] || {
	usage >&2
	exit 2
}
case $1 in
-h | --help)
	usage
	exit 0
	;;
status)
	dir=$(lock_dir)
	[[ -d $dir ]] || {
		echo "no locks taken yet: $dir does not exist"
		exit 0
	}
	status
	exit 0
	;;
node) names=(node-1 node-2) wanted="node-1 or node-2" ;;
browser) names=(browser) wanted=browser ;;
build) names=(build-1 build-2 build-3) wanted="a build slot" ;;
*) die "unknown lock '$1'; see --help" ;;
esac
shift
log=
while [[ $# -gt 0 ]]; do
	case $1 in
	--log)
		[[ $# -ge 2 ]] || die "--log needs a file"
		log=$2
		shift 2
		;;
	--)
		shift
		break
		;;
	*) die "unexpected '$1' before --; see --help" ;;
	esac
done
[[ $# -gt 0 ]] || die "no command given; see --help"

if [[ ${MINA_NO_LOCK:-} == 1 ]]; then
	echo "lock.sh: MINA_NO_LOCK=1, running without the $wanted lock" >&2
else
	command -v flock >/dev/null ||
		die "flock (util-linux) not found; or set MINA_NO_LOCK=1 on a machine you have to yourself"
	wait=${LOCK_WAIT:-}
	[[ -z $wait || $wait =~ ^[0-9]+$ ]] || die "LOCK_WAIT=$wait is not a number of seconds"
	dir=$(lock_dir)
	mkdir -p "$dir"
	start=$SECONDS shown=
	until take "${names[@]}"; do
		now=$(for name in "${names[@]}"; do echo "  $name: $(holder "$name")"; done)
		if [[ -n $wait ]] && ((SECONDS - start >= wait)); then
			printf 'lock.sh: gave up after %s s; taken by:\n%s\n' "$wait" "$now" >&2
			exit 75
		fi
		if [[ $now != "$shown" ]]; then
			printf 'lock.sh: waiting for %s in %s; taken by:\n%s\n' "$wanted" "$dir" "$now" >&2
			shown=$now
		fi
		sleep "${LOCK_POLL:-15}"
	done
	# Truncating the file keeps the lock, which is on the open file.
	what="$*"
	printf 'pid %s since %s in %s: %s\n' "$$" "$(date -u +%FT%TZ)" "$PWD" "${what:0:300}" >"$dir/$taken.lock"
	echo "lock.sh: took $taken, $dir/$taken.lock, for pid $$" >&2
fi
if [[ -n $log ]]; then
	exec >"$log" 2>&1
fi
exec "$@"
