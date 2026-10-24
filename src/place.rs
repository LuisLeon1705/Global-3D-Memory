//! X axis of the agent's global memory: where a memory applies.
//!
//! - `""` — the agent itself: who it is with its user, the lessons it keeps. Reaches every
//!   session through `CORE`, never by path (the root has spatial affinity 0).
//! - `@personas/<name>`, `@oficio/<topic>` — people and craft: found by their words, not by
//!   where the session runs.
//! - `<folder relative to the user's home>` — a project or a folder of projects. A memory there
//!   applies to everything under it, fading with depth (λ^d): what is learned in
//!   `…/Projects/Juegos` reaches every game project below it, not `…/Projects/Finanzas`.

use cthg_core::spatial;
use std::path::{Path, PathBuf};

pub const PEOPLE: &str = "@personas";
pub const CRAFT: &str = "@oficio";

/// The user's home folder (`USERPROFILE`, else `HOME`).
pub fn home() -> PathBuf {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default()
}

/// A folder as a point on X: relative to the home folder when inside it (case-insensitive, as
/// Windows is), else the whole normalized path.
pub fn from_dir_in(dir: &str, home: &Path) -> String {
    let d = spatial::normalize(dir);
    let h = spatial::normalize(&home.to_string_lossy());
    if h.is_empty() {
        return d;
    }
    let (dl, hl) = (d.to_lowercase(), h.to_lowercase());
    if dl == hl {
        return String::new();
    }
    if dl.starts_with(&hl) && d.as_bytes().get(h.len()) == Some(&b'/') {
        return d[h.len() + 1..].to_string();
    }
    d
}

pub fn from_dir(dir: &str) -> String {
    from_dir_in(dir, &home())
}

/// The project a folder belongs to: the nearest ancestor with `.git`, `Cargo.toml` or
/// `package.json`, below the home folder (home itself is a git repository and is not a
/// project). Without one, the folder itself.
pub fn project_root(dir: &Path, home: &Path) -> PathBuf {
    let markers = [".git", "Cargo.toml", "package.json", "pyproject.toml"];
    let mut best: Option<PathBuf> = None;
    let mut cur = Some(dir);
    while let Some(d) = cur {
        if same(d, home) {
            break;
        }
        if markers.iter().any(|m| d.join(m).exists()) {
            // The outermost marked folder below home wins: a crate inside a workspace belongs to
            // the workspace, a `src-tauri` to its app.
            best = Some(d.to_path_buf());
        }
        cur = d.parent();
    }
    best.unwrap_or_else(|| dir.to_path_buf())
}

fn same(a: &Path, b: &Path) -> bool {
    spatial::normalize(&a.to_string_lossy()).eq_ignore_ascii_case(&spatial::normalize(&b.to_string_lossy()))
}

/// Lowercase words joined by `-`: «Minecraft 26.2» → `minecraft-26-2`.
pub fn slug(s: &str) -> String {
    let folded: String = s.trim().to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
    folded.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-")
}

/// Where an entry says it applies, as a point on X:
/// - `yo` (or empty): the agent itself;
/// - `proyecto`: the project of `cwd`; `carpeta`: `cwd` itself;
/// - `persona:<Name>` and `oficio:<topic>`;
/// - anything else: a folder (absolute, or relative to home).
pub fn resolve(where_: &str, cwd: &Path, home: &Path) -> String {
    let w = where_.trim();
    let lower = w.to_lowercase();
    match lower.as_str() {
        "" | "yo" | "agente" | "self" => String::new(),
        "proyecto" | "project" => from_dir_in(&project_root(cwd, home).to_string_lossy(), home),
        "carpeta" | "aqui" | "aquí" => from_dir_in(&cwd.to_string_lossy(), home),
        _ => {
            if let Some(name) = lower.strip_prefix("persona:").map(|_| &w["persona:".len()..]) {
                format!("{PEOPLE}/{}", name.trim())
            } else if let Some(topic) = lower.strip_prefix("oficio:") {
                format!("{CRAFT}/{}", slug(topic))
            } else if w.starts_with('@') {
                spatial::normalize(w)
            } else {
                let p = Path::new(w);
                if p.is_absolute() {
                    from_dir_in(w, home)
                } else {
                    spatial::normalize(w)
                }
            }
        }
    }
}

/// How a point on X reads in the injected context.
pub fn describe(path: &str) -> String {
    if path.is_empty() {
        "yo".to_string()
    } else if let Some(p) = path.strip_prefix(&format!("{PEOPLE}/")) {
        format!("persona: {p}")
    } else if let Some(t) = path.strip_prefix(&format!("{CRAFT}/")) {
        format!("oficio: {t}")
    } else {
        // The last two folders say which project it is.
        let parts: Vec<&str> = path.split('/').collect();
        parts[parts.len().saturating_sub(2)..].join("/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_in_home_is_relative_to_it_whatever_the_case_and_slashes() {
        let home = Path::new(r"C:\Users\ana");
        assert_eq!(from_dir_in(r"C:\Users\ana\OneDrive\Escritorio\Projects\Juegos\Rol", home), "OneDrive/Escritorio/Projects/Juegos/Rol");
        assert_eq!(from_dir_in("c:/users/ANA/Projects", home), "Projects");
        assert_eq!(from_dir_in(r"C:\Users\ana", home), "");
        assert_eq!(from_dir_in(r"D:\juegos\mc", home), "D:/juegos/mc");
        assert_eq!(from_dir_in(r"C:\Users\anax\otra", home), "C:/Users/anax/otra", "a sibling with a longer name is not inside home");
    }

    #[test]
    fn where_an_entry_applies() {
        let home = std::env::temp_dir().join(format!("cgm-place-{}", std::process::id()));
        let app = home.join("Projects").join("App");
        std::fs::create_dir_all(app.join("src-tauri").join("src")).unwrap();
        std::fs::write(app.join("Cargo.toml"), "").unwrap();
        std::fs::write(app.join("src-tauri").join("Cargo.toml"), "").unwrap();
        let cwd = app.join("src-tauri").join("src");
        assert_eq!(resolve("yo", &cwd, &home), "");
        assert_eq!(resolve("proyecto", &cwd, &home), "Projects/App", "the outermost project below home");
        assert_eq!(resolve("carpeta", &cwd, &home), "Projects/App/src-tauri/src");
        assert_eq!(resolve("persona:Ana", &cwd, &home), "@personas/Ana");
        assert_eq!(resolve("oficio:Minecraft 26.2", &cwd, &home), "@oficio/minecraft-26-2");
        assert_eq!(resolve(&home.join("Projects").to_string_lossy(), &cwd, &home), "Projects");
        assert_eq!(describe("@personas/Ana"), "persona: Ana");
        assert_eq!(describe("OneDrive/Escritorio/Projects/Juegos/Rol"), "Juegos/Rol");
        let _ = std::fs::remove_dir_all(&home);
    }
}
