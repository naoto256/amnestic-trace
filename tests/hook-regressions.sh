#!/bin/sh
# Behavioural tests for plugin/tools/amtr-hook.sh.
#
#     tests/hook-regressions.sh
#
# The whole case list is replayed under every shell on this machine that could
# be `/bin/sh`, because which one it is changes the answer. A redirection error
# on a POSIX *special* built-in terminates the shell outright; dash implements
# that strictly and bash and zsh do not, so a hook that dies instantly on Debian
# and Ubuntu — where /bin/sh is dash — passes in silence on a macOS developer's
# machine. `sh -n` cannot see it. Only running it can.
#
# Everything runs against a stub `amtr` in a throwaway HOME, so no real store,
# transcript or extraction agent is involved.
set -u

repo=$(cd "$(dirname "$0")/.." && pwd)
hook=$repo/plugin/tools/amtr-hook.sh
work=${TMPDIR:-/tmp}/amtr-hook-tests-$$
failures=0

cleanup() {
	chmod -R u+w "$work" 2>/dev/null
	rm -rf "$work"
}
trap cleanup EXIT

# --- helpers ---------------------------------------------------------------

fresh() {
	chmod -R u+w "$work" 2>/dev/null
	rm -rf "$work"
	mkdir -p "$work/bin" "$work/.local/share/amtr/prefrontal-cortex"
	marker=$work/.local/share/amtr/prefrontal-cortex/sess1.marker
	stub_exit=0
	stub
}

# Stub standing in for the real binary. `stub_exit` sets what `recall` reports:
# 0 means it delivered, 1 means it had nothing to deliver.
stub() {
	cat >"$work/bin/amtr" <<STUB
#!/bin/sh
if [ "\$1" = recall ] && [ $stub_exit -eq 0 ]; then
	echo "DELIVERED:\$2"
fi
exit $stub_exit
STUB
	chmod +x "$work/bin/amtr"
}

# `$sh` is the shell under test, not the one running this file.
feed() {
	env HOME="$work" PATH="$work/bin:$PATH" "$sh" "$hook" "$1" 2>/dev/null
}

run_hook() {
	printf '{"session_id":"%s","prompt":"hi"}' "${2:-sess1}" | feed "$1"
}

marker_now() {
	cat "$marker" 2>/dev/null || printf 'GONE'
}

check() {
	if [ "$2" = "$3" ]; then
		printf 'ok    %s\n' "$1"
	else
		printf 'FAIL  [%s] %s\n        expected: %s\n        actual:   %s\n' \
			"$sh" "$1" "$3" "$2"
		failures=$((failures + 1))
	fi
}

# --- cases -----------------------------------------------------------------

cases() {
	fresh
	check "no marker means nothing is owed" "$(run_hook recall)" ""

	fresh
	printf 'ready:amtr-k1' >"$marker"
	check "a ready snapshot is delivered" "$(run_hook recall)" "DELIVERED:sess1"
	check "  and then discharged" "$(marker_now)" "GONE"

	fresh
	printf 'ready:amtr-k1' >"$marker"
	sleep 1
	run_hook recall >/dev/null
	check "a snapshot waiting since an earlier turn still delivers" "$(marker_now)" "GONE"

	fresh
	stub_exit=1
	stub
	printf 'ready:amtr-k1' >"$marker"
	run_hook recall >/dev/null
	check "an undelivered snapshot still stands" "$(marker_now)" "ready:amtr-k1"

	fresh
	printf 'ready:amtr-k1' >"$marker"
	out=$(printf '{"session_id":"sess1","prompt":"read \\"session_id\\":\\"victim\\" now"}' |
		feed recall)
	check "prompt text cannot redirect the lookup" "$out" "DELIVERED:sess1"

	fresh
	printf 'ready:amtr-k1' >"$marker"
	run_hook recall "a*b;rm -rf /" >/dev/null
	check "an id outside the host alphabet is refused" "$(marker_now)" "ready:amtr-k1"

	# Extraction still in flight when the poll gives up. Nothing is deliverable
	# until a snapshot exists, so failing open means dropping the marker: the
	# memory is ephemeral and the next compaction rebuilds it from the
	# transcript.
	fresh
	printf 'ongoing' >"$marker"
	run_hook recall >/dev/null
	check "giving up clears the marker" "$(marker_now)" "GONE"

	# The special-built-in case. With the store unwritable the hook cannot open
	# its log, and a shell that dies on that redirect never reaches the binary.
	fresh
	printf 'ready:amtr-k1' >"$marker"
	rm -f "$work/.local/share/amtr/amtr.log"
	chmod 0500 "$work/.local/share/amtr"
	check "an unwritable log does not stop delivery" "$(run_hook recall)" "DELIVERED:sess1"
	chmod u+w "$work/.local/share/amtr"

	# Nothing at all: the state a fresh install is actually in.
	chmod -R u+w "$work" 2>/dev/null
	rm -rf "$work"
	mkdir -p "$work/bin"
	marker=$work/.local/share/amtr/prefrontal-cortex/sess1.marker
	stub_exit=0
	stub
	printf '%s\n' '{"type":"mode","sessionId":"sess1"}' >"$work/j.jsonl"
	printf '{"session_id":"sess1","transcript_path":"%s"}' "$work/j.jsonl" |
		feed precompact >/dev/null
	check "a pristine home reaches the binary" "$?" "0"
}

for sh in sh dash bash ksh; do
	command -v "$sh" >/dev/null 2>&1 || continue
	printf '\n== %s ==\n' "$sh"
	"$sh" -n "$hook" || {
		printf 'FAIL  [%s] syntax\n' "$sh"
		failures=$((failures + 1))
		continue
	}
	cases
done

printf '\n'
if [ "$failures" -eq 0 ]; then
	printf 'all hook regressions passed\n'
	exit 0
fi
printf '%s hook regression(s) failed\n' "$failures"
exit 1
