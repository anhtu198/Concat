# In-app Claude Chat Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A collapsible, resizable Claude chat column on the far left of the Concat window that sends each message with
the playhead context to Claude Code (owner's subscription) and lets Claude edit the on-screen project in live mode.

**Architecture:** All logic that does not need Slint lives in a NEW crate `concat-chat` (finding `claude`, building its
arguments, parsing its stream, the playhead context text, the per-project session store, the process runner), so it
is unit-tested cheaply on this 8 GB PC. The window crate `concat` only adds a Slint global + sidebar component and a
thin glue module that turns UI callbacks into runner calls and runner events into model updates on the event loop.

**Tech Stack:** Rust 1.93 (edition 2024), Slint (window crate), serde_json, std::process; Claude Code CLI `claude -p
--output-format stream-json`; Concat MCP `--mcp window` (live mode, PR #601).

**Spec:** `docs/superpowers/specs/2026-11-20-in-app-claude-chat-design.md`

## Global Constraints
- Keep edits to existing Concat files minimal; new code in new files (`crates/concat-chat/`, `crates/concat/src/chat.rs`, `crates/concat/ui/chat/`).
- SPDX header on every new file: `// SPDX-License-Identifier: AGPL-3.0-or-later` + `// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors`.
- Every UI string via `I18n.t("chat.<key>")`; English in `locales/en.json`, Vietnamese (full diacritics) in `locales/vi.json`.
- No keyboard shortcut that clashes with Unikey/EVKey Telex: only Enter (send) and Shift+Enter (new line) in the input.
- Claude tools: only `mcp__concat`; disallow `Bash Edit Write WebFetch WebSearch`; `--max-turns 60`.
- Desktop only (`!root.phone`); on Android/iOS the column is not shown.
- Sidebar: open width default 340 px, min 260 px, max 50 % of window width; collapsed rail 40 px. Persist `chat_open`, `chat_width`.
- Session ids stored under the app data dir, never inside the project folder.
- CI bar (from README): `cargo fmt --check && cargo clippy --all-targets -- -D warnings`; run `python3 scripts/locales.py` after adding strings.

## Review Focus
- Automation server off or live switch off when the owner presses Send: show the notice with the enable button, never spawn `claude` (test in Task 5 glue: `send_refused_when_live_off`).
- `claude` not found / not logged in: one Vietnamese error bubble, the column stays usable (Task 1 `find_claude_none`, Task 3 `result_error_becomes_error_event`).
- Owner presses Stop mid-reply or closes the project/window while a reply runs: the process is killed, no event lands on a closed project (Task 4 `stop_kills_and_ends`, Task 5 drops events whose project path no longer matches).
- Project folder with spaces / Vietnamese characters / on OneDrive (`D:\OneDrive - xistatu\...`): context and MCP config carry it exactly (Task 2 `context_keeps_unicode_path`, Task 1 `mcp_config_escapes_path`).
- Very long replies or many messages: the list scrolls and stays responsive; keep at most 200 bubbles in the model (Task 5 `history_capped_at_200`).

---

### Task 1: crate `concat-chat` - find `claude` and build its arguments

**Files:**
- Create: `src/crates/concat-chat/Cargo.toml` (deps: `serde`, `serde_json` from workspace; dev: `tempfile` if the workspace has it, else std temp dir), `src/crates/concat-chat/src/lib.rs`, `src/crates/concat-chat/src/claude.rs`

**Interfaces:**
- Produces:
  - `pub fn find_claude(env: &Lookup) -> Option<PathBuf>`; `pub struct Lookup { pub override_path: Option<PathBuf>, pub path_dirs: Vec<PathBuf>, pub home: Option<PathBuf>, pub local_app_data: Option<PathBuf> }` with `Lookup::from_env()` (reads `CONCAT_CLAUDE`, `PATH`, `USERPROFILE`/`HOME`, `LOCALAPPDATA`).
    Order: override (if file exists) -> `claude.exe`/`claude` in PATH dirs -> `<home>/.local/bin/claude(.exe)` -> newest `<local_app_data>/Claude-Data/*/claude-code/<version>/**/claude.exe` (version compared numerically per dot part).
  - `pub struct RunSpec { pub claude: PathBuf, pub concat_exe: PathBuf, pub api_address: String, pub api_token: Option<String>, pub system_prompt: String, pub resume: Option<String> }`
  - `pub fn mcp_config_json(spec: &RunSpec) -> String` -> `{"mcpServers":{"concat":{"command":<concat_exe>,"args":["--mcp","window"],"env":{"CONCAT_API_ADDRESS":..,"CONCAT_API_TOKEN":..}}}}` (token key omitted when None).
  - `pub fn claude_args(spec: &RunSpec) -> Vec<OsString>`: `-p --output-format stream-json --verbose --mcp-config <json> --strict-mcp-config --allowedTools mcp__concat --disallowedTools Bash Edit Write WebFetch WebSearch --append-system-prompt <prompt> --max-turns 60` then `--resume <id>` when Some.
  - `pub const SYSTEM_PROMPT_TEMPLATE: &str` (English; rules from the spec "System prompt" bullet; `{path}` placeholder) and `pub fn system_prompt(project_path: &str) -> String`.

- [ ] **Step 1: Write failing tests** in `claude.rs` `#[cfg(test)]`:
  `find_claude_prefers_override`, `find_claude_picks_newest_bundled` (dirs `2.1.9` and `2.1.326` -> 2.1.326), `find_claude_none` (empty Lookup -> None),
  `args_have_strict_mcp_and_tool_limits` (contains `--strict-mcp-config`, `mcp__concat`, `Bash`, `60`; no `--resume`),
  `args_resume_when_session` (ends with `--resume`, `abc`), `mcp_config_escapes_path` (concat_exe `D:\OneDrive - xistatu\x\concat.exe` round-trips through `serde_json::from_str`), `prompt_mentions_path_and_vietnamese`.
- [ ] **Step 2:** `cargo test -p concat-chat` -> FAIL (unresolved items).
- [ ] **Step 3:** Implement the signatures above.
- [ ] **Step 4:** `cargo test -p concat-chat` -> PASS; `cargo clippy -p concat-chat --all-targets -- -D warnings` clean.
- [ ] **Step 5:** Commit `feat(chat): concat-chat crate - find claude and build its arguments`.

### Task 2: playhead context text

**Files:** Create `src/crates/concat-chat/src/context.rs` (add dep `concat-project` path dep).

**Interfaces:**
- Produces: `pub struct ContextInput<'a> { pub project_path: &'a str, pub project: &'a concat_project::model::Project, pub playhead: f64, pub playing: bool, pub selection: &'a [String] }` and `pub fn playhead_context(input: &ContextInput) -> String`.
  Output is a block starting `[PLAYHEAD CONTEXT]` and ending `[/PLAYHEAD CONTEXT]` with lines: `project: <path>`, `time: <s.sss> s (timecode HH:MM:SS:FF, frame N @ fps)`, `state: playing|paused`, `selected: <ids or none>`, `timeline length: <s> s`, then one line per clip of the ACTIVE timeline whose `[start, start+duration)` contains the playhead: `- <id> <kind> lane <track> "<name or text, max 80 chars>" <start>-<end> s, <offset> s into the clip`; `none` when empty.
  `pub fn compose_message(context: &str, owner_text: &str) -> String` = context + blank line + owner text.

- [ ] **Step 1: Tests:** `context_lists_clip_under_playhead` (a title c1 0-5 s, playhead 2.5 -> line contains `c1`, `2.500 s into` style offset), `context_none_when_empty`, `context_marks_selection_and_playing`, `context_keeps_unicode_path` (path `D:\OneDrive - xistatu\Desktop\Concat\thu-live`), `timecode_at_24fps` (2.5 s -> `00:00:02:12`). Build the test `Project` the same way `concat-project`'s own tests do (find a constructor/helper there; do not hand-write JSON if a builder exists).
- [ ] **Step 2:** run -> FAIL. **Step 3:** implement. **Step 4:** `cargo test -p concat-chat` PASS.
- [ ] **Step 5:** Commit `feat(chat): playhead context for each message`.

### Task 3: stream-json parsing and the session store

**Files:** Create `src/crates/concat-chat/src/stream.rs`, `src/crates/concat-chat/src/session.rs`.

**Interfaces:**
- Produces:
  - `pub enum ChatEvent { Text(String), Tool { name: String }, Done { session_id: Option<String> }, Error(String) }`
  - `pub fn parse_line(line: &str) -> Vec<ChatEvent>`: `type=assistant` -> each `text` block -> `Text`, each `tool_use` -> `Tool{name}` with prefix `mcp__concat__` stripped; `type=system` with `session_id` ignored; `type=result` -> `Done{session_id}` or, when `is_error` true / `subtype` != `success`, `Error(<result text or subtype>)` followed by `Done`. Unparseable lines -> empty vec.
  - `pub struct SessionStore { dir: PathBuf }`: `new(dir)`, `get(&self, project_path:&str) -> Option<String>`, `set(&self, project_path, id)`, `clear(&self, project_path)`. File per project: `<dir>/<16-hex hash of normalized path>.session` (normalize: lowercase on Windows, trailing separators trimmed).

- [ ] **Step 1: Tests:** `text_and_tool_events`, `result_success_gives_done_with_session`, `result_error_becomes_error_event`, `garbage_line_ignored`, `session_roundtrip_and_clear`, `session_path_case_insensitive_on_windows` (`#[cfg(windows)]`).
- [ ] **Step 2-4:** fail, implement, pass.
- [ ] **Step 5:** Commit `feat(chat): parse claude's stream and remember a session per project`.

### Task 4: the process runner

**Files:** Create `src/crates/concat-chat/src/runner.rs`.

**Interfaces:**
- Consumes: `RunSpec`, `claude_args`, `parse_line`, `ChatEvent`.
- Produces: `pub struct Runner` with `pub fn start(program: PathBuf, args: Vec<OsString>, stdin_text: String, cwd: Option<PathBuf>, on_event: impl Fn(ChatEvent) + Send + 'static) -> io::Result<Runner>`, `pub fn stop(&self)`, `pub fn is_running(&self) -> bool`.
  Spawns with stdin/stdout/stderr piped, `CREATE_NO_WINDOW` (0x08000000) on Windows via `CommandExt::creation_flags`; writes stdin then closes it; a reader thread feeds each stdout line to `parse_line` -> `on_event`; stderr collected; if the process exits without a `Done`, emit `Error(<last 500 chars of stderr or "exit code N">)` then `Done{None}`. `stop` kills the child and the reader then emits `Done{None}` exactly once.

- [ ] **Step 1: Tests** (Windows, using `cmd /C type <fixture>` as the program): fixture file with one assistant line + one result line -> events `Text`, `Done(Some)`; `exit_without_result_reports_error` (`cmd /C exit 3`); `stop_kills_and_ends` (`cmd /C ping -n 30 127.0.0.1`, stop after 200 ms, exactly one `Done` within 2 s).
- [ ] **Step 2-4:** fail, implement, pass.
- [ ] **Step 5:** Commit `feat(chat): run claude headless and stream its events`.

### Task 5: window glue, Slint global and sidebar, prefs and strings

**Files:**
- Create: `src/crates/concat/ui/chat/chat-sidebar.slint`, `src/crates/concat/src/chat.rs`
- Modify: `src/crates/concat/Cargo.toml` (dep `concat-chat`), `src/crates/concat/ui/app.slint` (wrap the desktop `Workspace` at ~line 808 in `HorizontalLayout { ChatSidebar {} Workspace {...} }`, import), `src/crates/concat/src/lib.rs` (call `chat::install(&app, ...)` next to the other `app.on_*` wiring ~line 849; `mod chat;`), `src/crates/concat/src/prefs.rs` (`pub chat_open: bool` default true, `pub chat_width: f32` default 340.0, serde defaults so old prefs load), `src/crates/concat/locales/en.json` + `vi.json` (keys below).

**Interfaces:**
- Consumes: Tasks 1-4; `Shell::with` / `slint::invoke_from_event_loop` (`src/host.rs`); `Studio { session, playhead, playing, selection, prefs.server }`; the window's server start path (the function in `panes/settings.rs` that calls `concat_server::Server::start`) for the enable button.
- Produces (Slint): `export struct ChatBubble { role: string /* "me" | "claude" | "tool" | "error" */, text: string }`;
  `export global Chat { in-out property <bool> open; in-out property <length> width; in property <[ChatBubble]> bubbles; in property <bool> busy; in property <bool> unread; in property <bool> live-off; in property <string> where /* e.g. "00:12" */; callback send(string); callback stop(); callback new-conversation(); callback toggle(); callback resized(length); callback enable-live(); }`;
  `export component ChatSidebar` (open: header with title, "New conversation", collapse « button; bubble list in a ScrollView auto-scrolling to the end; notice + button when `live-off`; TextEdit input, Send/Stop button swapping on `busy`; `ColSplitter` on its right edge calling `resized(delta)`; collapsed: 40 px rail with a chat icon from `icons.slint` and a dot when `busy || unread`).
- Produces (Rust, `chat.rs`): `pub fn install(app: &App)`; `pub struct ChatState { runner: Option<Runner>, bubbles: Vec<ChatBubble>, project: Option<String> }` held in `Studio` as `pub chat: crate::chat::ChatState` (one field added in `studio.rs`).
  Send: if no project -> error bubble `chat.noProject`; if `!prefs.server.enabled || !prefs.server.live` -> set `live-off`, no spawn; else build `ContextInput` from `Studio`, `compose_message`, `RunSpec` (concat_exe = `std::env::current_exe()`, address = `prefs.server.listen`, token = `prefs.server.token` non-empty, resume = `SessionStore` under `host.dirs.data/chat`), start `Runner` whose `on_event` hops to the event loop and appends bubbles only if `chat.project` still equals the open project's path; `Done{Some(id)}` -> `SessionStore::set`. Cap bubbles at 200 (drop oldest).
- Strings (en / vi): `chat.title` Claude chat / Chat với Claude; `chat.placeholder` Ask Claude about the edit… / Nhắn Claude về đoạn dựng…; `chat.send` Send / Gửi; `chat.stop` Stop / Dừng; `chat.newConversation` New conversation / Hội thoại mới; `chat.collapse` Hide chat / Ẩn chat; `chat.open` Open chat / Mở chat; `chat.liveOff` Turn on Settings › Automation and "Let scripts edit the project that's open" so Claude can edit this project. / Cần bật Settings › Automation và "Let scripts edit the project that's open" để Claude sửa được dự án này.; `chat.enableLive` Turn on for chat / Bật cho chat; `chat.noProject` Open a project first. / Mở một dự án trước đã.; `chat.notFound` Claude Code was not found on this computer. / Không tìm thấy Claude Code trên máy.; `chat.usingTool` Using {0}… / Đang dùng {0}…; `chat.attachedAt` with the playhead at {0} / kèm vị trí {0}.

- [ ] **Step 1: Tests** in `chat.rs` `#[cfg(test)]` for the pure helpers it owns: `send_refused_when_live_off` (a `fn send_decision(prefs, has_project) -> SendDecision` returns `LiveOff`), `history_capped_at_200`, `events_for_other_project_dropped`. (These compile with the window crate, so they run in CI, not locally.)
- [ ] **Step 2:** Implement Slint + glue + prefs + strings; run `python3 scripts/locales.py` (Python is NOT installed on this PC: note it and let CI catch drift, or run it in CI).
- [ ] **Step 3:** Local check is too heavy on this PC (window crate needs ~17 GB commit); verification is the GitHub build (Task 6). Run locally only `cargo fmt --check` and `cargo clippy -p concat-chat -- -D warnings`.
- [ ] **Step 4:** Commit `feat(chat): Claude chat column on the left of the window`.

### Task 6: build on GitHub and owner test

- [ ] **Step 1:** Push `velarista` to `origin` (owner already approved pushing for this feature).
- [ ] **Step 2:** Owner enables Actions on `anhtu198/Concat` (Actions tab -> "I understand my workflows, go ahead and enable them") and runs **Build app** (`build-app.yml`, workflow_dispatch) with ref `velarista`. Also `ci.yml` via workflow_dispatch for fmt/clippy/tests.
- [ ] **Step 3:** Download the Windows artifact, run it (portable or installer; do not uninstall the official 0.2.11 without asking).
- [ ] **Step 4:** Owner test from the spec "Verification" section; record results in `AGENTS.md`.
