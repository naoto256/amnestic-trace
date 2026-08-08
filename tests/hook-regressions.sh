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
	deadline_file=$work/.local/share/amtr/prefrontal-cortex/sess1.deliver-deadline
	stub_exit=0
	stub
}

# The tool-call deliverer waits until the epoch second named in its deadline
# file. Setting that file directly is how these cases reach the interesting
# states without spending the real 25s budget.
deadline_at() {
	printf '%s' "$(($(date +%s) + $1))" >"$deadline_file"
}

# A window already closed: the debt's budget has been spent by an earlier tool
# call, so the next one must not wait.
spend_budget() {
	deadline_at -60
}

# Publishes a claim the way the worker does, for cases that hand one to a hook
# already running. A plain redirect truncates before it writes, so a poll landing
# in that gap reads an empty marker, treats it as no longer `ongoing`, and steps
# aside — the hook under test then does nothing, and the case fails for a reason
# that has nothing to do with what it is testing.
publish_claim() {
	printf '%s' "$1" >"$marker.publishing"
	mv "$marker.publishing" "$marker"
}

# Waits for a hook to signal that it has reached the point a case wants to
# interfere with. A sleep long enough to be safe on a loaded machine is a slow
# suite; one short enough to keep the suite quick is a flake.
await_file() {
	waited=0
	while [ ! -f "$1" ] && [ "$waited" -lt 50 ]; do
		sleep 0.1 2>/dev/null || sleep 1
		waited=$((waited + 1))
	done
}

budget_shape() {
	case "$1" in
	'' | *[!0-9]*) printf 'absent-or-malformed' ;;
	*) [ "$1" -gt "$(date +%s)" ] && printf 'future' || printf 'past' ;;
	esac
}

# Stub standing in for the real binary. `stub_exit` sets what `recall` reports:
# 0 means it delivered, 1 means it had nothing to deliver.
#
# It echoes the event name it was handed as well as the session id, because the
# two deliverers differ only in that argument and a hook that named the wrong
# event would produce output the host discards — silently, and only on the path
# that was supposed to be the fast one.
stub() {
	cat >"$work/bin/amtr" <<STUB
#!/bin/sh
if [ "\$1" = recall ] && [ $stub_exit -eq 0 ]; then
	if [ "\${3:-}" = --hook-json ]; then
		echo "DELIVERED:\$2:\$4:\$6"
	else
		echo "DELIVERED:\$2"
	fi
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
	check "a ready snapshot is delivered" "$(run_hook recall)" "DELIVERED:sess1:UserPromptSubmit:amtr-k1"
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
	check "prompt text cannot redirect the lookup" "$out" "DELIVERED:sess1:UserPromptSubmit:amtr-k1"

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

	# --- the tool-call deliverer ---------------------------------------------
	#
	# It exists because a compaction fires mid-turn and the turn-start hook does
	# not run again until the user speaks. Everything below is about it being
	# early without being destructive.

	fresh
	check "no marker means nothing is owed at a tool call" "$(run_hook deliver)" ""

	fresh
	printf 'ready:amtr-k1' >"$marker"
	check "a ready snapshot is delivered at the first tool call" \
		"$(run_hook deliver)" "DELIVERED:sess1:PreToolUse:amtr-k1"
	check "  and then discharged" "$(marker_now)" "GONE"

	# The difference that matters between the two deliverers. The turn-start
	# hook gives up on a snapshot that never arrived, because the user is
	# waiting and something has to end the wait. This one is only ever early:
	# the next tool call is moments away and the backstop is still behind it, so
	# discarding the debt here would throw away a memory that was still coming.
	# Budget pre-spent, so this is the tool call that arrives after the window.
	fresh
	printf 'ongoing' >"$marker"
	spend_budget
	check "an unfinished extraction is left alone" "$(run_hook deliver)" ""
	check "  and the debt still stands" "$(marker_now)" "ongoing"

	# The bound on waiting. One budget is opened per compaction, so the tool
	# call that arrives after it has been spent does not stall — otherwise every
	# tool call in the stretch after a compaction would wait out the extraction,
	# and an extraction that had died would stall them all forever. Timed rather
	# than argued, because a poll is one line to add back by accident.
	fresh
	printf 'ongoing' >"$marker"
	spend_budget
	started=$(date +%s)
	run_hook deliver >/dev/null
	elapsed=$(($(date +%s) - started))
	check "a tool call past the budget does not stall" \
		"$([ "$elapsed" -lt 5 ] && echo prompt || echo "slow:${elapsed}s")" "prompt"

	# ...but the first one does wait, because the tool calls made in the stretch
	# right after a compaction are the ones made blind, and they are the reason
	# this hook exists at all. Here the extraction lands while the hook is
	# waiting, and the memory rides the tool call it would otherwise have missed.
	fresh
	printf 'ongoing' >"$marker"
	deadline_at 5
	( sleep 1; publish_claim 'ready:amtr-k1' ) &
	check "the first tool call waits for an extraction in flight" \
		"$(run_hook deliver)" "DELIVERED:sess1:PreToolUse:amtr-k1"
	wait 2>/dev/null || true

	# Waiting on a shared deadline means concurrent tool calls wake together by
	# design, so the moment the snapshot lands they are all holding the same
	# ready claim. Exactly one may inject it: discharging the marker afterwards
	# cannot prevent a second injection, because by then it has already gone to
	# the host. Two waiters, one delivery.
	fresh
	printf 'ongoing' >"$marker"
	deadline_at 10
	i=1
	while [ "$i" -le 2 ]; do
		run_hook deliver >"$work/race.$i" 2>/dev/null &
		i=$((i + 1))
	done
	sleep 2
	publish_claim 'ready:amtr-k1'
	wait 2>/dev/null || true
	check "concurrent waiters deliver the snapshot exactly once" \
		"$(cat "$work"/race.* 2>/dev/null | grep -c DELIVERED)" "1"

	# Nobody has waited yet, so this tool call opens the window. Checked from
	# outside because the hook is still sitting in it.
	fresh
	printf 'ongoing' >"$marker"
	run_hook deliver >/dev/null 2>&1 &
	hook_pid=$!
	sleep 1
	budget=$(cat "$deadline_file" 2>/dev/null)
	check "the first tool call opens a waiting window" \
		"$(budget_shape "$budget")" "future"
	kill "$hook_pid" 2>/dev/null
	wait 2>/dev/null || true

	# A deadline further ahead than one budget cannot have been written by a
	# tool call whose clock agreed with this one. Sitting in it would hand the
	# job of ending the wait to the host's hook timeout; refusing it keeps that
	# bound inside the script.
	#
	# Observed from outside rather than timed around a foreground call: the
	# behaviour this guards against is an over-long wait, and a foreground call
	# that waits too long hangs the suite instead of failing it.
	fresh
	printf 'ongoing' >"$marker"
	deadline_at 60
	(
		run_hook deliver >/dev/null 2>&1
		printf 'returned' >"$work/deliver-returned"
	) &
	hook_pid=$!
	sleep 3
	check "an implausibly distant deadline is refused, not waited out" \
		"$(cat "$work/deliver-returned" 2>/dev/null || printf 'still-waiting')" "returned"
	kill "$hook_pid" 2>/dev/null
	wait 2>/dev/null || true
	check "  and the debt still stands" "$(marker_now)" "ongoing"

	# One window per debt, not one per tool call: a second call landing inside
	# the window joins it rather than starting its own.
	fresh
	printf 'ongoing' >"$marker"
	deadline_at 5
	before=$(cat "$deadline_file")
	run_hook deliver >/dev/null 2>&1 &
	hook_pid=$!
	sleep 1
	check "a later tool call joins the window instead of opening another" \
		"$(cat "$deadline_file")" "$before"
	kill "$hook_pid" 2>/dev/null
	wait 2>/dev/null || true

	# A new compaction is a new debt, and a new debt gets its own window. The
	# previous one's spent budget must not carry over, or the session that most
	# needs the wait — one compacting repeatedly — never gets it.
	fresh
	spend_budget
	printf '%s' "irrelevant" >"$work/journal.jsonl"
	printf '{"session_id":"sess1","transcript_path":"%s"}' "$work/journal.jsonl" |
		feed precompact >/dev/null 2>&1
	check "a new compaction reopens the waiting window" \
		"$(cat "$deadline_file" 2>/dev/null || printf 'GONE')" "GONE"

	# A claim taken for delivery is normally discharged or restored, but a hook
	# killed between the two leaves it behind and nothing else would remove it.
	# Left alone it accumulates in the store for the life of the machine.
	fresh
	printf 'ready:amtr-orphan' >"$marker.delivering.99999"
	printf '%s\n' '{"type":"mode","sessionId":"sess1"}' >"$work/j.jsonl"
	printf '{"session_id":"sess1","transcript_path":"%s"}' "$work/j.jsonl" |
		feed precompact >/dev/null 2>&1
	check "a new compaction sweeps a claim left behind by a killed hook" \
		"$(ls "$work"/.local/share/amtr/prefrontal-cortex/*.delivering.* 2>/dev/null | wc -l | tr -d ' ')" "0"

	# Sweeping means a claim can vanish from under a delivery that is still
	# running, and that delivery still tries to put it back when it fails. There
	# is nothing to put back, and the marker must not appear anyway. The sweep
	# is timed against a signal from the delivery rather than a sleep, so it
	# lands inside the window on a loaded machine instead of before it.
	fresh
	printf 'ready:amtr-k1' >"$marker"
	cat >"$work/bin/amtr" <<STUB
#!/bin/sh
: >"$work/recall-entered"
sleep 2
exit 1
STUB
	chmod +x "$work/bin/amtr"
	run_hook deliver >/dev/null 2>&1 &
	hook_pid=$!
	await_file "$work/recall-entered"
	rm -f "$marker".delivering.*
	wait 2>/dev/null || true
	check "a claim swept mid-delivery is not restored as an empty marker" \
		"$(marker_now)" "GONE"
	stub

	# A claim held for delivery is superseded if a compaction lands while the
	# delivery runs, so returning it must not displace what took its place.
	# This is the other half of what the restore has to get right, and it is
	# the half that stops an older snapshot outliving the one that replaced it.
	fresh
	printf 'ready:amtr-k1' >"$marker"
	cat >"$work/bin/amtr" <<STUB
#!/bin/sh
: >"$work/recall-entered"
sleep 2
exit 1
STUB
	chmod +x "$work/bin/amtr"
	run_hook deliver >/dev/null 2>&1 &
	hook_pid=$!
	await_file "$work/recall-entered"
	printf 'ready:amtr-k2' >"$marker"
	wait 2>/dev/null || true
	check "a failed delivery does not put its claim back over a newer one" \
		"$(marker_now)" "ready:amtr-k2"
	stub

	# The window a check cannot close. A restore that tests the claim and then
	# copies it has a gap between the two, and the redirect creates the marker
	# before the copy learns its input is gone — so a sweep landing in that gap
	# produces the empty marker the test was meant to prevent. Reached
	# deterministically rather than by racing: a `cat` that removes the claim
	# the second time it is asked to read one stands in for the sweep, the
	# first read being the delivery's own identity check.
	fresh
	printf 'ready:amtr-k1' >"$marker"
	cat >"$work/bin/amtr" <<'STUB'
#!/bin/sh
exit 1
STUB
	chmod +x "$work/bin/amtr"
	cat >"$work/bin/cat" <<CATW
#!/bin/sh
case "\${1:-}" in
*.delivering.*)
	if [ -f "$work/claim-read" ]; then
		rm -f "\$1"
	else
		: >"$work/claim-read"
	fi
	;;
esac
exec /bin/cat "\$@"
CATW
	chmod +x "$work/bin/cat"
	run_hook deliver >/dev/null 2>&1
	rm -f "$work/bin/cat"
	check "a claim lost between testing it and reading it leaves no empty marker" \
		"$(if [ -f "$marker" ] && [ ! -s "$marker" ]; then echo empty; else echo "not-empty"; fi)" \
		"not-empty"
	stub

	# And a delivered debt takes its window with it, so the next compaction
	# starts from a clean slate even if nothing else ran in between.
	fresh
	printf 'ready:amtr-k1' >"$marker"
	deadline_at 5
	run_hook deliver >/dev/null
	check "discharging the debt clears its window" \
		"$(cat "$deadline_file" 2>/dev/null || printf 'GONE')" "GONE"

	# Both hooks fire on the same turn once the memory is ready. The marker is
	# what stops the same snapshot being injected twice.
	fresh
	printf 'ready:amtr-k1' >"$marker"
	run_hook deliver >/dev/null
	check "the turn-start hook does not deliver it again" "$(run_hook recall)" ""

	# A snapshot that lands mid-turn belongs to the next delivery, not this one.
	fresh
	printf 'ready:amtr-k1' >"$marker"
	cat >"$work/bin/amtr" <<STUB
#!/bin/sh
printf 'ready:amtr-k2' >"$marker"
echo "DELIVERED:\$2:\$4"
STUB
	chmod +x "$work/bin/amtr"
	run_hook deliver >/dev/null
	check "a newer claim arriving mid-delivery is not discharged" \
		"$(marker_now)" "ready:amtr-k2"
	stub

	# And the binary is told which snapshot the marker owed, stripped of its
	# `ready:` prefix, so it can decline to hand over a newer one it was not
	# asked for. Without that, the case above delivers memory it then fails to
	# discharge, and the next hook delivers it again.
	fresh
	printf 'ready:amtr-k1' >"$marker"
	check "the claimed key reaches the binary" \
		"$(run_hook deliver)" "DELIVERED:sess1:PreToolUse:amtr-k1"

	# The special-built-in case. With the store unwritable the hook cannot open
	# its log, and a shell that dies on that redirect never reaches the binary.
	fresh
	printf 'ready:amtr-k1' >"$marker"
	rm -f "$work/.local/share/amtr/amtr.log"
	chmod 0500 "$work/.local/share/amtr"
	check "an unwritable log does not stop delivery" "$(run_hook recall)" "DELIVERED:sess1:UserPromptSubmit:amtr-k1"
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
