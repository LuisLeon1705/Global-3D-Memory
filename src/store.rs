//! Where the agent's global memory lives and how it is opened and saved.
//!
//! One file, `~/.cthg/agentes/<slug(CGM_AGENT)>/memoria.cthg` (or in `CGM_DIR`), in the same
//! binary format as every CTHG memory. It belongs to one agent only: another agent keeps its own,
//! and a memory shared between agents would be a separate one. Without `CGM_AGENT` (and without
//! `CGM_DIR`), nothing is written or injected. Writers take `lock` for the whole load-modify-save,
//! so a hook and the writer subagent never lose each other's work.
//! Per session: which memories were already injected and where its transcript is.
//!
//! The memory registers itself in `~/.cthg/ai-memories.json` (or `CTHG_AI_REGISTRY`), the list
//! of AI memories that viewers such as `memviz` show in their «AI Memory» section.

use cthg_core::binary;
use cthg_core::json::Json;
use cthg_core::model::Memory;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// A lock older than this was left by a crashed writer.
const STALE_LOCK: Duration = Duration::from_secs(30);

/// Whether an explicit memory folder (`CGM_DIR`) or an agent name (`CGM_AGENT`) is configured.
pub fn configured() -> bool {
    std::env::var_os("CGM_DIR").is_some_and(|d| !d.is_empty())
        || agent().is_some_and(|a| !crate::place::slug(&a).is_empty())
}

/// Rejects write operations when neither `CGM_AGENT` nor `CGM_DIR` is set.
pub fn require_configured() -> Result<(), String> {
    if configured() {
        Ok(())
    } else {
        Err("falta CGM_AGENT: define CGM_AGENT=<nombre del agente> (o CGM_DIR=<carpeta>) para elegir qué memoria usar".into())
    }
}

/// Default folder for an agent under a given home directory: `<home>/.cthg/agentes/<slug(agent)>`.
pub fn agent_dir(home: &Path, agent_name: &str) -> PathBuf {
    home.join(".cthg").join("agentes").join(crate::place::slug(agent_name))
}

/// Legacy `<repo>/memoria` folder from single-agent versions.
pub fn legacy_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("memoria")
}

fn move_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    if let Err(e) = fs::rename(src, dst) {
        fs::copy(src, dst).map_err(|_| e)?;
        let _ = fs::remove_file(src);
    }
    Ok(())
}

/// One-time migration for Claude: moves `<old_dir>/memoria.cthg` (or `claude.cthg`), its `.bak`
/// and its `sesion/` into `new_dir` when `new_dir/memoria.cthg` does not exist yet.
pub fn migrate_legacy(old_dir: &Path, new_dir: &Path) {
    let target = new_dir.join("memoria.cthg");
    if target.exists() {
        return;
    }
    let old_file = if old_dir.join("memoria.cthg").exists() {
        old_dir.join("memoria.cthg")
    } else if old_dir.join("claude.cthg").exists() {
        old_dir.join("claude.cthg")
    } else {
        return;
    };
    if fs::create_dir_all(new_dir).is_err() {
        return;
    }
    if move_file(&old_file, &target).is_err() {
        return;
    }
    for bak_name in ["memoria.cthg.bak", "claude.cthg.bak", "memoria.bak", "claude.bak"] {
        let old_bak = old_dir.join(bak_name);
        let new_bak = new_dir.join("memoria.cthg.bak");
        if old_bak.exists() && !new_bak.exists() {
            let _ = move_file(&old_bak, &new_bak);
        }
    }
    let old_ses = old_dir.join("sesion");
    if old_ses.is_dir() {
        let new_ses = new_dir.join("sesion");
        if fs::create_dir_all(&new_ses).is_ok() {
            if let Ok(rd) = fs::read_dir(&old_ses) {
                for e in rd.flatten() {
                    let dst = new_ses.join(e.file_name());
                    if !dst.exists() {
                        let _ = move_file(&e.path(), &dst);
                    }
                }
            }
            let _ = fs::remove_dir(&old_ses);
        }
    }
    if let Some(reg) = registry() {
        register_in(new_dir, &reg, "Claude");
    }
}

pub fn dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CGM_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    if let Some(a) = agent() {
        let slug = crate::place::slug(&a);
        if !slug.is_empty() {
            let d = agent_dir(&crate::place::home(), &a);
            if slug == "claude" {
                migrate_legacy(&legacy_dir(), &d);
            }
            return d;
        }
    }
    crate::place::home().join(".cthg").join("agentes").join("sin-agente")
}

pub fn file(dir: &Path) -> PathBuf {
    let f = dir.join("memoria.cthg");
    // Early versions named the file after its agent.
    let old = dir.join("claude.cthg");
    if !f.exists() && old.exists() {
        let _ = fs::rename(&old, &f);
    }
    f
}

/// The agent's name (`CGM_AGENT`), if it was given one.
pub fn agent() -> Option<String> {
    std::env::var("CGM_AGENT").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// How the user is called in the transcripts the writer reads (`CGM_USER`).
pub fn user_name() -> String {
    std::env::var("CGM_USER").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "Usuario".into())
}

pub fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// The memory, or a new empty one the first time.
pub fn load(dir: &Path) -> Memory {
    let f = file(dir);
    if f.exists() {
        if let Ok(m) = binary::load(&f) {
            return m;
        }
    }
    let mut m = Memory::default();
    m.set_meta("created", &format!("{}", now() as u64));
    m.set_meta("owner", &agent().unwrap_or_else(|| "agente".into()).to_lowercase());
    m
}

pub fn save(m: &Memory, dir: &Path) -> std::io::Result<()> {
    binary::save(m, &file(dir))?;
    register(dir);
    Ok(())
}

// ── the registry of AI memories ──

fn registry() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CTHG_AI_REGISTRY") {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(PathBuf::from(home).join(".cthg").join("ai-memories.json"))
}

fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Adds this memory to the registry once (or updates its path if its previous file no longer
/// exists), keeping every other entry intact. Never fails out loud.
pub fn register(dir: &Path) {
    let Some(reg) = registry() else { return };
    let my_name = agent().unwrap_or_else(|| "Agente".into());
    register_in(dir, &reg, &my_name);
}

pub fn register_in(dir: &Path, reg: &Path, my_name: &str) {
    let me = file(dir);
    if !me.exists() {
        return;
    }
    let me_s = me.display().to_string();
    let raw = fs::read_to_string(reg).unwrap_or_default();
    let mut entries: Vec<(String, String)> = Vec::new();
    if let Some(Json::Arr(list)) = Json::parse(raw.trim()) {
        for e in &list {
            if let Some(f) = e.get("file").and_then(Json::str) {
                entries.push((e.get("name").and_then(Json::str).unwrap_or("").to_string(), f.to_string()));
            }
        }
    }
    let mut updated = false;
    for (n, f) in &mut entries {
        if n.eq_ignore_ascii_case(my_name) && !Path::new(f).exists() {
            *n = my_name.to_string();
            *f = me_s.clone();
            updated = true;
            break;
        }
    }
    if !updated {
        if entries.iter().any(|(_, f)| Path::new(f) == me) {
            return;
        }
        entries.push((my_name.to_string(), me_s));
    } else {
        let mut seen_me = false;
        entries.retain(|(_, f)| {
            if Path::new(f) == me {
                if seen_me {
                    return false;
                }
                seen_me = true;
            }
            true
        });
    }
    let body: Vec<String> = entries.iter().map(|(n, f)| format!("  {{ \"name\": {}, \"file\": {} }}", quote(n), quote(f))).collect();
    let _ = fs::create_dir_all(reg.parent().unwrap_or(Path::new(".")));
    let _ = fs::write(reg, format!("[\n{}\n]\n", body.join(",\n")));
}

/// Held for one load-modify-save; dropping it releases `lock`.
pub struct Lock(PathBuf);

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub fn lock(dir: &Path, wait: Duration) -> Option<Lock> {
    let _ = fs::create_dir_all(dir);
    let path = dir.join("lock");
    let start = Instant::now();
    loop {
        match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => return Some(Lock(path)),
            Err(_) => {
                let stale = fs::metadata(&path).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > STALE_LOCK);
                if stale {
                    let _ = fs::remove_file(&path);
                    continue;
                }
                if start.elapsed() > wait {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(40));
            }
        }
    }
}

// ── per session ──

fn session_dir(dir: &Path) -> PathBuf {
    dir.join("sesion")
}

fn safe(id: &str) -> String {
    id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(80).collect()
}

/// What a session already received, where it runs and where its transcript is.
#[derive(Debug, Default, PartialEq)]
pub struct Session {
    pub id: String,
    pub cwd: String,
    pub transcript: String,
    pub injected: Vec<u32>,
    /// Transcript lines already turned into memories (by the writer subagent).
    pub done_lines: usize,
    /// Transcript lines handed to the writer by the last `cgm sesion`, saved once it writes.
    pub pending_lines: usize,
    /// Whether the session-start context has already been injected (for single-hook harnesses).
    pub started: bool,
    /// Bytes of the transcript already scanned by `cgm hook antigravity`.
    pub hook_offset: usize,
}

impl Session {
    pub fn load(dir: &Path, id: &str) -> Session {
        let id = safe(id);
        let mut s = Session { id: id.clone(), ..Default::default() };
        let Ok(text) = fs::read_to_string(session_dir(dir).join(format!("{id}.txt"))) else { return s };
        for line in text.lines() {
            let (k, v) = line.split_once('=').unwrap_or((line, ""));
            match k {
                "cwd" => s.cwd = v.to_string(),
                "transcript" => s.transcript = v.to_string(),
                "injected" => s.injected = v.split(',').filter_map(|x| x.parse().ok()).collect(),
                "done_lines" => s.done_lines = v.parse().unwrap_or(0),
                "pending_lines" => s.pending_lines = v.parse().unwrap_or(0),
                "started" => s.started = v == "true" || v == "1",
                "hook_offset" => s.hook_offset = v.parse().unwrap_or(0),
                _ => {}
            }
        }
        s
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        let d = session_dir(dir);
        fs::create_dir_all(&d)?;
        let injected: Vec<String> = self.injected.iter().map(|i| i.to_string()).collect();
        let text = format!(
            "cwd={}\ntranscript={}\ninjected={}\ndone_lines={}\npending_lines={}\nstarted={}\nhook_offset={}\n",
            self.cwd,
            self.transcript,
            injected.join(","),
            self.done_lines,
            self.pending_lines,
            self.started,
            self.hook_offset
        );
        fs::write(d.join(format!("{}.txt", self.id)), text)
    }

    /// Notes where the session runs and where its transcript is (hooks know; the agent does not).
    pub fn note(&mut self, cwd: &str, transcript: &str) {
        if !cwd.is_empty() {
            self.cwd = cwd.to_string();
        }
        if !transcript.is_empty() {
            self.transcript = transcript.to_string();
        }
    }
}

/// Sessions untouched for a week are forgotten (their files only; the memory stays).
pub fn prune_sessions(dir: &Path) {
    let Ok(rd) = fs::read_dir(session_dir(dir)) else { return };
    for e in rd.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > Duration::from_secs(7 * 86400));
        if old {
            let _ = fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cgm-store-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_new_memory_has_an_owner_and_survives_a_save() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let d = tmp("new");
        std::env::set_var("CTHG_AI_REGISTRY", d.join("registro.json"));
        let mut m = load(&d);
        assert!(m.meta("owner").is_some());
        m.set_meta("probe", "1");
        save(&m, &d).unwrap();
        assert_eq!(load(&d).meta("probe"), Some("1"));
        save(&m, &d).unwrap();
        let reg = fs::read_to_string(d.join("registro.json")).unwrap();
        assert_eq!(reg.matches("memoria.cthg").count(), 1, "registered once: {reg}");
        assert!(Json::parse(reg.trim()).is_some(), "the registry stays valid JSON: {reg}");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn one_writer_at_a_time() {
        let d = tmp("lock");
        let first = lock(&d, Duration::from_millis(10)).unwrap();
        assert!(lock(&d, Duration::from_millis(60)).is_none(), "a second writer waits and gives up");
        drop(first);
        assert!(lock(&d, Duration::from_millis(10)).is_some());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_session_remembers_what_it_got_and_where_its_transcript_is() {
        let d = tmp("session");
        let mut s = Session::load(&d, "abc-123");
        s.note(r"C:\Users\x\Projects\App", r"C:\Users\x\.claude\projects\t.jsonl");
        s.injected = vec![3, 7];
        s.done_lines = 42;
        s.pending_lines = 50;
        s.save(&d).unwrap();
        let back = Session::load(&d, "abc-123");
        assert_eq!(back, s);
        assert_eq!(Session::load(&d, "../../etc").id, "etc", "ids never escape the folder");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn default_dir_is_per_agent_and_cgm_dir_overrides() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let d = tmp("per-agent");
        std::env::remove_var("CGM_DIR");
        std::env::remove_var("CGM_AGENT");
        assert!(!configured(), "unconfigured without CGM_AGENT and without CGM_DIR");
        assert!(require_configured().is_err());

        std::env::set_var("CGM_AGENT", "Argon");
        assert!(configured());
        let argon = dir();
        assert!(argon.ends_with(Path::new(".cthg").join("agentes").join("argon")), "got {}", argon.display());
        assert!(file(&argon).ends_with(Path::new(".cthg").join("agentes").join("argon").join("memoria.cthg")));

        let home = d.join("home");
        let claude = agent_dir(&home, "Claude");
        let argon_in_home = agent_dir(&home, "Argon");
        assert_ne!(claude, argon_in_home, "two agents never share the same default folder");

        std::env::set_var("CGM_DIR", &d);
        assert_eq!(dir(), d, "CGM_DIR overrides the per-agent default");
        std::env::remove_var("CGM_DIR");
        std::env::remove_var("CGM_AGENT");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn migrates_legacy_claude_memory_bak_and_sessions_in_temp_dirs() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = tmp("migrate");
        let old_dir = root.join("legacy-memoria");
        let new_dir = root.join("home").join(".cthg").join("agentes").join("claude");
        let reg_path = root.join("ai-memories.json");
        std::env::set_var("CTHG_AI_REGISTRY", &reg_path);

        fs::create_dir_all(old_dir.join("sesion")).unwrap();
        let mut m = Memory::default();
        m.set_meta("owner", "claude");
        m.set_meta("legacy_probe", "si");
        binary::save(&m, &old_dir.join("memoria.cthg")).unwrap();
        fs::write(old_dir.join("memoria.cthg.bak"), b"backup-bytes").unwrap();
        fs::write(old_dir.join("sesion").join("s1.txt"), "cwd=C:\\repo\ndone_lines=12\n").unwrap();
        register_in(&old_dir, &reg_path, "Claude");

        migrate_legacy(&old_dir, &new_dir);

        assert!(!old_dir.join("memoria.cthg").exists(), "old memory file was moved");
        assert!(!old_dir.join("memoria.cthg.bak").exists(), "old backup was moved");
        assert!(!old_dir.join("sesion").join("s1.txt").exists(), "old session was moved");
        assert!(new_dir.join("memoria.cthg").exists(), "new memory file exists");
        assert_eq!(load(&new_dir).meta("legacy_probe"), Some("si"));
        assert_eq!(fs::read(new_dir.join("memoria.cthg.bak")).unwrap(), b"backup-bytes");
        assert_eq!(Session::load(&new_dir, "s1").done_lines, 12);

        let reg = fs::read_to_string(&reg_path).unwrap();
        assert_eq!(reg.matches("Claude").count(), 1, "registry updated without duplicating: {reg}");
        assert!(reg.contains(&new_dir.join("memoria.cthg").display().to_string().replace('\\', "\\\\")), "registry points to new location: {reg}");

        // Also test migrating the older `claude.cthg` name when target does not exist yet.
        let old2 = root.join("legacy2");
        let new2 = root.join("target2");
        fs::create_dir_all(&old2).unwrap();
        binary::save(&m, &old2.join("claude.cthg")).unwrap();
        migrate_legacy(&old2, &new2);
        assert!(new2.join("memoria.cthg").exists());
        assert!(!old2.join("claude.cthg").exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn registry_updates_missing_file_for_same_agent_without_duplicating() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = tmp("registry");
        let reg_path = root.join("ai-memories.json");
        std::env::set_var("CTHG_AI_REGISTRY", &reg_path);

        let other_dir = root.join("otro");
        let old_claude = root.join("viejo-claude");
        let new_claude = root.join("nuevo-claude");
        let argon_dir = root.join("argon");

        let m = Memory::default();
        binary::save(&m, &file(&other_dir)).unwrap();
        binary::save(&m, &file(&old_claude)).unwrap();
        register_in(&other_dir, &reg_path, "Otro");
        register_in(&old_claude, &reg_path, "Claude");

        // Now remove the old Claude file and save at the new location.
        fs::remove_dir_all(&old_claude).unwrap();
        binary::save(&m, &file(&new_claude)).unwrap();
        register_in(&new_claude, &reg_path, "Claude");

        // Also register Argon.
        binary::save(&m, &file(&argon_dir)).unwrap();
        register_in(&argon_dir, &reg_path, "Argon");

        let raw = fs::read_to_string(&reg_path).unwrap();
        let Some(Json::Arr(list)) = Json::parse(raw.trim()) else {
            panic!("invalid JSON in registry: {raw}");
        };
        assert_eq!(list.len(), 3, "expected Otro, Claude (updated), and Argon: {raw}");
        assert_eq!(list[0].get("name").and_then(Json::str), Some("Otro"));
        assert_eq!(list[0].get("file").and_then(Json::str), Some(file(&other_dir).display().to_string().as_str()));
        assert_eq!(list[1].get("name").and_then(Json::str), Some("Claude"));
        assert_eq!(list[1].get("file").and_then(Json::str), Some(file(&new_claude).display().to_string().as_str()));
        assert_eq!(list[2].get("name").and_then(Json::str), Some("Argon"));
        assert_eq!(list[2].get("file").and_then(Json::str), Some(file(&argon_dir).display().to_string().as_str()));

        let _ = fs::remove_dir_all(&root);
    }
}
