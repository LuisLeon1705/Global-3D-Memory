//! Reading the memory into a session, with no model and no cost but the lines themselves.
//!
//! - Session start: who the agent is (`CORE`), what applies where the session runs (spatial, λ^d
//!   over the folder tree), its own pending plans there, and how the last session there ended.
//! - Each message: only what the user's words call up and the session has not seen yet.
//!
//! Every line carries its `#id` (to confirm, deny or supersede it later) and its band of χ.

use crate::place;
use crate::store::{self, Session};
use cthg_core::model::{flags, Memory, Node, NodeKind};
use cthg_core::retrieve::{self, Query};
use cthg_core::score::{self, certainty_tag, Score, View};
use cthg_core::spatial::{self, Target};
use cthg_core::text;

/// Token budgets: small and fixed, so the memory never crowds the conversation.
pub const CORE_BUDGET: usize = 600;
pub const PLACE_BUDGET: usize = 450;
pub const AGENDA_MAX: usize = 5;
pub const PROMPT_BUDGET: usize = 300;

pub fn view(now: f64) -> View {
    View::on_branch(now, "")
}

fn line(n: &Node, s: &Score, with_place: bool) -> String {
    let body = text::clip(&n.text, 240);
    if with_place && !n.path.is_empty() {
        format!("#{} [{}] {} · {}", n.id, certainty_tag(s.chi), body, place::describe(&n.path))
    } else {
        format!("#{} [{}] {}", n.id, certainty_tag(s.chi), body)
    }
}

/// What fits the budget, leaving out whatever an active substitute replaced: recall and the core
/// still return those (a search shows them, marked), but injecting them would bring the old
/// version back next to the new one.
fn fit(m: &Memory, scores: &[Score], ids: &[u32], budget: usize, with_place: bool) -> Vec<u32> {
    let live: Vec<u32> = ids.iter().copied().filter(|&i| !scores[i as usize].superseded).collect();
    retrieve::fit_budget(m, &live, budget, |n| line(n, &scores[n.id as usize], with_place))
}

/// Paths that touch: one contains the other (a plan for the whole project shows inside any of
/// its folders, and a plan for a folder shows from the project root).
fn overlaps(a: &str, b: &str) -> bool {
    !a.is_empty() && (spatial::is_ancestor_or_self(a, b) || spatial::is_ancestor_or_self(b, a))
}

/// ` de <agent>` when the agent has a name.
fn whose() -> String {
    store::agent().map(|a| format!(" de {a}")).unwrap_or_default()
}

/// What was injected, so a session never gets the same memory twice.
pub struct Injection {
    pub text: String,
    pub ids: Vec<u32>,
}

pub fn session_start(m: &Memory, session: &Session, here: &str, now: f64) -> Injection {
    let scores = score::score_all(m, &view(now));
    let mut out = vec![format!(
        "# Memoria global{} (solo tuya) · sesión {}\nPara guardar lo que valga la pena: skill `memoria-global` (la escribe un subagente). Para buscar: `cgm recordar \"…\"`. Cada línea lleva su #id y su banda de certeza.",
        whose(),
        session.id
    )];
    let mut ids = Vec::new();

    let core = fit(m, &scores, &retrieve::core(m, &scores), CORE_BUDGET, true);
    if !core.is_empty() {
        out.push("\n## Quién eres y lo que no olvidas".to_string());
        out.extend(core.iter().map(|&i| line(&m.nodes[i as usize], &scores[i as usize], true)));
        ids.extend(&core);
    }

    if !here.is_empty() {
        let q = Query { text: String::new(), targets: vec![Target::new(here, 1.0)] };
        let found: Vec<u32> = retrieve::recall(m, &scores, &q, 16, -0.25)
            .into_iter()
            .map(|r| r.id)
            .filter(|i| m.nodes[*i as usize].kind != NodeKind::Session)
            .collect();
        let place_ids = fit(m, &scores, &found, PLACE_BUDGET, true);
        if !place_ids.is_empty() {
            out.push(format!("\n## Aquí ({})", place::describe(here)));
            out.extend(place_ids.iter().map(|&i| line(&m.nodes[i as usize], &scores[i as usize], true)));
            ids.extend(&place_ids);
        }

        // His own plans: hidden from recall until done, but they are what he meant to do here.
        let mut plans: Vec<&Node> = m.nodes.iter().filter(|n| n.has(flags::POTENTIAL) && overlaps(&n.path, here)).collect();
        plans.sort_by(|a, b| b.y.partial_cmp(&a.y).unwrap_or(std::cmp::Ordering::Equal));
        if !plans.is_empty() {
            out.push("\n## Lo que querías hacer aquí".to_string());
            out.extend(plans.iter().take(AGENDA_MAX).map(|n| format!("#{} {}", n.id, text::clip(&n.text, 200))));
        }

        let last = m
            .nodes
            .iter()
            .filter(|n| {
                let s = &scores[n.id as usize];
                n.kind == NodeKind::Session && s.visible && !s.superseded && overlaps(&n.path, here)
            })
            .max_by(|a, b| a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal));
        if let Some(n) = last {
            out.push(format!("\n## La última vez aquí\n#{} {}", n.id, text::clip(&n.text, 300)));
            ids.push(n.id);
        }
    }
    Injection { text: out.join("\n"), ids }
}

/// The memories a message calls up that this session has not seen; nothing if none.
pub fn prompt(m: &Memory, session: &Session, prompt: &str, here: &str, now: f64) -> Injection {
    if text::tokens(prompt).len() < 2 {
        return Injection { text: String::new(), ids: vec![] };
    }
    let scores = score::score_all(m, &view(now));
    let targets = if here.is_empty() { vec![] } else { vec![Target::new(here, 0.5)] };
    let q = Query { text: prompt.to_string(), targets };
    let fresh: Vec<u32> = retrieve::recall(m, &scores, &q, 8, -0.25)
        .into_iter()
        .map(|r| r.id)
        .filter(|i| !session.injected.contains(i))
        .collect();
    let ids = fit(m, &scores, &fresh, PROMPT_BUDGET, true);
    if ids.is_empty() {
        return Injection { text: String::new(), ids };
    }
    let mut out = vec![format!("Recuerdos (memoria global{}):", whose())];
    out.extend(ids.iter().map(|&i| line(&m.nodes[i as usize], &scores[i as usize], true)));
    Injection { text: out.join("\n"), ids }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cthg_core::learn::{self, Claim};
    use cthg_core::model::Origin;

    fn learn_at(m: &mut Memory, text: &str, path: &str, kind: NodeKind, core: bool) -> u32 {
        let mut c = Claim::rule(text, path);
        c.kind = kind;
        c.core = core;
        c.origin = Origin::User;
        learn::learn(m, &c, 1000.0 + m.nodes.len() as f64)[0].node
    }

    #[test]
    fn a_session_gets_who_it_is_what_applies_there_and_what_it_meant_to_do() {
        let mut m = Memory::default();
        let me = learn_at(&mut m, "No preguntes el alcance: mira hacia un futuro más grande que uno mismo", "", NodeKind::Rule, true);
        let games = learn_at(&mut m, "Los proyectos de Juegos guardan la memoria en módulos cthm por equipo", "Projects/Juegos", NodeKind::Fact, false);
        let other = learn_at(&mut m, "Finanzas usa una API de bolsa de pago", "Projects/Finanzas", NodeKind::Fact, false);
        let plan = learn_at(&mut m, "Probar el nado del personaje dentro del juego", "Projects/Juegos/Rol", NodeKind::Plan, false);
        let s = Session { id: "s1".into(), ..Default::default() };
        let inj = session_start(&m, &s, "Projects/Juegos/Rol/Cliente", 1e9);
        assert!(inj.text.contains(&format!("#{me} ")), "{}", inj.text);
        assert!(inj.text.contains(&format!("#{games} ")), "a folder above reaches the session: {}", inj.text);
        assert!(!inj.text.contains(&format!("#{other} ")), "a sibling project does not");
        assert!(inj.text.contains(&format!("#{plan} Probar el nado")), "its own plan there: {}", inj.text);
        assert!(inj.ids.contains(&me) && inj.ids.contains(&games));
    }

    #[test]
    fn a_message_brings_only_what_its_words_call_and_never_twice() {
        let mut m = Memory::default();
        let mc = learn_at(&mut m, "En Minecraft 26.2 AvatarRenderState pinta a Steve: usa HumanoidRenderState", "@oficio/minecraft-26-2", NodeKind::Incident, false);
        learn_at(&mut m, "Ana prefiere Rust para herramientas nuevas", "@personas/Ana", NodeKind::Fact, false);
        let mut s = Session { id: "s2".into(), ..Default::default() };
        let first = prompt(&m, &s, "el renderizado de Minecraft sale con la skin de Steve", "", 1e9);
        assert!(first.ids.contains(&mc), "{}", first.text);
        assert!(!first.text.contains("Rust"), "unrelated memories stay out: {}", first.text);
        s.injected.extend(&first.ids);
        assert!(prompt(&m, &s, "otra vez la skin de Steve en Minecraft", "", 1e9).ids.is_empty(), "never twice in a session");
        assert!(prompt(&m, &s, "ok", "", 1e9).text.is_empty(), "a word alone calls nothing");
    }

    #[test]
    fn a_replaced_memory_is_never_injected_next_to_its_substitute() {
        let mut m = Memory::default();
        let old = learn_at(&mut m, "El archivo memoria.cthg se guarda junto al código del repositorio", "Projects/Memoria", NodeKind::Fact, false);
        let new = learn_at(&mut m, "Cada agente guarda memoria.cthg bajo ~/.cthg/agentes con su nombre", "Projects/Memoria", NodeKind::Fact, false);
        learn::supersede(&mut m, new, old).unwrap();
        let s = Session { id: "s3".into(), ..Default::default() };
        let inj = prompt(&m, &s, "dónde se guarda memoria.cthg", "Projects/Memoria", 1e9);
        assert!(inj.ids.contains(&new), "{}", inj.text);
        assert!(!inj.ids.contains(&old), "the old version stays out: {}", inj.text);
        let start = session_start(&m, &s, "Projects/Memoria", 1e9);
        assert!(!start.ids.contains(&old), "at session start too: {}", start.text);
    }

    #[test]
    fn a_replaced_session_summary_is_not_the_last_time_anywhere() {
        // A summary anchored too high ("Projects") was replaced by one in its project: in a
        // sibling folder the old one must not come back as «la última vez aquí».
        let mut m = Memory::default();
        let old = learn_at(&mut m, "Estado del proyecto AE: fases cero a cuatro hechas", "Projects", NodeKind::Session, false);
        let new = learn_at(&mut m, "Estado de AE: fases cero a cuatro hechas y elipsis", "Projects/AE", NodeKind::Session, false);
        learn::supersede(&mut m, new, old).unwrap();
        let s = Session { id: "s4".into(), ..Default::default() };
        let elsewhere = session_start(&m, &s, "Projects/Otro", 1e9);
        assert!(!elsewhere.ids.contains(&old), "the replaced summary leaks: {}", elsewhere.text);
        let here = session_start(&m, &s, "Projects/AE", 1e9);
        assert!(here.ids.contains(&new), "{}", here.text);
        assert!(!here.ids.contains(&old), "{}", here.text);
    }
}
