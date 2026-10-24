//! Writing into the memory: what the writer subagent decided is worth keeping, as a JSON list.
//!
//! ```json
//! [{"texto": "…", "tipo": "regla", "donde": "yo", "origen": "usuario",
//!   "causa": "…", "sustituye": 12, "contradice": 4, "nucleo": true, "niega": false}]
//! ```
//!
//! - `tipo`: regla, decision, hecho (default), incidente, convencion, plan, comando, sesion.
//! - `donde`: yo · proyecto (default) · carpeta · persona:<Nombre> · oficio:<tema> · a folder.
//! - `origen`: usuario (the user said it) or agente (the agent concluded it, the default). What
//!   the agent concludes starts with no more evidence than a coin: only the user, or time, raises it.
//!
//! The engine decides the rest: a restated memory is reinforced instead of duplicated, a plan
//! that something shows done is fulfilled, a superseded one stops being injected.

use crate::place;
use cthg_core::json::Json;
use cthg_core::learn::{self, Claim, LearnEvent, Stance};
use cthg_core::model::{Memory, NodeKind, Origin};
use std::path::Path;

#[derive(Debug)]
pub struct Entry {
    pub claim: Claim,
}

fn field<'a>(o: &'a Json, keys: &[&str]) -> Option<&'a Json> {
    keys.iter().find_map(|k| o.get(k))
}

fn text_of(o: &Json, keys: &[&str]) -> Option<String> {
    field(o, keys).and_then(Json::str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn id_of(o: &Json, keys: &[&str]) -> Option<u32> {
    match field(o, keys)? {
        Json::Num(n) if *n >= 0.0 => Some(*n as u32),
        Json::Str(s) => s.trim().trim_start_matches('#').parse().ok(),
        _ => None,
    }
}

fn flag_of(o: &Json, keys: &[&str]) -> bool {
    matches!(field(o, keys), Some(Json::Bool(true)))
}

/// The entries of a JSON list (or a single object). Entries without text are skipped and
/// reported, never guessed.
pub fn parse(json: &str, cwd: &Path, home: &Path) -> Result<(Vec<Entry>, Vec<String>), String> {
    let v = Json::parse(json.trim_start_matches('\u{feff}').trim()).ok_or("el JSON no se puede leer (¿falta una coma o un corchete?)")?;
    let items: Vec<&Json> = match &v {
        Json::Arr(a) => a.iter().collect(),
        Json::Obj(_) => vec![&v],
        _ => return Err("se esperaba una lista de entradas".into()),
    };
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    for (i, o) in items.into_iter().enumerate() {
        let Some(text) = text_of(o, &["texto", "text"]) else {
            skipped.push(format!("entrada {}: sin «texto»", i + 1));
            continue;
        };
        let kind = text_of(o, &["tipo", "kind"]).and_then(|k| NodeKind::parse(&k.to_lowercase())).unwrap_or(NodeKind::Fact);
        let origin = match text_of(o, &["origen", "origin"]).as_deref() {
            Some("usuario") | Some("user") => Origin::User,
            _ => Origin::Agent,
        };
        let where_ = text_of(o, &["donde", "where"]).unwrap_or_else(|| "proyecto".to_string());
        let mut c = Claim::rule(&text, &place::resolve(&where_, cwd, home));
        c.kind = kind;
        c.origin = origin;
        c.cause = text_of(o, &["causa", "cause"]);
        c.supersedes = id_of(o, &["sustituye", "supersedes"]);
        c.contradicts = id_of(o, &["contradice", "contradicts"]);
        c.core = flag_of(o, &["nucleo", "núcleo", "core"]);
        if flag_of(o, &["niega", "deny"]) {
            c.stance = Stance::Denies;
        }
        out.push(Entry { claim: c });
    }
    Ok((out, skipped))
}

/// Learns every entry; what each one did to the memory.
pub fn apply(m: &mut Memory, entries: &[Entry], now: f64) -> Vec<LearnEvent> {
    let mut events = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        // Entries of one call keep their order on the time axis.
        events.extend(learn::learn(m, &e.claim, now + i as f64 * 1e-3));
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use cthg_core::model::flags;

    #[test]
    fn entries_say_what_where_who_and_why() {
        let home = Path::new(r"C:\Users\ana");
        let cwd = Path::new(r"C:\Users\ana\Lejos\NoExiste");
        let json = r##"[
          {"texto": "No preguntes el alcance: mira más lejos", "tipo": "regla", "donde": "yo", "origen": "usuario", "nucleo": true},
          {"texto": "Ana juega Arknights y habla español", "donde": "persona:Ana", "origen": "usuario"},
          {"texto": "En 26.2 AvatarRenderState pinta a Steve", "tipo": "incidente", "donde": "oficio:Minecraft 26.2", "causa": "el dispatcher lo manda al renderizador de jugador", "sustituye": "#4"},
          {"tipo": "regla"}
        ]"##;
        let (entries, skipped) = parse(json, cwd, home).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(skipped, vec!["entrada 4: sin «texto»"]);
        let (a, b, c) = (&entries[0].claim, &entries[1].claim, &entries[2].claim);
        assert_eq!((a.path.as_str(), a.kind, a.origin, a.core), ("", NodeKind::Rule, Origin::User, true));
        assert_eq!(b.path, "@personas/Ana");
        assert_eq!((b.kind, b.origin), (NodeKind::Fact, Origin::User));
        assert_eq!((c.path.as_str(), c.origin, c.supersedes), ("@oficio/minecraft-26-2", Origin::Agent, Some(4)));
        assert!(c.cause.as_deref().unwrap().contains("dispatcher"));
        assert!(parse("{texto: mal}", cwd, home).is_err());
    }

    #[test]
    fn saying_it_again_reinforces_instead_of_duplicating_and_plans_wait() {
        let home = Path::new(r"C:\Users\ana");
        let cwd = Path::new(r"C:\Users\ana\Projects\X");
        let mut m = Memory::default();
        let (e, _) = parse(r#"[{"texto": "Ana prefiere Rust para herramientas nuevas", "donde": "persona:Ana", "origen": "usuario"}]"#, cwd, home).unwrap();
        let first = apply(&mut m, &e, 100.0);
        let again = apply(&mut m, &e, 200.0);
        assert_eq!(m.nodes.len(), 1, "{first:?} {again:?}");
        assert_eq!(again[0].action, "refuerza");
        let (p, _) = parse(r#"[{"texto": "Revisar el nado del personaje en el juego", "tipo": "plan", "donde": "carpeta"}]"#, cwd, home).unwrap();
        let ev = apply(&mut m, &p, 300.0);
        assert!(m.nodes[ev[0].node as usize].has(flags::POTENTIAL), "a plan waits until something shows it done");
    }
}
