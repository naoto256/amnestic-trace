#!/bin/sh
# Behavioural tests for the deliberately thin host adapter. State-machine and
# payload tests live in Rust; this suite proves the adapter stays portable and
# does no interpretation of its own.
set -u

repo=$(cd "$(dirname "$0")/.." && pwd)
hook=$repo/plugin/tools/amtr-hook.sh
work=${TMPDIR:-/tmp}/amtr-hook-tests-$$
failures=0

cleanup() {
	chmod -R u+w "$work" 2>/dev/null || true
	rm -rf "$work"
}
trap cleanup EXIT HUP INT TERM

fresh() {
	chmod -R u+w "$work" 2>/dev/null || true
	rm -rf "$work"
	mkdir -p "$work/bin" "$work/.local/bin"
}

install_stub() {
	destination=$1
	cat >"$destination" <<'STUB'
#!/bin/sh
printf '%s\n' "$*" >"$HOME/call"
cat >"$HOME/input"
printf '%s' "${STUB_STDOUT:-HOOK-OUTPUT}"
printf '%s' "${STUB_STDERR:-}" >&2
exit "${STUB_EXIT:-0}"
STUB
	chmod +x "$destination"
}

invoke() {
	payload=$1
	shift
	printf '%s' "$payload" |
		env HOME="$work" PATH="$work/bin:$PATH" "$sh" "$hook" "$@"
}

check() {
	name=$1
	actual=$2
	expected=$3
	if [ "$actual" = "$expected" ]; then
		printf 'ok    [%s] %s\n' "$sh" "$name"
	else
		printf 'FAIL  [%s] %s\n        expected: %s\n        actual:   %s\n' \
			"$sh" "$name" "$expected" "$actual"
		failures=$((failures + 1))
	fi
}

cases() {
	for arguments in \
		"synthesize" \
		"recall SessionStart" \
		"recall PreToolUse" \
		"recall UserPromptSubmit"
	do
		fresh
		install_stub "$work/bin/amtr"
		payload='{"session_id":"sess1","quoted":"a\\\"b"}'
		# Intentional splitting: these are the four fixed canonical invocations.
		# shellcheck disable=SC2086
		out=$(invoke "$payload" $arguments 2>"$work/hook-stderr")
		check "$arguments forwards stdout" "$out" "HOOK-OUTPUT"
		check "$arguments forwards canonical argv" "$(cat "$work/call")" "$arguments"
		check "$arguments forwards stdin byte-for-byte" "$(cat "$work/input")" "$payload"
		check "$arguments keeps stderr silent" "$(cat "$work/hook-stderr")" ""
	done

	fresh
	install_stub "$work/.local/bin/amtr"
	out=$(env HOME="$work" PATH="/usr/bin:/bin" "$sh" "$hook" recall PreToolUse <<'EOF'
{"session_id":"sess1"}
EOF
	)
	check "appended user path finds amtr" "$out" "HOOK-OUTPUT"
	check "appended user path preserves argv" "$(cat "$work/call")" "recall PreToolUse"

	fresh
	install_stub "$work/bin/amtr"
	for invalid in "precompact" "deliver" "recall" "recall Human" "recall PreToolUse extra"
	do
		rm -f "$work/call"
		# Intentional splitting: invalid fixed examples, never user input.
		# shellcheck disable=SC2086
		out=$(invoke '{}' $invalid 2>&1)
		check "legacy/invalid '$invalid' is rejected" "$out" ""
		check "legacy/invalid '$invalid' never calls amtr" \
			"$([ -e "$work/call" ] && printf called || printf untouched)" "untouched"
	done

	fresh
	missing_hook="$work/amtr-hook-without-system-paths.sh"
	sed \
		-e "s|:/opt/homebrew/bin:/usr/local/bin|:$work/no-homebrew:$work/no-local|" \
		-e "s|command -v amtr >/dev/null 2>\&1|command -v amtr >\"$work/resolved-amtr\" 2>/dev/null|" \
		"$hook" >"$missing_hook"
	chmod +x "$missing_hook"
	shell_path=$(command -v "$sh")
	out=$(printf '%s' '{}' |
		env HOME="$work" PATH="$work/bin" "$shell_path" "$missing_hook" recall SessionStart 2>&1)
	check "missing-amtr fixture excludes the real binary" \
		"$(cat "$work/resolved-amtr" 2>/dev/null)" ""
	check "missing amtr fails open" "$out" ""

	fresh
	install_stub "$work/bin/amtr"
	out=$(
		export STUB_EXIT=17 STUB_STDOUT=PARTIAL STUB_STDERR=SECRET
		invoke '{}' recall UserPromptSubmit 2>"$work/hook-stderr"
	)
	check "amtr failure cannot fail the hook" "$?" "0"
	check "stdout already produced by amtr is preserved" "$out" "PARTIAL"
	check "amtr diagnostics are suppressed" "$(cat "$work/hook-stderr")" ""
}

for sh in sh dash bash ksh; do
	if ! command -v "$sh" >/dev/null 2>&1; then
		printf 'FAIL  required shell is missing: %s\n' "$sh"
		failures=$((failures + 1))
		continue
	fi
	cases
done

if [ "$failures" -ne 0 ]; then
	printf '\n%d hook regression(s) failed\n' "$failures"
	exit 1
fi

printf '\nall hook regressions passed\n'
