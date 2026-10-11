#!/bin/sh
# codex subagent-stop hook.
# Forwards the event JSON to the ai-memory server, fire-and-forget. Codex
# sends the PARENT session_id here, so the server never seeds a session from
# this event. SubagentStop requires JSON on stdout, so every path prints {}.
_lib_dir="$(dirname "$0")"
[ -f "$_lib_dir/_lib.sh" ] || _lib_dir="$_lib_dir/.."
. "$_lib_dir/_lib.sh"

SERVER="${AI_MEMORY_HOOK_URL:-http://127.0.0.1:49374}"
PAYLOAD=$(cat)
# The script fallback cannot sanitize the subagent's assistant message, so a
# payload that still carries the raw field is dropped whole (as in
# claude-code/stop.sh) rather than posted or spooled verbatim.
case "$PAYLOAD" in
    *'"last_assistant_message"'*)
        printf '{}\n'
        exit 0
        ;;
esac
CWD=$(ai_memory_extract_cwd "$PAYLOAD")
QS=$(ai_memory_marker_qs "$CWD")

printf '%s' "$PAYLOAD" \
    | ai_memory_post_hook "$SERVER/hook?event=subagent-stop&agent=codex${QS}" >/dev/null 2>&1 || true
printf '{}\n'
exit 0
