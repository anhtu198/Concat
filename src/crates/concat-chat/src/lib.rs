// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors
//! Claude Code as a chat beside the timeline (velarista fork).
//!
//! Everything here is plain Rust with no window in it, so it is tested on
//! its own; the window crate only draws the column and hands this the
//! playhead, the selection and the project.

pub mod claude;
pub mod context;
pub mod runner;
pub mod session;
pub mod stream;
