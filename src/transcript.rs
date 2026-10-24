//! A session's transcript, boiled down for the writer subagent: what the user said and what the
//! agent answered, without tool calls, tool output, system reminders or subagents' own talk.
//!
//! Understands both Claude Code (`user` / `assistant` JSON lines) and Google Antigravity
//! (`USER_INPUT` / `PLANNER_RESPONSE` JSON lines, reusing `cthg_core::antigravity`). Only text
//! turns between the user and the main agent count. Reading starts at the line where the last
//! saving stopped, so the same conversation is never sent to the writer twice.

use cthg_core::antigravity::{self, Step};
use cthg_core::json::Json;
use cthg_core::text;

/// Characters sent at most (the most recent ones): about 6 000 tokens.
pub const MAX_CHARS: usize = 24_000;
const USER_CLIP: usize = 1_500;
const AGENT_CLIP: usize = 900;

/// Transcript format: detected automatically line by line, or forced via `--formato`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Auto,
    Claude,
    Antigravity,
}

impl Format {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "auto" => Some(Format::Auto),
            "claude" | "claude-code" => Some(Format::Claude),
            "antigravity" => Some(Format::Antigravity),
            _ => None,
        }
    }
}

/// Drops `<system-reminder>…</system-reminder>` and similar injected or metadata blocks.
fn strip_injected(s: &str) -> String {
    let mut cur = s.to_string();
    for tag in [
        "system-reminder",
        "command-message",
        "local-command-stdout",
        "ADDITIONAL_METADATA",
        "USER_SETTINGS_CHANGE",
    ] {
        let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
        let mut kept = String::new();
        let mut rest = cur.as_str();
        while let Some(i) = rest.find(&open) {
            kept.push_str(&rest[..i]);
            rest = match rest[i..].find(&close) {
                Some(j) => &rest[i + j + close.len()..],
                None => "",
            };
        }
        kept.push_str(rest);
        cur = kept;
    }
    cur.trim().to_string()
}

fn texts(content: &Json) -> Vec<String> {
    match content {
        Json::Str(s) => vec![s.clone()],
        Json::Arr(blocks) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(Json::str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Json::str).map(String::from))
            .collect(),
        _ => vec![],
    }
}

/// The dialogue from line `from` on, auto-detecting the transcript format.
pub fn compact(transcript: &str, from: usize, user: &str, agent: &str) -> (String, usize) {
    compact_with_format(transcript, from, user, agent, Format::Auto)
}

/// The dialogue from line `from` on with an explicit or auto-detected format, and how many lines
/// the transcript has.
pub fn compact_with_format(
    transcript: &str,
    from: usize,
    user: &str,
    agent: &str,
    format: Format,
) -> (String, usize) {
    let lines: Vec<&str> = transcript.trim_start_matches('\u{feff}').lines().collect();
    let mut turns: Vec<String> = Vec::new();
    for raw in lines.iter().skip(from) {
        let Some(v) = Json::parse(raw) else { continue };
        let kind = v.get("type").and_then(Json::str).unwrap_or("");
        if format != Format::Antigravity && matches!(kind, "user" | "assistant") {
            if matches!(v.get("isSidechain"), Some(Json::Bool(true)))
                || matches!(v.get("isMeta"), Some(Json::Bool(true)))
            {
                continue;
            }
            let (who, clip) = if kind == "user" { (user, USER_CLIP) } else { (agent, AGENT_CLIP) };
            let Some(content) = v.get("message").and_then(|m| m.get("content")) else { continue };
            let said: Vec<String> = texts(content)
                .iter()
                .map(|t| strip_injected(t))
                .filter(|t| !t.is_empty())
                .collect();
            if !said.is_empty() {
                turns.push(format!("{who}: {}", text::clip(&said.join("\n"), clip)));
            }
            continue;
        }
        if format != Format::Claude {
            if kind == "PLANNER_RESPONSE" {
                if let Some(c) = v.get("content").and_then(Json::str) {
                    let said = strip_injected(c);
                    if !said.is_empty() {
                        turns.push(format!("{agent}: {}", text::clip(&said, AGENT_CLIP)));
                    }
                }
            } else {
                let mut steps = Vec::new();
                antigravity::line_steps(&v, &mut steps);
                for s in steps {
                    if let Step::User { text: t, .. } = s {
                        let said = strip_injected(&t);
                        if !said.is_empty() {
                            turns.push(format!("{user}: {}", text::clip(&said, USER_CLIP)));
                        }
                    }
                }
            }
        }
    }
    let mut out = turns.join("\n\n");
    if out.len() > MAX_CHARS {
        // Keep the end: the latest part is what has not been saved yet.
        let mut cut = out.len() - MAX_CHARS;
        while !out.is_char_boundary(cut) {
            cut += 1;
        }
        out = format!("[…]\n{}", &out[cut..]);
    }
    (out, lines.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = r#"{"type":"summary","summary":"x"}
{"type":"user","message":{"role":"user","content":"<system-reminder>reglas</system-reminder>No preguntes el alcance."}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Tienes razón."},{"type":"tool_use","name":"Edit","input":{}}]}}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"ok"}]}}
{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"charla de un subagente"}]}}
{"type":"user","message":{"role":"user","content":"Ahora el nado."}}"#;

    #[test]
    fn only_what_was_said_without_tools_reminders_or_subagents() {
        let (out, total) = compact(T, 0, "Ana", "Agente");
        assert_eq!(total, 6);
        assert_eq!(out, "Ana: No preguntes el alcance.\n\nAgente: Tienes razón.\n\nAna: Ahora el nado.");
    }

    #[test]
    fn it_starts_where_the_last_saving_stopped() {
        let (out, _) = compact(T, 4, "Ana", "Agente");
        assert_eq!(out, "Ana: Ahora el nado.");
        assert_eq!(compact(T, 6, "Ana", "Agente").0, "");
    }

    #[test]
    fn a_long_session_keeps_its_end() {
        let line = r#"{"type":"user","message":{"content":"palabra palabra palabra palabra palabra palabra palabra palabra"}}"#;
        let long = vec![line; 2000].join("\n");
        let (out, _) = compact(&long, 0, "Ana", "Agente");
        assert!(out.len() <= MAX_CHARS + 10 && out.starts_with("[…]"));
    }

    const AGY: &str = concat!(
        r#"{"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","content":"<USER_REQUEST>\nSepara la memoria por agente.\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\n2026-10-24\n</ADDITIONAL_METADATA>"}"#, "\n",
        r##"{"step_index":1,"source":"SYSTEM_SDK","type":"EPHEMERAL_MESSAGE","status":"DONE","content":"# Memoria global de Argon"}"##, "\n",
        r#"{"step_index":2,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","thinking":"leo archivos","tool_calls":[{"name":"read_file","args":{"Path":"src/store.rs"}}]}"#, "\n",
        r#"{"step_index":3,"source":"MODEL","type":"GENERIC","status":"DONE","content":"1: pub fn dir()"}"#, "\n",
        r#"{"step_index":4,"source":"SYSTEM","type":"SYSTEM_MESSAGE","status":"DONE","content":"<SYSTEM_MESSAGE>\n[Message] sender=abc/task-1 content=cargo test ok\n</SYSTEM_MESSAGE>"}"#, "\n",
        r#"{"step_index":5,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","content":"Listo: cada agente guarda en su carpeta."}"#, "\n",
        r#"{"step_index":6,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","content":"<USER_REQUEST>\nAñade los tests del hook.\n</USER_REQUEST>"}"#,
    );

    #[test]
    fn antigravity_transcript_only_keeps_user_and_agent_dialogue() {
        let (out, total) = compact(AGY, 0, "Luis", "Argon");
        assert_eq!(total, 7);
        assert_eq!(
            out,
            "Luis: Separa la memoria por agente.\n\nArgon: Listo: cada agente guarda en su carpeta.\n\nLuis: Añade los tests del hook."
        );
        let (from_five, _) = compact_with_format(AGY, 5, "Luis", "Argon", Format::Antigravity);
        assert_eq!(from_five, "Argon: Listo: cada agente guarda en su carpeta.\n\nLuis: Añade los tests del hook.");
        assert_eq!(compact_with_format(AGY, 0, "Luis", "Argon", Format::Claude).0, "");
    }
}
