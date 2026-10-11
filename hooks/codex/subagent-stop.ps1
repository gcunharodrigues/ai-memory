. "$PSScriptRoot\..\lib\ai-memory-hook.ps1"
Invoke-AiMemoryHook -Event "subagent-stop" -Agent "codex"
# SubagentStop requires JSON on stdout on every path.
[Console]::Out.Write("{}")
exit 0
