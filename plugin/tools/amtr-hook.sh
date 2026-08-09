#!/bin/sh
# Thin host adapter. All payload parsing and state transitions belong to the
# binary; this file only bridges the hook's minimal environment to it and makes
# every failure non-fatal to the host turn.
set -u

home=${HOME:-}
[ -n "$home" ] || exit 0

case "$#:${1:-}:${2:-}" in
1:synthesize: | 2:recall:SessionStart | 2:recall:PreToolUse | 2:recall:UserPromptSubmit) ;;
*) exit 0 ;;
esac

# Hook processes often omit user-installed binary locations. Append them so a
# system command already selected by the host cannot be shadowed.
PATH="$PATH:$home/.local/bin:$home/.cargo/bin:/opt/homebrew/bin:/usr/local/bin"
export PATH

command -v amtr >/dev/null 2>&1 || exit 0

# The binary redirects diagnostics to its private log before parsing stdin and
# buffers hook output until the debt is successfully claimed. Any remaining
# failure is deliberately silent and cannot fail the host event.
amtr "$@" 2>/dev/null || true
exit 0
