# In-app Claude chat for Concat (velarista fork) - design

Date: 2026-11-20. Status: approved by the owner 2026-11-20 (chat column on the left, collapsible, resizable).

## Intent
The owner (non-coder, Vietnamese) edits video in Concat and wants to talk to Claude inside the Concat window, the way
ZataCut's "Agent" panel works: type "sửa chữ chỗ này" and Claude knows where "here" is (playhead, selection) and makes
the edit on the project open on screen, where the owner watches it land and can Ctrl+Z it.

## Goals
- A **Chat sidebar on the far left**, a column of its own beside the media library, running the full height of the
  workspace (beside library AND timeline), below the title bar. It is NOT a dock pane.
- **Open / collapsed toggle**: open = ~340 px column with the chat; collapsed = a thin icon rail (~40 px) showing
  only a chat icon (with a dot while Claude is replying or has an unread reply). Clicking the icon opens it; a
  collapse button in the column header hides it. The state is remembered in prefs across restarts. Collapsed gives
  the space back to the preview and timeline.
- **Resizable like Concat's other columns**: a drag gutter on the column's right edge, reusing Concat's own
  splitter look/behaviour (`ui/workspace/splitter.slint`), with a sensible minimum (~260 px) and maximum (~half the
  window). The chosen width is remembered in prefs (`chat_width`) together with open/collapsed.
- Brain: **Claude Code on the owner's Claude subscription** (no API key, no extra cost), run headless per message.
- Each message carries a **playhead context** built by the window itself (no API needed for this).
- Claude edits the on-screen project through Concat's own MCP in **live mode** (PR #601, already merged in
  `velarista`): every edit is one undo step, visible at once.
- Small edits are done directly; big ones (re-cutting the whole video, deleting many clips, anything over ~5 edits)
  are first described as a plan and wait for the owner's OK in chat (system prompt rule).
- Conversation remembered per project; a "New conversation" button resets it; a "Stop" button ends a running reply.

## Non-goals (later)
- Paid API brains (Gemini / OpenAI / Claude API) and the cost-approval guard (ZataCut `ai_hub/cost.py`): next phase.
- Attaching the frame picture to every message (Claude calls the existing `preview_frame` tool when it needs to see).
- Mobile (Android/iOS) - desktop only; the pane is hidden where no `claude` executable can exist.
- Exposing playhead/selection to OUTSIDE agents (separate task, runs while GitHub builds this one).

## Anti-goals
- Claude touching files outside the project (no Bash/Edit/Write/Web tools).
- Big edits landing without the owner seeing a plan first.
- Large edits to existing Concat files (keep upstream merges easy): new code lives in new files.

## Constraints
- Rust + Slint; the window's state lives only on the event-loop thread (`host.rs`). The `claude` process and its
  output reader run on a worker thread; events come back with `slint::invoke_from_event_loop`.
- Live mode needs Settings > Automation "Scripts and apps" ON and "Let scripts edit the project that's open" ON. The
  pane shows a notice with one button that turns both on when they are off (the owner's click is the consent).
- `claude` is not on PATH on this PC; it is bundled at `%LOCALAPPDATA%\Claude-Data\*\claude-code\<ver>\claude.exe`.
- UI strings go through Concat's i18n (`locales/*.json`): add keys to `en.json` and `vi.json`.
- Builds are slow on this PC (~90 min); the owner wants the build done by GitHub Actions on the fork.

## Approaches considered
1. **Chat pane inside Concat, code in new files, Claude Code subscription (chosen by the owner).** Direct access to
   playhead/selection, same feel as ZataCut. Cost: a core change (~600-1000 lines, mostly new files) and a rebuild
   for every UI change.
2. Separate companion window/app talking to Concat over MCP. Keeps Concat's core untouched, but needs another app
   and the playhead feature for outside agents first; the owner asked for the chat inside Concat.
3. Paid API chat inside Concat. Needs key storage and cost guard; postponed by the owner.

## Design (decided)
- **Port of ZataCut `zatacut/claude_bridge.py`** (Python, ~320 lines), rewritten in Rust as `src/chat.rs`:
  - `find_claude()`: env `CONCAT_CLAUDE` -> `claude` on PATH -> `~/.local/bin/claude.exe` -> newest
    `%LOCALAPPDATA%\Claude-Data\*\claude-code\<ver>\**\claude.exe`.
  - Arguments per message (CHANGED 2026-11-20 at the owner's request: Claude Code's own tools are KEPT, `--permission-mode bypassPermissions`, no --tools/--allowedTools/--disallowedTools; risky actions are rules in the system prompt): `-p --output-format stream-json --verbose --mcp-config <json> --strict-mcp-config
    --tools "" --allowedTools mcp__concat --disallowedTools Bash Edit Write WebFetch WebSearch
    --append-system-prompt <rules> --max-turns 60 [--resume <session id>]`; message on stdin; no console window.
  - MCP config: server `concat` = `std::env::current_exe()` with `--mcp window`, env `CONCAT_API_ADDRESS` /
    `CONCAT_API_TOKEN` taken from the running server (so it always reaches THIS window).
  - Session id saved in the app data dir keyed by the project folder (not inside the project folder).
  - Stream parsing as ZataCut: assistant text -> chat bubble; tool_use -> a short "đang dùng: title_add" line;
    result -> end of turn, error text shown in Vietnamese.
- **Playhead context block** prepended to each message (built in `chat.rs` from `Studio`): project folder, playhead
  seconds + timecode + frame number, playing or paused, selected clip ids, every clip under the playhead (id, kind,
  lane, name/text, start, end, offset into the clip), the timeline length. Same shape idea as ZataCut
  `playhead_context`.
- **System prompt** (English, short): you edit the Concat project open on screen through `mcp__concat__*`; pass the
  project folder as `path`; "here / chỗ này" = the playhead context; reply in Vietnamese, plainly; small edits
  directly, big ones as a plan first; never `project_save` / `project_close` the on-screen project; use
  `preview_frame` to look.
- **UI** (`ui/chat/chat-sidebar.slint`, new): message list (owner right, Claude left, tool lines muted), a text box
  (Enter sends, Shift+Enter new line; no shortcut that clashes with Unikey/EVKey Telex), Send / Stop, "New
  conversation", collapse button, and the Automation notice; plus the collapsed icon rail. State lives in a new
  Slint global `Chat` so the hook in existing files stays tiny: in `app.slint` the desktop `Workspace` (inside the
  window's vertical layout, under the title bar) is wrapped in a `HorizontalLayout { ChatSidebar {} Workspace {} }`;
  `lib.rs` installs the callbacks from `chat.rs`; `prefs.rs` gains `chat_open: bool`. Desktop only (`!root.phone`).

## Left to the builder
- Exact Slint layout and styling (follow the inspector's look), scroll behaviour, how long messages wrap.
- Whether the context block is shown in the chat (proposal: hidden, with a small "kèm vị trí 00:12" tag).
- Error wording for: claude not found, not logged in (`claude auth login`), Automation off, window busy.

## Verification
- Build via GitHub Actions on the fork (`build-app.yml`, workflow_dispatch, ref `velarista`), install/run on the PC.
- Owner test: open a project, put the playhead on a title, type "đổi chữ này thành màu đỏ"; the title turns red,
  Ctrl+Z restores it; "Hội thoại mới" forgets the conversation; Stop ends a long reply.
