// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors
//! One real turn through the Claude Code on this machine, with the exact
//! command line the chat column uses: run by hand, it spends a little of
//! the subscription (`cargo test -p concat-chat --test real_claude -- --ignored`).
//! Concat's MCP is pointed at an address nothing listens on, so no project
//! is touched; the turn must still answer, with no tools.

use std::sync::mpsc;
use std::time::Duration;

use concat_chat::claude::{Lookup, RunSpec, claude_args, find_claude, launch_for, system_prompt};
use concat_chat::runner::Runner;
use concat_chat::stream::ChatEvent;

#[test]
#[ignore = "spends a real Claude turn"]
fn real_claude_turn() {
    let lookup = Lookup::from_env();
    let claude = find_claude(&lookup).expect("Claude Code on this machine");
    let spec = RunSpec {
        claude: claude.clone(),
        concat_exe: std::env::current_exe().unwrap(),
        api_address: "127.0.0.1:9".into(),
        api_token: Some("none".into()),
        system_prompt: system_prompt(r"C:\nowhere"),
        resume: None,
    };
    let (program, mut args) = launch_for(&claude, &lookup);
    args.extend(claude_args(&spec));
    let (tx, rx) = mpsc::channel();
    let _runner = Runner::start(
        program,
        args,
        "Reply with exactly the word: PONG".into(),
        Some(std::env::temp_dir()),
        move |event| {
            let _ = tx.send(event);
        },
    )
    .unwrap();
    let mut events = Vec::new();
    while let Ok(event) = rx.recv_timeout(Duration::from_secs(180)) {
        let done = matches!(event, ChatEvent::Done { .. });
        events.push(event);
        if done {
            break;
        }
    }
    println!("{events:#?}");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ChatEvent::Text(t) if t.contains("PONG"))),
        "{events:?}"
    );
    assert!(matches!(
        events.last(),
        Some(ChatEvent::Done {
            session_id: Some(_)
        })
    ));
}
