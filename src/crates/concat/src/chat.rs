// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors
//! The Claude chat column (velarista fork): the window's side of
//! `concat-chat`. It answers the `Chat` global's callbacks, builds each
//! message's playhead context from the window's own state, runs Claude Code
//! against this window's server (live mode) and lands its replies in the
//! column. ui/chat/chat-sidebar.slint draws it.

use std::path::PathBuf;

use concat_chat::claude::{Lookup, RunSpec, claude_args, find_claude, launch_for, system_prompt};
use concat_chat::context::{ContextInput, compose_message, playhead_context};
use concat_chat::decide::{Line, SendDecision, belongs, link, push_line, send_decision};
use concat_chat::runner::Runner;
use concat_chat::session::SessionStore;
use concat_chat::stream::ChatEvent;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::host::Shell;
use crate::i18n::{t, tf};
use crate::studio::Studio;
use crate::ui::{App, Chat, ChatBubble};

/// The narrowest the open column may be dragged.
const MIN_WIDTH: f32 = 260.0;
/// The open column's width before anyone drags it.
const DEFAULT_WIDTH: f32 = 340.0;

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
    /// Saves the dragged width once the drag has paused, not at every move.
    save_width: slint::Timer,
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

    // True when the message went (or was answered with an error line), so
    // the box empties; false keeps the words for after the fix.
    chat.on_send(|text: SharedString| {
        let mut sent = false;
        with_chat(|studio, _| sent = send(studio, text.to_string()));
        sent
    });
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
    // A drag is a stream of moves: the width alone changes, the conversation
    // is not republished, and the preferences are written once it pauses.
    chat.on_resize_started(|| {
        Shell::with(|shell, app| {
            shell.studio.borrow_mut().chat.width_at_press = app.global::<Chat>().get_width();
        });
    });
    chat.on_resized(|delta: f32| {
        Shell::with(|shell, app| {
            let mut studio = shell.studio.borrow_mut();
            let window = app.window();
            let half = window.size().width as f32 / window.scale_factor() / 2.0;
            let width = (studio.chat.width_at_press + delta).clamp(MIN_WIDTH, half.max(MIN_WIDTH));
            app.global::<Chat>().set_width(width);
            studio.prefs.chat_width = Some(width);
            studio.chat.save_width.start(
                slint::TimerMode::SingleShot,
                std::time::Duration::from_millis(400),
                || {
                    Shell::with(|shell, _| {
                        let studio = shell.studio.borrow();
                        studio.prefs.save(&studio.host.dirs);
                    });
                },
            );
        });
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
                studio.chat.live_off = !(studio.host.server.is_some() && studio.prefs.server.live);
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

/// Sends one message; false when it could not go and the words should stay
/// in the box (no project, or the window not lending its project).
fn send(studio: &mut Studio, text: String) -> bool {
    let text = text.trim().to_owned();
    if text.is_empty() || studio.chat.runner.as_ref().is_some_and(Runner::is_running) {
        return false;
    }
    // The server as it runs, not as Settings asks for it: a switch left on
    // with a bad address or a weak token starts nothing.
    let server = studio
        .host
        .server
        .as_ref()
        .and_then(|server| server.json_addr().map(|addr| link(addr, server.token())));
    match send_decision(
        server.is_some(),
        studio.prefs.server.live,
        studio.session.is_some(),
    ) {
        SendDecision::NoProject => {
            say(studio, "error", t("chat.noProject"));
            return false;
        }
        SendDecision::LiveOff => {
            studio.chat.live_off = true;
            return false;
        }
        SendDecision::Go => studio.chat.live_off = false,
    }
    let Some((api_address, api_token)) = server else {
        return false;
    };
    let lookup = Lookup::from_env();
    let Some(claude) = find_claude(&lookup) else {
        say(studio, "me", text);
        say(studio, "error", t("chat.notFound"));
        return true;
    };
    let Some(session) = studio.session.as_ref() else {
        return false;
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
        api_address,
        api_token: Some(api_token),
        system_prompt: system_prompt(&project_path),
        resume: sessions(studio).get(&project_path),
    };
    say(studio, "me", text.clone());
    let turn_project = project_path.clone();
    // An npm install's shim runs as `node <script>`; see `launch_for`.
    let (program, mut args) = launch_for(&claude, &lookup);
    args.extend(claude_args(&spec));
    let started = Runner::start(
        program,
        args,
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
    true
}

/// The window opened another project or closed this one: a reply still
/// running is stopped (its edits could only be refused now) and the
/// conversation on screen is cleared; the next project's own is resumed
/// from its session when the person writes. Called by `project_files.rs`.
pub fn project_changed(studio: &mut Studio) {
    if let Some(runner) = studio.chat.runner.take() {
        runner.stop();
    }
    let had_lines = !studio.chat.lines.is_empty();
    studio.chat.lines.clear();
    studio.chat.where_note.clear();
    studio.chat.project = None;
    studio.chat.unread = false;
    if had_lines {
        // Shown after the caller's borrow of the window's state ends.
        let _ = slint::invoke_from_event_loop(|| with_chat(|_, _| {}));
    }
}

fn on_event(studio: &mut Studio, app: &App, turn_project: &str, event: ChatEvent) {
    if !belongs(Some(turn_project), open_project(studio).as_deref()) {
        // The project changed under the turn: its replies are not this
        // one's, and a turn still running is ended (a backstop to
        // `project_changed`).
        if let Some(runner) = &studio.chat.runner
            && studio.chat.project.as_deref() == Some(turn_project)
        {
            runner.stop();
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
