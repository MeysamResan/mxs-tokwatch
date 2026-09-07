# Claude subscription integration

TokWatch reads Claude allowance through Claude Code's documented `statusLine` interface. The bridge receives `rate_limits.five_hour` and `rate_limits.seven_day` on stdin. It saves only percentages, reset timestamps, a format version, and the local receipt time. Claude Code remains responsible for authentication. TokWatch neither opens credential files nor invokes an inference API.

The official interface supplies these subscription windows for Claude Pro and Max after the first response in a Claude Code session. Status lines run locally without consuming API tokens. They run at session start and on subsequent Claude Code events; they are not a continuously refreshed account API. See [Claude Code status line documentation](https://code.claude.com/docs/en/statusline).

## Connection and files

Choose Claude as the provider, then select **Connect Claude Code**. TokWatch merges a `statusLine` entry into `%USERPROFILE%\.claude\settings.json`, or into the directory named by an absolute `CLAUDE_CONFIG_DIR`. Claude Code documents that location and override in its [settings reference](https://code.claude.com/docs/en/settings).

The generated command launches PowerShell without a profile, with an encoded command that forwards stdin to the current executable's `--claude-statusline` entry point. Encoding the command keeps path characters from being interpreted by Claude Code's outer shell. Keep the EXE in its permanent location before connecting.

Unrelated settings are preserved. When changing an existing settings file, setup first creates a unique `.settings.tokwatch-backup.PID.SEQUENCE.json` backup in the Claude configuration directory. Reconnecting with the same EXE and same command changes nothing. A different existing status line is preserved: TokWatch instead writes `%LOCALAPPDATA%\TokWatch\claude-statusline-settings.json` containing the proposed entry and explains that a manual merge is needed. Save the existing `statusLine` before replacing it. To disconnect, remove the TokWatch `statusLine` entry or restore your previous entry; restoring an entire old backup may also undo later settings changes.

A project or managed setting can override the user setting, and Claude Code's hook/trust restrictions can prevent its execution. `is_configured()` checks the user entry only. The provider does not claim that this proves sign-in or active data delivery.

## Reading semantics

`run_statusline()` runs before the application acquires its tray mutex or creates windows. It bounds stdin to one MiB, rejects malformed JSON without repeating payload content, and atomically writes `%LOCALAPPDATA%\TokWatch\claude-statusline.json`. It prints a short percentage summary for Claude Code's own status line. The feed remains usable while the tray is closed.

`read_limits()` reads at most 16 KiB from that local feed. Polling retains the original receipt time; reading a cache does not make it fresh. The shared UI can therefore label old readings as stale. An expired window becomes unknown rather than being guessed to have reset to full allowance. A missing window or a new payload without subscription windows also becomes unknown. Context token counts, session cost, and API billing are never substituted for subscription allowance.

TokWatch displays only the two supported subscription windows. It does not infer the subscription plan, model-specific weekly windows, extra spending, or reset credits. Claude web activity cannot update this local feed by itself. The most recent Claude Code status-line emission wins when several sessions share the same Windows account.

## Local verification

Provider unit tests cover independent windows, ignored sensitive metadata, missing/invalid/expired rates, future cache timestamps, bounded malformed input, and preserving an existing status line. Launcher tests ensure arbitrary path characters remain inside an encoded argument. Run the repository's `scripts/build.ps1 -Task Verify` before packaging. Use synthetic stdin with a temporary `LOCALAPPDATA` when exercising `--claude-statusline`; no account sign-in or prompts are necessary for that check.
