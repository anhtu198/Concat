// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors
//! What each chat message carries about where the person is: the playhead,
//! the selection, and the clips under the playhead, so "chỗ này" means
//! something to Claude. The shape follows ZataCut's `playhead_context`.

use std::fmt::Write as _;

use concat_project::model::{Clip, ClipKind, Project};

/// What the window knows at the moment a message is sent.
pub struct ContextInput<'a> {
    /// The project folder, as the MCP tools take it.
    pub project_path: &'a str,
    /// The document as it stands.
    pub project: &'a Project,
    /// Seconds on the active timeline.
    pub playhead: f64,
    /// Whether the cut is playing.
    pub playing: bool,
    /// The ids of the selected clips.
    pub selection: &'a [String],
}

/// The `[PLAYHEAD CONTEXT]` block one message starts with.
pub fn playhead_context(input: &ContextInput) -> String {
    let project = input.project;
    let timeline = project
        .timelines
        .iter()
        .find(|timeline| timeline.id == project.active_timeline_id)
        .or_else(|| project.timelines.first());
    let (num, den) = timeline
        .map(|timeline| (timeline.video.rate_num, timeline.video.rate_den))
        .unwrap_or((30, 1));
    let fps = if num > 0 && den > 0 {
        num as f64 / den as f64
    } else {
        30.0
    };
    let t = input.playhead.max(0.0);
    let frame = (t * fps + 1e-6).floor() as u64;
    let whole = fps.round().max(1.0) as u64;
    let seconds = frame / whole;
    let timecode = format!(
        "{:02}:{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60,
        frame % whole
    );

    let mut out = String::from("[PLAYHEAD CONTEXT]\n");
    let _ = writeln!(out, "project: {}", input.project_path);
    let _ = writeln!(
        out,
        "time: {t:.3} s (timecode {timecode}, frame {frame} @ {} fps)",
        round3(fps)
    );
    let _ = writeln!(
        out,
        "state: {}",
        if input.playing { "playing" } else { "paused" }
    );
    let selected = if input.selection.is_empty() {
        "none".to_owned()
    } else {
        input.selection.join(", ")
    };
    let _ = writeln!(out, "selected: {selected}");

    let clips: Vec<&Clip> = timeline
        .map(|timeline| timeline.clips.iter().map(|clip| &**clip).collect())
        .unwrap_or_default();
    let length = clips
        .iter()
        .map(|clip| clip.start + clip.duration)
        .fold(0.0_f64, f64::max);
    let _ = writeln!(out, "timeline length: {length:.3} s");

    let mut under: Vec<&Clip> = clips
        .into_iter()
        .filter(|clip| clip.start <= t && t < clip.start + clip.duration)
        .collect();
    under.sort_by(|a, b| a.start.total_cmp(&b.start));
    if under.is_empty() {
        out.push_str("under the playhead: none\n");
    } else {
        out.push_str("under the playhead:\n");
        for clip in under {
            let label = clip
                .text
                .as_ref()
                .filter(|_| clip.kind == ClipKind::Text)
                .map(|style| style.content.as_str())
                .unwrap_or(&clip.name);
            let label: String = label.replace('\n', " ").chars().take(80).collect();
            let _ = writeln!(
                out,
                "- {} {} lane {} \"{label}\" {:.3}-{:.3} s, {:.3} s into the clip",
                clip.id,
                format!("{:?}", clip.kind).to_lowercase(),
                clip.track_id,
                clip.start,
                clip.start + clip.duration,
                t - clip.start
            );
        }
    }
    out.push_str("[/PLAYHEAD CONTEXT]\n");
    out
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// The message as Claude receives it: the context, then the person's words.
pub fn compose_message(context: &str, owner_text: &str) -> String {
    format!("{}\n\n{owner_text}", context.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use concat_project::model::{Clip, ClipKind, Project, TextStyle, VideoSettings};
    use std::sync::Arc;

    fn project_with_title() -> Project {
        let mut project = Project::with_video(VideoSettings {
            rate_num: 24,
            rate_den: 1,
            ..VideoSettings::default()
        });
        let timeline = Arc::make_mut(&mut project.timelines[0]);
        let track = timeline.tracks[0].id.clone();
        timeline.clips.push(Arc::new(Clip {
            id: "c1".into(),
            track_id: track,
            name: "Xin chào".into(),
            kind: ClipKind::Text,
            start: 0.0,
            duration: 5.0,
            text: Some(TextStyle {
                content: "Xin chào anh".into(),
                ..TextStyle::default()
            }),
            ..Clip::default()
        }));
        project
    }

    fn input<'a>(project: &'a Project, playhead: f64, selection: &'a [String]) -> ContextInput<'a> {
        ContextInput {
            project_path: r"D:\OneDrive - xistatu\Desktop\Concat\thu-live",
            project,
            playhead,
            playing: false,
            selection,
        }
    }

    #[test]
    fn context_lists_clip_under_playhead() {
        let project = project_with_title();
        let text = playhead_context(&input(&project, 2.5, &[]));
        assert!(text.starts_with("[PLAYHEAD CONTEXT]"));
        assert!(text.trim_end().ends_with("[/PLAYHEAD CONTEXT]"));
        let line = text
            .lines()
            .find(|line| line.starts_with("- c1"))
            .expect("c1 listed");
        assert!(line.contains("text"), "{line}");
        assert!(line.contains("\"Xin chào anh\""), "{line}");
        assert!(line.contains("0.000-5.000 s"), "{line}");
        assert!(line.contains("2.500 s into the clip"), "{line}");
    }

    #[test]
    fn context_none_when_empty() {
        let project = project_with_title();
        let text = playhead_context(&input(&project, 7.0, &[]));
        assert!(text.contains("under the playhead: none"), "{text}");
        assert!(text.contains("selected: none"), "{text}");
    }

    #[test]
    fn context_marks_selection_and_playing() {
        let project = project_with_title();
        let selection = vec!["c1".to_owned()];
        let mut input = input(&project, 1.0, &selection);
        input.playing = true;
        let text = playhead_context(&input);
        assert!(text.contains("state: playing"), "{text}");
        assert!(text.contains("selected: c1"), "{text}");
    }

    #[test]
    fn context_keeps_unicode_path() {
        let project = project_with_title();
        let text = playhead_context(&input(&project, 1.0, &[]));
        assert!(text.contains(r"project: D:\OneDrive - xistatu\Desktop\Concat\thu-live"));
    }

    #[test]
    fn timecode_at_24fps() {
        let project = project_with_title();
        let text = playhead_context(&input(&project, 2.5, &[]));
        assert!(text.contains("00:00:02:12"), "{text}");
        assert!(text.contains("frame 60 @ 24 fps"), "{text}");
    }

    #[test]
    fn compose_puts_context_first() {
        assert_eq!(compose_message("CTX", "đổi màu"), "CTX\n\nđổi màu");
    }
}
