// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors
//! The Claude chat column (velarista fork): the window's side of
//! `concat-chat`. It answers the `Chat` global's callbacks, builds each
//! message's playhead context from the window's own state, runs Claude Code
//! against this window's server (live mode) and lands its replies in the
//! column. ui/chat/chat-sidebar.slint draws it.

use std::path::PathBuf;

use concat_chat::claude::{Lookup, RunSpec, claude_args, find_claude, system_prompt};
use concat_chat::context::{ContextInput, compose_message, playhead_context};
use concat_chat::runner::Runner;
use concat_chat::session::SessionStore;
use concat_chat::stream::ChatEvent;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::host::Shell;
use crate::i18n::{t, tf};
use crate::studio::Studio;
use crate::ui::{App, Chat, ChatBubble};

/// The most lines the column keeps; older ones scroll away for good.
pub const HISTORY: usize = 200;
/// The narrowest the open column may be dragged.
const MIN_WIDTH: f32 = 260.0;
/// The open column's width before anyone drags it.
const DEFAULT_WIDTH: f32 = 340.0;

/// One line of the conversation, as the column shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub role: &'static str,
    pub text: String,
}

/// The column's state, kept in [`Studio`].
#[derive(Default)]
pub struct ChatState {
    runner: Option<Runner>,
    lines: Vec<Line>,
    /// The project folder the running turn belongs to.
    project: Option<String>,
    unread: bool,
    live_off: bool,
    where_note: String,
    /// The width when the splitter was pressed.
    width_at_press: f32,
}

/// What pressing Send does.
#[derive(Debug, PartialEq)]
pub enum SendDecision {
    NoProject,
    LiveOff,
    Go,
}

/// Whether a message can go: a project open, and the window lending it to
/// scripts (Settings › Automation, both switches).
pub fn send_decision(server_on: bool, live_on: bool, has_project: bool) -> SendDecision {
    if !has_project {
        SendDecision::NoProject
    } else if !server_on || !live_on {
        SendDecision::LiveOff
    } else {
        SendDecision::Go
    }
}

/// Adds a line, dropping the oldest past [`HISTORY`].
pub fn push_line(lines: &mut Vec<Line>, line: Line) {
    lines.push(line);
    if lines.len() > HISTORY {
        let extra = lines.len() - HISTORY;
        lines.drain(..extra);
    }
}

/// Whether an event of the turn for `turn_project` still belongs on screen
/// now that `open_project` is open.
pub fn belongs(turn_project: Option<&str>, open_project: Option<&str>) -> bool {
    turn_project.is_some() && turn_project == open_project
}

/// `host:port` to connect to: a server listening on every address is
/// reached on this machine's own.
fn connect_address(listen: &str) -> String {
    match listen.rsplit_once(':') {
        Some((host, port)) if host.is_empty() || host == "0.0.0.0" || host == "[::]" => {
            format!("127.0.0.1:{port}")
        }
        _ => listen.to_owned(),
    }
}

fn sessions(studio: &Studio) -> SessionStore {
    SessionStore::new(studio.host.dirs.data.join("chat"))
}

fn open_project(studio: &Studio) -> Option<String> {
    studio
        .session
        .as_ref()
        .map(|session| session.path().to_owned())
}

/// Wires the `Chat` global and restores the column as it was left. Called
/// once, after [`Shell::install`].
pub fn install(app: &App) {
    let chat = app.global::<Chat>();
    Shell::with(|shell, app| {
        let studio = shell.studio.borrow();
        let chat = app.global::<Chat>();
        chat.set_open(!studio.prefs.chat_closed);
        chat.set_width(
            studio
                .prefs
                .chat_width
                .unwrap_or(DEFAULT_WIDTH)
                .max(MIN_WIDTH),
        );
    });

    chat.on_send(|text: SharedString| with_chat(|studio, _| send(studio, text.to_string())));
    chat.on_stop(|| {
        with_chat(|studio, _| {
            if let Some(runner) = &studio.chat.runner {
                runner.stop();
            }
        })
    });
    chat.on_new_conversation(|| {
        with_chat(|studio, _| {
            if let Some(project) = open_project(studio) {
                sessions(studio).clear(&project);
            }
            studio.chat.lines.clear();
            studio.chat.where_note.clear();
        })
    });
    chat.on_toggle(|| {
        with_chat(|studio, app| {
            let open = !app.global::<Chat>().get_open();
            app.global::<Chat>().set_open(open);
            if open {
                studio.chat.unread = false;
            }
            studio.prefs.chat_closed = !open;
            studio.prefs.save(&studio.host.dirs);
        })
    });
    chat.on_resize_started(|| {
        with_chat(|studio, app| studio.chat.width_at_press = app.global::<Chat>().get_width());
    });
    chat.on_resized(|delta: f32| {
        with_chat(|studio, app| {
            let window = app.window();
            let half = window.size().width as f32 / window.scale_factor() / 2.0;
            let width = (studio.chat.width_at_press + delta).clamp(MIN_WIDTH, half.max(MIN_WIDTH));
            app.global::<Chat>().set_width(width);
            studio.prefs.chat_width = Some(width);
            studio.prefs.save(&studio.host.dirs);
        })
    });
    chat.on_enable_live(|| {
        Shell::with(|shell, app| {
            {
                let mut studio = shell.studio.borrow_mut();
                use crate::panes::Msg;
                use crate::panes::settings::SettingsMsg;
                if !studio.prefs.server.enabled {
                    studio.handle(Msg::Settings(SettingsMsg::ServerEnabledChanged(true)));
                }
                if !studio.prefs.server.live {
                    studio.handle(Msg::Settings(SettingsMsg::ServerLiveChanged(true)));
                }
                studio.chat.live_off = !(studio.prefs.server.enabled && studio.prefs.server.live);
            }
            shell.studio.borrow().publish(&app, &shell.models);
            publish(&shell.studio.borrow(), &app);
        });
    });
}

/// Runs `body` on the window's state, then shows the column again.
fn with_chat(body: impl FnOnce(&mut Studio, &App)) {
    Shell::with(|shell, app| {
        body(&mut shell.studio.borrow_mut(), &app);
        publish(&shell.studio.borrow(), &app);
    });
}

fn say(studio: &mut Studio, role: &'static str, text: String) {
    push_line(&mut studio.chat.lines, Line { role, text });
}

fn send(studio: &mut Studio, text: String) {
    let text = text.trim().to_owned();
    if text.is_empty() || studio.chat.runner.as_ref().is_some_and(Runner::is_running) {
        return;
    }
    let server = &studio.prefs.server;
    match send_decision(server.enabled, server.live, studio.session.is_some()) {
        SendDecision::NoProject => {
            say(studio, "error", t("chat.noProject"));
            return;
        }
        SendDecision::LiveOff => {
            studio.chat.live_off = true;
            return;
        }
        SendDecision::Go => studio.chat.live_off = false,
    }
    let Some(claude) = find_claude(&Lookup::from_env()) else {
        say(studio, "me", text);
        say(studio, "error", t("chat.notFound"));
        return;
    };
    let Some(session) = studio.session.as_ref() else {
        return;
    };
    let project_path = session.path().to_owned();
    let context = playhead_context(&ContextInput {
        project_path: &project_path,
        project: session.project(),
        playhead: f64::from(studio.playhead),
        playing: studio.playing,
        selection: &studio.selection,
    });
    let seconds = studio.playhead.max(0.0) as u64;
    studio.chat.where_note = tf(
        "chat.attachedAt",
        &[&format!("{:02}:{:02}", seconds / 60, seconds % 60)],
    );
    let spec = RunSpec {
        claude: claude.clone(),
        concat_exe: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("concat")),
        api_address: connect_address(&studio.prefs.server.listen),
        api_token: Some(studio.prefs.server.token.clone()).filter(|token| !token.is_empty()),
        system_prompt: system_prompt(&project_path),
        resume: sessions(studio).get(&project_path),
    };
    say(studio, "me", text.clone());
    let turn_project = project_path.clone();
    let started = Runner::start(
        claude,
        claude_args(&spec),
        compose_message(&context, &text),
        Some(PathBuf::from(&project_path)),
        move |event| {
            let turn_project = turn_project.clone();
            // The window's state is only touched on the event loop.
            let _ = slint::invoke_from_event_loop(move || {
                with_chat(|studio, app| on_event(studio, app, &turn_project, event));
            });
        },
    );
    match started {
        Ok(runner) => {
            studio.chat.runner = Some(runner);
            studio.chat.project = Some(project_path);
        }
        Err(error) => say(studio, "error", format!("{}: {error}", t("chat.notFound"))),
    }
}

fn on_event(studio: &mut Studio, app: &App, turn_project: &str, event: ChatEvent) {
    if !belongs(Some(turn_project), open_project(studio).as_deref()) {
        // The project changed under the turn: its replies are not this one's.
        if matches!(event, ChatEvent::Done { .. }) {
            studio.chat.project = None;
        }
        return;
    }
    match event {
        ChatEvent::Text(text) => say(studio, "claude", text),
        ChatEvent::Tool { name } => say(studio, "tool", tf("chat.usingTool", &[&name])),
        ChatEvent::Error(text) => say(studio, "error", text),
        ChatEvent::Done { session_id } => {
            if let Some(id) = session_id {
                sessions(studio).set(turn_project, &id);
            }
            studio.chat.project = None;
        }
    }
    if !app.global::<Chat>().get_open() {
        studio.chat.unread = true;
    }
}

/// Shows the column's state.
pub fn publish(studio: &Studio, app: &App) {
    let chat = app.global::<Chat>();
    let bubbles: Vec<ChatBubble> = studio
        .chat
        .lines
        .iter()
        .map(|line| ChatBubble {
            role: line.role.into(),
            text: line.text.as_str().into(),
        })
        .collect();
    chat.set_bubbles(ModelRc::new(VecModel::from(bubbles)));
    chat.set_busy(studio.chat.runner.as_ref().is_some_and(Runner::is_running));
    chat.set_unread(studio.chat.unread);
    chat.set_live_off(studio.chat.live_off);
    chat.set_where(studio.chat.where_note.as_str().into());
    // Each publish follows a change in the conversation or the column.
    chat.set_scroll_tick(chat.get_scroll_tick().wrapping_add(1));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_refused_when_live_off() {
        assert_eq!(send_decision(true, false, true), SendDecision::LiveOff);
        assert_eq!(send_decision(false, true, true), SendDecision::LiveOff);
        assert_eq!(send_decision(true, true, false), SendDecision::NoProject);
        assert_eq!(send_decision(true, true, true), SendDecision::Go);
    }

    #[test]
    fn history_capped_at_200() {
        let mut lines = Vec::new();
        for i in 0..250 {
            push_line(
                &mut lines,
                Line {
                    role: "me",
                    text: i.to_string(),
                },
            );
        }
        assert_eq!(lines.len(), HISTORY);
        assert_eq!(lines[0].text, "50");
        assert_eq!(lines.last().unwrap().text, "249");
    }

    #[test]
    fn events_for_other_project_dropped() {
        assert!(belongs(Some("D:/a"), Some("D:/a")));
        assert!(!belongs(Some("D:/a"), Some("D:/b")));
        assert!(!belongs(Some("D:/a"), None));
        assert!(!belongs(None, None));
    }

    #[test]
    fn listen_on_all_addresses_connects_locally() {
        assert_eq!(connect_address("0.0.0.0:7420"), "127.0.0.1:7420");
        assert_eq!(connect_address("127.0.0.1:7420"), "127.0.0.1:7420");
    }
}
