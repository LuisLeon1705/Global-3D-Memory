//! `cgm` — an AI agent's own global 3D memory (CTHG), across every session and project.
//!
//! - Reading is free: hooks (`cgm hook session-start`, `cgm hook prompt`, `cgm hook antigravity`)
//!   inject what applies, with no model involved.
//! - Writing is a small subagent's: at the moments worth it (a correction, a decision, the end
//!   of real work), the agent hands the session to a cheap subagent, which reads `cgm sesion`
//!   and writes `cgm anotar`. Nothing runs on its own in a loop: usage costs.

mod inject;
mod place;
mod store;
mod transcript;
mod write;

use cthg_core::hook::HookInput;
use cthg_core::learn;
use cthg_core::model::{flags, Memory};
use cthg_core::retrieve::{self, Query};
use cthg_core::score::{self, certainty_tag};
use cthg_core::spatial::Target;
use cthg_core::text;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

const HELP: &str = "cgm — memoria global 3D de un agente de IA (solo suya)

  cgm hook session-start|prompt|antigravity   (lee el JSON del hook por stdin)
  cgm sesion --id <ID> [--todo] [--formato claude|antigravity]
  cgm anotar [--sesion <ID>] [--cwd <DIR>] [ARCHIVO|-]   guarda una lista JSON de entradas
  cgm recordar \"<texto>\" [--cwd <DIR>]  busca en la memoria
  cgm lista [--todo]                   todos los recuerdos con su χ
  cgm agenda                           lo que el agente quiere hacer y aún no hizo
  cgm hecho <id>                       un plan de la agenda se cumplió
  cgm confirmar <id> | cgm negar <id>  más o menos evidencia
  cgm nucleo <id> [--quitar]           que siempre (o ya no) entre al iniciar sesión
  cgm donde [DIR]                      cómo ve la memoria una carpeta (eje X)
  cgm ruta                             dónde está el archivo de la memoria

Variables (o los flags --agente / --usuario, que valen en cualquier shell):
CGM_AGENT (nombre del agente, obligatoria para escribir),
CGM_USER (nombre del usuario en los transcripts), CGM_DIR (carpeta de la memoria),
CTHG_AI_REGISTRY (registro de memorias de IA).";

fn main() {
    let args = take_names(std::env::args().skip(1).collect());
    let cmd = args.first().map(String::as_str).unwrap_or("");
    if cmd == "hook" {
        // A hook never blocks or fails out loud; without CGM_AGENT/CGM_DIR it stays silent.
        let mut input = String::new();
        let _ = std::io::stdin().read_to_string(&mut input);
        if let Some(out) = run_hook_cmd(args.get(1).map(String::as_str), &input) {
            println!("{out}");
        }
        return;
    }
    let dir = store::dir();
    let result = match cmd {
        "sesion" => cmd_session(&dir, &args[1..]),
        "anotar" => cmd_write(&dir, &args[1..]),
        "recordar" => cmd_recall(&dir, &args[1..]),
        "lista" => cmd_list(&dir, args.iter().any(|a| a == "--todo")),
        "agenda" => cmd_agenda(&dir),
        "hecho" => cmd_done(&dir, &args[1..]),
        "confirmar" => cmd_vote(&dir, &args[1..], 1),
        "negar" => cmd_vote(&dir, &args[1..], -1),
        "nucleo" | "núcleo" => cmd_core(&dir, &args[1..]),
        "donde" | "dónde" => cmd_where(&args[1..]),
        "ruta" => {
            println!("{}", store::file(&dir).display());
            Ok(())
        }
        _ => {
            println!("{HELP}");
            Ok(())
        }
    };
    if let Err(e) = result {
        eprintln!("cgm: {e}");
        std::process::exit(1);
    }
}

/// `--agente <nombre>` and `--usuario <nombre>`, anywhere in the line, set `CGM_AGENT` and
/// `CGM_USER`: the same command then works in any shell (cmd, PowerShell, sh), with no
/// shell-specific syntax for environment variables. Returns the arguments without them.
fn take_names(args: Vec<String>) -> Vec<String> {
    let (rest, vars) = split_names(args);
    for (var, v) in vars {
        // Still single-threaded: nothing else reads the environment yet.
        std::env::set_var(var, v);
    }
    rest
}

/// The arguments without `--agente`/`--usuario`, and the variables those set.
fn split_names(args: Vec<String>) -> (Vec<String>, Vec<(&'static str, String)>) {
    let mut rest = Vec::with_capacity(args.len());
    let mut vars = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let var = match a.as_str() {
            "--agente" | "--agent" => "CGM_AGENT",
            "--usuario" | "--user" => "CGM_USER",
            _ => {
                rest.push(a);
                continue;
            }
        };
        if let Some(v) = it.next() {
            vars.push((var, v));
        }
    }
    (rest, vars)
}

fn opt(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn positional(args: &[String]) -> Option<String> {
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
            continue;
        }
        if matches!(a.as_str(), "--cwd" | "--sesion" | "--id" | "--formato" | "--format") {
            skip = true;
            continue;
        }
        if !a.starts_with("--") {
            return Some(a.clone());
        }
    }
    None
}

fn id_arg(args: &[String]) -> Result<u32, String> {
    positional(args).and_then(|s| s.trim_start_matches('#').parse().ok()).ok_or_else(|| "falta el #id".to_string())
}

fn cwd_or(args: &[String], fallback: &str) -> PathBuf {
    match opt(args, "--cwd") {
        Some(c) => PathBuf::from(c),
        None if !fallback.is_empty() => PathBuf::from(fallback),
        None => std::env::current_dir().unwrap_or_default(),
    }
}

/// One load-modify-save under the lock.
fn modify<T>(dir: &Path, f: impl FnOnce(&mut Memory) -> Result<T, String>) -> Result<T, String> {
    store::require_configured()?;
    let _lock = store::lock(dir, Duration::from_secs(5)).ok_or("la memoria está ocupada por otro proceso; inténtalo de nuevo")?;
    let mut m = store::load(dir);
    let out = f(&mut m)?;
    store::save(&m, dir).map_err(|e| format!("no se pudo guardar {}: {e}", store::file(dir).display()))?;
    Ok(out)
}

// ── hooks ──

fn run_hook_cmd(kind: Option<&str>, input: &str) -> Option<String> {
    if !store::configured() {
        return None;
    }
    let dir = store::dir();
    match kind {
        Some("session-start") => Some(hook_session_start(&dir, &HookInput::parse(input))),
        Some("prompt") => {
            let out = hook_prompt(&dir, &HookInput::parse(input));
            (!out.is_empty()).then_some(out)
        }
        Some("antigravity") => Some(hook_antigravity(&dir, input)),
        _ => None,
    }
}

fn hook_session_start(dir: &Path, h: &HookInput) -> String {
    let mut s = store::Session::load(dir, &h.session_id);
    s.note(&h.cwd, &h.transcript_path);
    let here = place::from_dir(&s.cwd);
    let now = store::now();
    // Forgetting runs once per session; injected memories become fresh. Without the lock the
    // memory is still read, just not touched.
    let inj = match store::lock(dir, Duration::from_millis(800)) {
        Some(_lock) => {
            let mut m = store::load(dir);
            if s.injected.is_empty() {
                learn::begin_session(&mut m);
            }
            let inj = inject::session_start(&m, &s, &here, now);
            learn::co_recalled(&mut m, &inj.ids);
            let _ = store::save(&m, dir);
            inj
        }
        None => inject::session_start(&store::load(dir), &s, &here, now),
    };
    for id in inj.ids {
        if !s.injected.contains(&id) {
            s.injected.push(id);
        }
    }
    s.started = true;
    let _ = s.save(dir);
    store::prune_sessions(dir);
    inj.text
}

fn hook_prompt(dir: &Path, h: &HookInput) -> String {
    let mut s = store::Session::load(dir, &h.session_id);
    s.note(&h.cwd, &h.transcript_path);
    let m = store::load(dir);
    let inj = inject::prompt(&m, &s, &h.prompt, &place::from_dir(&s.cwd), store::now());
    s.injected.extend(inj.ids);
    let _ = s.save(dir);
    inj.text
}

/// Latest user message added to the Antigravity transcript since `hook_offset` (or from
/// `lastUserInput`/`prompt` in the hook JSON when no transcript file exists on disk). Only the
/// user's words call memories up, as in Claude Code's `UserPromptSubmit`; a call between tool
/// steps, with no new user message, recalls nothing.
fn antigravity_prompt(raw: &str, transcript_path: &str, hook_offset: &mut usize) -> String {
    if !transcript_path.is_empty() {
        if let Ok(bytes) = std::fs::read(transcript_path) {
            if bytes.len() < *hook_offset {
                *hook_offset = 0;
            }
            let fresh = String::from_utf8_lossy(&bytes[*hook_offset..]);
            let (steps, consumed) = cthg_core::antigravity::scan(&fresh);
            *hook_offset += consumed;
            for step in steps.into_iter().rev() {
                if let cthg_core::antigravity::Step::User { text, .. } = step {
                    return text;
                }
            }
            return String::new();
        }
    }
    let Some(v) = cthg_core::json::Json::parse(raw.trim()) else {
        return String::new();
    };
    v.get("lastUserInput")
        .or_else(|| v.get("prompt"))
        .and_then(cthg_core::json::Json::str)
        .map(cthg_core::antigravity::user_request)
        .unwrap_or_default()
}

/// Google Antigravity's `PreInvocation` hook: on the first turn of a session it injects what
/// `session-start` injects; on subsequent turns, only what `prompt` recalls.
fn hook_antigravity(dir: &Path, raw: &str) -> String {
    let p = cthg_core::antigravity::parse_payload(raw);
    let fallback = HookInput::parse(raw);
    let sid = if !p.conversation.is_empty() {
        p.conversation.as_str()
    } else if !fallback.session_id.is_empty() {
        fallback.session_id.as_str()
    } else {
        "antigravity"
    };
    let cwd = if !p.workspace.is_empty() { p.workspace.as_str() } else { fallback.cwd.as_str() };
    let transcript = if !p.transcript.is_empty() { p.transcript.as_str() } else { fallback.transcript_path.as_str() };

    let mut s = store::Session::load(dir, sid);
    s.note(cwd, transcript);
    let here = place::from_dir(&s.cwd);
    let now = store::now();

    if !s.started && s.injected.is_empty() {
        // Consume the initial transcript lines so the next call only reacts to later turns.
        let _ = antigravity_prompt(raw, &s.transcript, &mut s.hook_offset);
        let inj = match store::lock(dir, Duration::from_millis(800)) {
            Some(_lock) => {
                let mut m = store::load(dir);
                learn::begin_session(&mut m);
                let inj = inject::session_start(&m, &s, &here, now);
                learn::co_recalled(&mut m, &inj.ids);
                let _ = store::save(&m, dir);
                inj
            }
            None => inject::session_start(&store::load(dir), &s, &here, now),
        };
        for id in inj.ids {
            if !s.injected.contains(&id) {
                s.injected.push(id);
            }
        }
        s.started = true;
        let _ = s.save(dir);
        store::prune_sessions(dir);
        cthg_core::antigravity::inject(&inj.text)
    } else {
        let prompt_text = antigravity_prompt(raw, &s.transcript, &mut s.hook_offset);
        let m = store::load(dir);
        let inj = inject::prompt(&m, &s, &prompt_text, &here, now);
        s.injected.extend(inj.ids);
        let _ = s.save(dir);
        cthg_core::antigravity::inject(&inj.text)
    }
}

// ── for the writer subagent ──

fn cmd_session(dir: &Path, args: &[String]) -> Result<(), String> {
    let id = opt(args, "--id").ok_or("falta --id <sesión> (sale al iniciar la sesión, en la memoria inyectada)")?;
    let mut s = store::Session::load(dir, &id);
    if s.transcript.is_empty() {
        return Err(format!("no sé dónde está el transcript de la sesión {id}: los hooks lo anotan al iniciarla"));
    }
    let raw = std::fs::read_to_string(&s.transcript).map_err(|e| format!("{}: {e}", s.transcript))?;
    let from = if args.iter().any(|a| a == "--todo") { 0 } else { s.done_lines };
    let (dialogue, total) = match opt(args, "--formato").or_else(|| opt(args, "--format")) {
        Some(f) => {
            let fmt = transcript::Format::parse(&f).ok_or_else(|| format!("formato desconocido '{f}' (usa claude o antigravity)"))?;
            transcript::compact_with_format(&raw, from, &store::user_name(), &store::agent().unwrap_or_else(|| "Agente".into()), fmt)
        }
        None => transcript::compact(&raw, from, &store::user_name(), &store::agent().unwrap_or_else(|| "Agente".into())),
    };
    s.pending_lines = total;
    s.save(dir).map_err(|e| e.to_string())?;
    if dialogue.is_empty() {
        println!("(nada nuevo desde lo último guardado)");
        return Ok(());
    }
    println!("# Sesión {id} · carpeta: {}\n# Desde la línea {from} de {total} del transcript.\n\n{dialogue}", s.cwd);
    Ok(())
}

fn cmd_write(dir: &Path, args: &[String]) -> Result<(), String> {
    store::require_configured()?;
    let session = opt(args, "--sesion").map(|id| store::Session::load(dir, &id));
    let cwd = cwd_or(args, session.as_ref().map(|s| s.cwd.as_str()).unwrap_or(""));
    let json = match positional(args).filter(|p| p != "-") {
        Some(file) => std::fs::read_to_string(&file).map_err(|e| format!("{file}: {e}"))?,
        None => {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s).map_err(|e| e.to_string())?;
            s
        }
    };
    let (entries, skipped) = write::parse(&json, &cwd, &place::home())?;
    for s in &skipped {
        println!("omitida: {s}");
    }
    let now = store::now();
    let events = modify(dir, |m| Ok(write::apply(m, &entries, now)))?;
    for e in &events {
        println!("#{} {} · {} (χ {:+.2} → {:+.2})", e.node, e.action, text::clip(&e.label, 90), e.chi_before, e.chi_after);
    }
    if events.is_empty() && !entries.is_empty() {
        println!("nada nuevo: las entradas eran demasiado cortas o ya estaban");
    }
    // What the writer read is now saved: the next `cgm sesion` starts after it.
    if let Some(mut s) = session {
        s.done_lines = s.done_lines.max(s.pending_lines);
        s.save(dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ── for the agent and its user ──

fn cmd_recall(dir: &Path, args: &[String]) -> Result<(), String> {
    let q = positional(args).ok_or("¿qué busco? cgm recordar \"<texto>\"")?;
    let here = place::from_dir(&cwd_or(args, "").to_string_lossy());
    let m = store::load(dir);
    let scores = score::score_all(&m, &inject::view(store::now()));
    let query = Query { text: q, targets: vec![Target::new(&here, 0.5)] };
    let mut ids: Vec<u32> = retrieve::recall(&m, &scores, &query, 12, -1.0).into_iter().map(|r| r.id).collect();
    // CORE memories are left out of recall (they are always injected); a search still shows them.
    let qt = text::tokens(&query.text);
    for n in m.nodes.iter().filter(|n| n.has(flags::CORE) && scores[n.id as usize].visible) {
        if text::similarity(&qt, &text::tokens(&n.text)) > 0.15 {
            ids.insert(0, n.id);
        }
    }
    if ids.is_empty() {
        println!("(no recuerdo nada sobre eso)");
    }
    for id in ids {
        let n = &m.nodes[id as usize];
        let s = &scores[id as usize];
        println!("#{} [{}] {} · {}{}", n.id, certainty_tag(s.chi), n.text, place::describe(&n.path), if s.superseded { " (sustituido)" } else { "" });
    }
    Ok(())
}

fn cmd_list(dir: &Path, all: bool) -> Result<(), String> {
    let m = store::load(dir);
    let scores = score::score_all(&m, &inject::view(store::now()));
    for n in &m.nodes {
        let s = &scores[n.id as usize];
        if !all && (!s.visible || s.superseded) {
            continue;
        }
        let mut tags = Vec::new();
        if n.has(flags::CORE) {
            tags.push("núcleo");
        }
        if n.has(flags::POTENTIAL) {
            tags.push("plan");
        }
        if s.superseded {
            tags.push("sustituido");
        }
        println!(
            "#{:<4} χ {:+.2} {:<10} {:<8} {:<22} [{}] {}",
            n.id,
            s.chi,
            n.kind.name(),
            n.origin.name(),
            text::clip(&place::describe(&n.path), 22),
            tags.join(","),
            text::clip(&n.text, 110)
        );
    }
    Ok(())
}

fn cmd_agenda(dir: &Path) -> Result<(), String> {
    let m = store::load(dir);
    let plans: Vec<_> = m.nodes.iter().filter(|n| n.has(flags::POTENTIAL)).collect();
    if plans.is_empty() {
        println!("(agenda vacía)");
    }
    for n in plans {
        println!("#{} {} · {}", n.id, n.text, place::describe(&n.path));
    }
    Ok(())
}

fn cmd_done(dir: &Path, args: &[String]) -> Result<(), String> {
    store::require_configured()?;
    let id = id_arg(args)?;
    let now = store::now();
    modify(dir, |m| {
        let y = learn::next_y(m, now);
        let n = m.node_mut(id).ok_or(format!("no hay ningún #{id}"))?;
        if !n.has(flags::POTENTIAL) {
            return Err(format!("#{id} no es un plan pendiente"));
        }
        n.flags &= !flags::POTENTIAL;
        n.y = y;
        n.alpha += 1.0;
        n.activation = 1.0;
        println!("#{id} se cumple · {}", text::clip(&n.text, 90));
        Ok(())
    })
}

fn cmd_vote(dir: &Path, args: &[String], vote: i32) -> Result<(), String> {
    store::require_configured()?;
    let id = id_arg(args)?;
    modify(dir, |m| {
        if !learn::feedback(m, id, vote) {
            return Err(format!("#{id} no existe o está anclado"));
        }
        println!("#{id} {}", if vote > 0 { "confirmado" } else { "negado" });
        Ok(())
    })
}

fn cmd_core(dir: &Path, args: &[String]) -> Result<(), String> {
    store::require_configured()?;
    let id = id_arg(args)?;
    let unset = args.iter().any(|a| a == "--quitar");
    modify(dir, |m| {
        let n = m.node_mut(id).ok_or(format!("no hay ningún #{id}"))?;
        if unset {
            n.flags &= !flags::CORE;
        } else {
            n.flags |= flags::CORE;
        }
        println!("#{id} {} · {}", if unset { "fuera del núcleo" } else { "en el núcleo" }, text::clip(&n.text, 90));
        Ok(())
    })
}

fn cmd_where(args: &[String]) -> Result<(), String> {
    let dir = positional(args).map(PathBuf::from).unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let home = place::home();
    println!("carpeta:  {}", place::from_dir_in(&dir.to_string_lossy(), &home));
    println!("proyecto: {}", place::resolve("proyecto", &dir, &home));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cthg_core::json::Json;
    use std::fs;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cgm-main-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn names_come_as_flags_anywhere_so_any_shell_can_pass_them() {
        let args = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        let (rest, vars) = split_names(args("--agente Argon --usuario Luis hook antigravity"));
        assert_eq!(rest, args("hook antigravity"));
        assert_eq!(vars, vec![("CGM_AGENT", "Argon".to_string()), ("CGM_USER", "Luis".to_string())]);
        let (rest, vars) = split_names(args("anotar --sesion s1 --agente Argon f.json"));
        assert_eq!(rest, args("anotar --sesion s1 f.json"));
        assert_eq!(vars, vec![("CGM_AGENT", "Argon".to_string())]);
    }

    #[test]
    fn rejects_writes_and_silences_hooks_without_agent() {
        let _guard = store::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let d = tmp("no-agent");
        std::env::remove_var("CGM_DIR");
        std::env::remove_var("CGM_AGENT");
        std::env::set_var("CTHG_AI_REGISTRY", d.join("registro.json"));

        let json_file = d.join("entradas.json");
        fs::create_dir_all(&d).unwrap();
        fs::write(&json_file, r#"[{"texto": "Regla que no debe guardarse", "tipo": "regla", "donde": "yo"}]"#).unwrap();

        let err_write = cmd_write(&d, &[json_file.to_string_lossy().into_owned()]).unwrap_err();
        let err_done = cmd_done(&d, &["0".into()]).unwrap_err();
        let err_confirm = cmd_vote(&d, &["0".into()], 1).unwrap_err();
        let err_deny = cmd_vote(&d, &["0".into()], -1).unwrap_err();
        let err_core = cmd_core(&d, &["0".into()]).unwrap_err();

        for err in [&err_write, &err_done, &err_confirm, &err_deny, &err_core] {
            assert!(err.contains("CGM_AGENT"), "clear message about CGM_AGENT, got: {err}");
        }
        assert_eq!(run_hook_cmd(Some("session-start"), r#"{"session_id":"s1","cwd":"C:\\repo"}"#), None);
        assert_eq!(run_hook_cmd(Some("prompt"), r#"{"session_id":"s1","prompt":"hola mundo"}"#), None);
        assert_eq!(run_hook_cmd(Some("antigravity"), r#"{"conversationId":"s1","workspacePaths":["C:\\repo"]}"#), None);
        assert!(!store::file(&d).exists(), "nothing was written to disk");
        assert!(!d.join("registro.json").exists(), "registry was not touched");

        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn antigravity_hook_injects_session_start_then_prompt() {
        let _guard = store::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let d = tmp("agy-hook");
        let mem_dir = d.join("argon-mem");
        let reg_path = d.join("ai-memories.json");
        let transcript_path = d.join("transcript.jsonl");
        let workspace = d.join("workspace");
        fs::create_dir_all(&workspace).unwrap();

        std::env::set_var("CGM_DIR", &mem_dir);
        std::env::set_var("CGM_AGENT", "Argon");
        std::env::set_var("CGM_USER", "Luis");
        std::env::set_var("CTHG_AI_REGISTRY", &reg_path);

        // Seed Argon's memory with a CORE lesson and a craft memory.
        let seed = r#"[
            {"texto": "Código e identificadores en inglés; mensajes al usuario en español", "tipo": "regla", "donde": "yo", "origen": "usuario", "nucleo": true},
            {"texto": "En Minecraft 26.2 AvatarRenderState pinta a Steve: usa HumanoidRenderState", "tipo": "incidente", "donde": "oficio:minecraft-26-2", "origen": "usuario"}
        ]"#;
        let seed_file = d.join("seed.json");
        fs::write(&seed_file, seed).unwrap();
        cmd_write(&mem_dir, &[seed_file.to_string_lossy().into_owned()]).unwrap();

        // Turn 1: user starts the conversation.
        let line1 = r#"{"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","content":"<USER_REQUEST>\nHola Argon, revisemos la arquitectura.\n</USER_REQUEST>"}"#;
        fs::write(&transcript_path, format!("{line1}\n")).unwrap();

        let payload = format!(
            r#"{{"conversationId":"conv-agy-1","workspacePaths":["{}"],"transcriptPath":"{}","modelName":"auto","invocationNum":1}}"#,
            workspace.to_string_lossy().replace('\\', "\\\\"),
            transcript_path.to_string_lossy().replace('\\', "\\\\")
        );

        let out1 = hook_antigravity(&mem_dir, &payload);
        let v1 = Json::parse(&out1).expect("valid JSON from hook_antigravity");
        let msg1 = v1.get("injectSteps").unwrap().arr()[0].get("ephemeralMessage").unwrap().str().unwrap();
        assert!(msg1.contains("# Memoria global de Argon (solo tuya) · sesión conv-agy-1"), "got: {msg1}");
        assert!(msg1.contains("Código e identificadores en inglés"), "CORE lesson injected on first turn: {msg1}");
        assert!(!msg1.contains("HumanoidRenderState"), "unrelated craft memory not injected on start: {msg1}");

        // Second PreInvocation inside Turn 1 (after a tool call, no new USER_INPUT).
        let line2 = r#"{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","tool_calls":[{"name":"read_file","args":{"Path":"Cargo.toml"}}]}"#;
        fs::write(&transcript_path, format!("{line1}\n{line2}\n")).unwrap();
        assert_eq!(hook_antigravity(&mem_dir, &payload), "{}", "nothing injected mid-turn without a new user message");

        // Turn 2: user asks about Minecraft rendering.
        let line3 = r#"{"step_index":2,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","content":"<USER_REQUEST>\nel renderizado de Minecraft sale con la skin de Steve\n</USER_REQUEST>"}"#;
        fs::write(&transcript_path, format!("{line1}\n{line2}\n{line3}\n")).unwrap();
        let out2 = hook_antigravity(&mem_dir, &payload);
        let v2 = Json::parse(&out2).expect("valid JSON on turn 2");
        let msg2 = v2.get("injectSteps").unwrap().arr()[0].get("ephemeralMessage").unwrap().str().unwrap();
        assert!(msg2.contains("Recuerdos (memoria global de Argon):"), "got: {msg2}");
        assert!(msg2.contains("HumanoidRenderState"), "craft memory recalled on turn 2: {msg2}");

        // Turn 3: mentioning Minecraft again in the same session does not re-inject it.
        let line4 = r#"{"step_index":3,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","content":"<USER_REQUEST>\notra vez la skin de Steve en Minecraft\n</USER_REQUEST>"}"#;
        fs::write(&transcript_path, format!("{line1}\n{line2}\n{line3}\n{line4}\n")).unwrap();
        assert_eq!(hook_antigravity(&mem_dir, &payload), "{}", "never injected twice in the same session");

        std::env::remove_var("CGM_DIR");
        std::env::remove_var("CGM_AGENT");
        std::env::remove_var("CGM_USER");
        let _ = fs::remove_dir_all(&d);
    }
}
