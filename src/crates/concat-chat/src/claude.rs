// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors
//! Finding Claude Code on this machine and the command line one chat turn
//! runs it with.
//!
//! A port of ZataCut's `claude_bridge.py`: Claude Code runs headless
//! (`claude -p`) on the person's own Claude subscription, with Concat's MCP
//! as its only tools, pointed at this window's server so every edit lands
//! on the project on screen (live mode).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The executable's file name on this platform.
#[cfg(windows)]
pub const EXE: &str = "claude.exe";
/// The executable's file name on this platform.
#[cfg(not(windows))]
pub const EXE: &str = "claude";

/// Where to look for Claude Code, read from the environment once.
#[derive(Debug, Clone, Default)]
pub struct Lookup {
    /// `CONCAT_CLAUDE`: an explicit executable, tried first.
    pub override_path: Option<PathBuf>,
    /// The directories of `PATH`.
    pub path_dirs: Vec<PathBuf>,
    /// The home folder, for `~/.local/bin`.
    pub home: Option<PathBuf>,
    /// `%LOCALAPPDATA%`, where the Claude desktop app keeps its bundled
    /// copies of Claude Code.
    pub local_app_data: Option<PathBuf>,
    /// `%APPDATA%`, where `npm install -g` puts its command shims.
    pub app_data: Option<PathBuf>,
}

impl Lookup {
    /// This process's environment.
    pub fn from_env() -> Lookup {
        let var = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
        Lookup {
            override_path: var("CONCAT_CLAUDE").map(PathBuf::from),
            path_dirs: var("PATH")
                .map(|path| std::env::split_paths(&path).collect())
                .unwrap_or_default(),
            home: var("USERPROFILE")
                .or_else(|| var("HOME"))
                .map(PathBuf::from),
            local_app_data: var("LOCALAPPDATA").map(PathBuf::from),
            app_data: var("APPDATA").map(PathBuf::from),
        }
    }
}

/// Claude Code's executable: the override, then `PATH`, then
/// `~/.local/bin`, then the newest copy the Claude desktop app carries,
/// then an npm install's `claude.cmd` (run through [`launch_for`]).
pub fn find_claude(lookup: &Lookup) -> Option<PathBuf> {
    if let Some(path) = lookup.override_path.as_ref().filter(|path| path.is_file()) {
        return Some(path.clone());
    }
    let named = |dir: &Path| Some(dir.join(EXE)).filter(|path| path.is_file());
    if let Some(found) = lookup.path_dirs.iter().find_map(|dir| named(dir)) {
        return Some(found);
    }
    if let Some(found) = lookup
        .home
        .as_ref()
        .and_then(|home| named(&home.join(".local").join("bin")))
    {
        return Some(found);
    }
    if let Some(found) = lookup
        .local_app_data
        .as_ref()
        .and_then(|dir| newest_bundled(&dir.join("Claude-Data")))
    {
        return Some(found);
    }
    let shim = |dir: &Path| Some(dir.join("claude.cmd")).filter(|path| path.is_file());
    lookup
        .path_dirs
        .iter()
        .find_map(|dir| shim(dir))
        .or_else(|| {
            lookup
                .app_data
                .as_ref()
                .and_then(|dir| shim(&dir.join("npm")))
        })
}

/// The program to start for `claude` and the arguments that go before
/// Claude Code's own. An executable runs as it is. An npm `.cmd` shim is
/// not run through cmd.exe, whose quoting would mangle the JSON arguments:
/// the script it names is run with `node` from `PATH` instead.
pub fn launch_for(claude: &Path, lookup: &Lookup) -> (PathBuf, Vec<OsString>) {
    let is_shim = claude
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd"));
    if is_shim && let Some(target) = shim_target(claude) {
        if target
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
        {
            return (target, Vec::new());
        }
        let node = lookup
            .path_dirs
            .iter()
            .map(|dir| dir.join("node.exe"))
            .find(|path| path.is_file())
            .unwrap_or_else(|| PathBuf::from("node"));
        return (node, vec![target.into_os_string()]);
    }
    (claude.to_path_buf(), Vec::new())
}

/// The file an npm shim runs: the quoted path after `%dp0%\`.
fn shim_target(shim: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(shim).ok()?;
    let start = text.find("%dp0%\\")? + "%dp0%\\".len();
    let rest = &text[start..];
    let relative = &rest[..rest.find('"')?];
    Some(
        shim.parent()?
            .join(relative.replace('\\', std::path::MAIN_SEPARATOR_STR)),
    )
}

/// `<root>/*/claude-code/<version>/**/claude.exe`, the highest version.
fn newest_bundled(root: &Path) -> Option<PathBuf> {
    let mut best: Option<(Vec<u64>, PathBuf)> = None;
    for profile in read_dirs(root) {
        for version_dir in read_dirs(&profile.join("claude-code")) {
            let Some(name) = version_dir.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let version: Vec<u64> = name
                .split('.')
                .map(|part| part.parse().unwrap_or(0))
                .collect();
            if best.as_ref().is_some_and(|(have, _)| *have >= version) {
                continue;
            }
            if let Some(exe) = find_file(&version_dir, EXE, 3) {
                best = Some((version, exe));
            }
        }
    }
    best.map(|(_, exe)| exe)
}

fn read_dirs(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .collect()
        })
        .unwrap_or_default()
}

/// `name` in `dir` or up to `depth` folders below it.
fn find_file(dir: &Path, name: &str, depth: usize) -> Option<PathBuf> {
    let here = dir.join(name);
    if here.is_file() {
        return Some(here);
    }
    if depth == 0 {
        return None;
    }
    read_dirs(dir)
        .iter()
        .find_map(|sub| find_file(sub, name, depth - 1))
}

/// Everything one chat turn's command line needs.
#[derive(Debug, Clone)]
pub struct RunSpec {
    /// Claude Code's executable.
    pub claude: PathBuf,
    /// This app's own executable, which serves the MCP.
    pub concat_exe: PathBuf,
    /// The window's API server, `host:port`.
    pub api_address: String,
    /// Its token, when it has one.
    pub api_token: Option<String>,
    /// Appended to Claude Code's own system prompt.
    pub system_prompt: String,
    /// The conversation to carry on, when there is one.
    pub resume: Option<String>,
}

/// The `--mcp-config` JSON: Concat's MCP, through this window's server.
pub fn mcp_config_json(spec: &RunSpec) -> String {
    let mut env = serde_json::Map::new();
    env.insert("CONCAT_API_ADDRESS".into(), spec.api_address.clone().into());
    if let Some(token) = &spec.api_token {
        env.insert("CONCAT_API_TOKEN".into(), token.clone().into());
    }
    serde_json::json!({
        "mcpServers": {
            "concat": {
                "command": spec.concat_exe.to_string_lossy(),
                "args": ["--mcp", "window"],
                "env": env,
            }
        }
    })
    .to_string()
}

/// The arguments after the executable; the message goes on stdin.
pub fn claude_args(spec: &RunSpec) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--mcp-config",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    args.push(mcp_config_json(spec).into());
    for arg in [
        "--strict-mcp-config",
        // None of Claude Code's own tools: Concat's MCP is all it may use.
        "--tools",
        "",
        "--allowedTools",
        "mcp__concat",
        "--disallowedTools",
        "Bash",
        "Edit",
        "Write",
        "WebFetch",
        "WebSearch",
        "--append-system-prompt",
    ] {
        args.push(arg.into());
    }
    args.push(spec.system_prompt.clone().into());
    args.push("--max-turns".into());
    args.push("60".into());
    if let Some(id) = &spec.resume {
        args.push("--resume".into());
        args.push(id.into());
    }
    args
}

/// The rules Claude works by in the chat; `{path}` is the project folder.
pub const SYSTEM_PROMPT_TEMPLATE: &str = "\
You are the editing assistant inside Concat, a video editor. The person is watching the project open in the window \
while you work. Edit it only through the mcp__concat__ tools and pass this project folder as `path`: {path}
Each message starts with a [PLAYHEAD CONTEXT] block written by the window: \"here\", \"this\", \"chỗ này\", \
\"đoạn này\" mean the playhead time and the clips listed there, or the selected clips. To see the frame, call \
preview_frame at the playhead time.
Small edits (change a text, colour, trim or move one clip, add one title or effect) - do them directly; each is one \
undo step the person can Ctrl+Z. Big edits (re-cutting the video, deleting or adding many clips, anything over about \
five edits) - first describe the plan in a few short lines and wait for the person's OK.
Never call project_save, project_close or project_open on another folder: the window saves this project itself.
Reply in Vietnamese, short and plain, without technical words; say what you changed and where (time in seconds).";

/// The system prompt for the project at `project_path`.
pub fn system_prompt(project_path: &str) -> String {
    SYSTEM_PROMPT_TEMPLATE.replace("{path}", project_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::fs;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("concat-chat-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(path: &std::path::Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"").unwrap();
    }

    fn spec() -> RunSpec {
        RunSpec {
            claude: PathBuf::from("claude.exe"),
            concat_exe: PathBuf::from(r"C:\Concat\concat.exe"),
            api_address: "127.0.0.1:7420".into(),
            api_token: Some("tok".into()),
            system_prompt: "rules".into(),
            resume: None,
        }
    }

    #[test]
    fn find_claude_prefers_override() {
        let dir = scratch("override");
        let mine = dir.join("mine.exe");
        touch(&mine);
        let on_path = dir.join("bin").join(EXE);
        touch(&on_path);
        let lookup = Lookup {
            override_path: Some(mine.clone()),
            path_dirs: vec![dir.join("bin")],
            home: None,
            local_app_data: None,
            app_data: None,
        };
        assert_eq!(find_claude(&lookup), Some(mine));
    }

    #[test]
    fn find_claude_picks_newest_bundled() {
        let dir = scratch("bundled");
        let old = dir.join("Claude-Data/x/claude-code/2.1.9/claude.exe");
        let new = dir.join("Claude-Data/y/claude-code/2.1.326/claude.exe");
        touch(&old);
        touch(&new);
        let lookup = Lookup {
            override_path: None,
            path_dirs: vec![],
            home: None,
            local_app_data: Some(dir.clone()),
            app_data: None,
        };
        assert_eq!(find_claude(&lookup), Some(new));
    }

    #[test]
    fn find_claude_none() {
        let lookup = Lookup {
            override_path: None,
            path_dirs: vec![],
            home: None,
            local_app_data: None,
            app_data: None,
        };
        assert_eq!(find_claude(&lookup), None);
    }

    #[test]
    fn find_claude_finds_npm_shim() {
        let dir = scratch("npm");
        let shim = dir.join("npm").join("claude.cmd");
        touch(&shim);
        let lookup = Lookup {
            app_data: Some(dir.clone()),
            ..Lookup::default()
        };
        assert_eq!(find_claude(&lookup), Some(shim));
    }

    /// What npm writes for a package's command, trimmed to what matters.
    const NPM_SHIM: &str = "@ECHO off\r\nGOTO start\r\n:start\r\nSETLOCAL\r\n\"%_prog%\"  \"%dp0%\\node_modules\\@anthropic-ai\\claude-code\\cli.js\" %*\r\n";

    #[test]
    fn launch_runs_npm_shim_script_with_node() {
        let dir = scratch("launch");
        let shim = dir.join("npm").join("claude.cmd");
        touch(&shim);
        fs::write(&shim, NPM_SHIM).unwrap();
        let node = dir.join("nodejs").join("node.exe");
        touch(&node);
        let lookup = Lookup {
            path_dirs: vec![dir.join("nodejs")],
            ..Lookup::default()
        };
        let (program, leading) = launch_for(&shim, &lookup);
        assert_eq!(program, node);
        let script = dir
            .join("npm")
            .join("node_modules")
            .join("@anthropic-ai")
            .join("claude-code")
            .join("cli.js");
        assert_eq!(leading, vec![OsString::from(script)]);
    }

    #[test]
    fn launch_runs_exe_directly() {
        let exe = PathBuf::from(r"C:\x\claude.exe");
        let (program, leading) = launch_for(&exe, &Lookup::default());
        assert_eq!(program, exe);
        assert!(leading.is_empty());
    }

    fn strings(args: &[std::ffi::OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn args_have_strict_mcp_and_tool_limits() {
        let args = strings(&claude_args(&spec()));
        for want in [
            "-p",
            "--strict-mcp-config",
            "mcp__concat",
            "Bash",
            "WebSearch",
            "60",
        ] {
            assert!(args.iter().any(|a| a == want), "missing {want} in {args:?}");
        }
        assert!(!args.iter().any(|a| a == "--resume"));
        // Claude Code's own tools (Read, Glob, Grep...) are switched off.
        let tools = args.iter().position(|a| a == "--tools").expect("--tools");
        assert_eq!(args[tools + 1], "");
    }

    #[test]
    fn args_resume_when_session() {
        let mut spec = spec();
        spec.resume = Some("abc".into());
        let args = strings(&claude_args(&spec));
        assert_eq!(&args[args.len() - 2..], ["--resume", "abc"]);
    }

    #[test]
    fn mcp_config_escapes_path() {
        let mut spec = spec();
        spec.concat_exe = PathBuf::from(r"D:\OneDrive - xistatu\x\concat.exe");
        spec.api_token = None;
        let json: serde_json::Value = serde_json::from_str(&mcp_config_json(&spec)).unwrap();
        let server = &json["mcpServers"]["concat"];
        assert_eq!(server["command"], r"D:\OneDrive - xistatu\x\concat.exe");
        assert_eq!(server["args"], serde_json::json!(["--mcp", "window"]));
        assert_eq!(server["env"]["CONCAT_API_ADDRESS"], "127.0.0.1:7420");
        assert!(server["env"].get("CONCAT_API_TOKEN").is_none());
    }

    #[test]
    fn prompt_mentions_path_and_vietnamese() {
        let prompt = system_prompt(r"D:\Concat\thu-live");
        assert!(prompt.contains(r"D:\Concat\thu-live"));
        assert!(prompt.contains("Vietnamese"));
        assert!(prompt.contains("project_save"));
    }
}
