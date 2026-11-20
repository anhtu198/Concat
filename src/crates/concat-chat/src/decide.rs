// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors
//! The chat column's small decisions, kept out of the window crate so they
//! are tested on their own: whether a message can go, which server Claude's
//! MCP reaches, which events still belong on screen, how long the history is.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// The most lines the column keeps; older ones scroll away for good.
pub const HISTORY: usize = 200;

/// One line of the conversation, as the column shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// "me", "claude", "tool" or "error".
    pub role: &'static str,
    pub text: String,
}

/// What pressing Send does.
#[derive(Debug, PartialEq)]
pub enum SendDecision {
    NoProject,
    LiveOff,
    Go,
}

/// Whether a message can go: a project open, the window's API server
/// actually running, and the window lending its project to scripts.
pub fn send_decision(server_running: bool, live_on: bool, has_project: bool) -> SendDecision {
    if !has_project {
        SendDecision::NoProject
    } else if !server_running || !live_on {
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

/// `CONCAT_API_ADDRESS` and `CONCAT_API_TOKEN` for the running server: its
/// bound address (a wildcard reached on this machine's own loopback) and
/// the token it actually checks, minted when Settings left it empty.
pub fn link(addr: SocketAddr, token: &str) -> (String, String) {
    let ip = match addr.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    (
        SocketAddr::new(ip, addr.port()).to_string(),
        token.to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_refused_when_server_not_running() {
        assert_eq!(send_decision(false, true, true), SendDecision::LiveOff);
        assert_eq!(send_decision(true, false, true), SendDecision::LiveOff);
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
    fn link_uses_running_servers_token() {
        let addr: SocketAddr = "127.0.0.1:7420".parse().unwrap();
        assert_eq!(
            link(addr, "minted"),
            ("127.0.0.1:7420".to_owned(), "minted".to_owned())
        );
    }

    #[test]
    fn link_reaches_wildcard_listeners_locally() {
        let v4: SocketAddr = "0.0.0.0:7420".parse().unwrap();
        let v6: SocketAddr = "[::]:7420".parse().unwrap();
        assert_eq!(link(v4, "t").0, "127.0.0.1:7420");
        assert_eq!(link(v6, "t").0, "[::1]:7420");
    }
}
