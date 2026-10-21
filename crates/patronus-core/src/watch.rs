//! Déclencheur « fichier modifié » : surveillance des dossiers, changements rapprochés regroupés.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::Duration;

use globset::{Glob, GlobSet, GlobSetBuilder};
use notify_debouncer_mini::{DebounceEventResult, Debouncer, new_debouncer, notify::RecommendedWatcher, notify::RecursiveMode};

use crate::task::FileWatch;

/// Dossiers jamais pris en compte : l'outillage les touche sans cesse.
const ALWAYS_IGNORED: [&str; 3] = [".git", "target", "node_modules"];

/// Les motifs d'un déclencheur, compilés. Un motif sans `/` vaut aussi pour le nom seul du fichier,
/// comme dans un `.gitignore` (`*.toml` attrape `crates/a/Cargo.toml`).
pub struct Matcher {
    dossier: PathBuf,
    include: Option<GlobSet>,
    exclude: GlobSet,
}

fn build(patterns: &[String]) -> GlobSet {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        if let Ok(g) = Glob::new(p) {
            b.add(g);
        }
        if !p.contains('/') {
            if let Ok(g) = Glob::new(&format!("**/{p}")) {
                b.add(g);
            }
        }
    }
    b.build().unwrap_or_else(|_| GlobSet::empty())
}

impl Matcher {
    pub fn new(w: &FileWatch) -> Matcher {
        Matcher {
            dossier: w.dossier.clone(),
            include: (!w.inclure.is_empty()).then(|| build(&w.inclure)),
            exclude: build(&w.exclure),
        }
    }

    pub fn dossier(&self) -> &Path {
        &self.dossier
    }

    pub fn matches(&self, path: &Path) -> bool {
        let Some(rel) = strip_prefix_ci(path, &self.dossier) else { return false };
        let rel = rel.to_string_lossy().replace('\\', "/");
        if rel.is_empty() || rel.split('/').any(|part| ALWAYS_IGNORED.contains(&part)) {
            return false;
        }
        if self.exclude.is_match(&rel) {
            return false;
        }
        self.include.as_ref().is_none_or(|inc| inc.is_match(&rel))
    }
}

/// `strip_prefix` insensible à la casse, comme les chemins Windows.
fn strip_prefix_ci(path: &Path, base: &Path) -> Option<PathBuf> {
    let p = path.to_string_lossy().replace('\\', "/");
    let b = base.to_string_lossy().replace('\\', "/");
    let b = b.trim_end_matches('/');
    if p.len() > b.len() && p[..b.len()].eq_ignore_ascii_case(b) && p.as_bytes()[b.len()] == b'/' {
        Some(PathBuf::from(&p[b.len() + 1..]))
    } else {
        None
    }
}

/// Surveillance d'un ensemble de dossiers. Les chemins modifiés arrivent par paquets, après
/// `debounce` de calme. Abandonner la valeur arrête la surveillance.
pub struct Watcher {
    _debouncer: Debouncer<RecommendedWatcher>,
    /// Dossiers qu'on n'a pas pu surveiller, avec la raison.
    pub problems: Vec<String>,
}

impl Watcher {
    pub fn start(dirs: &BTreeSet<PathBuf>, debounce: Duration, tx: Sender<Vec<PathBuf>>) -> std::io::Result<Watcher> {
        let mut debouncer = new_debouncer(debounce, move |res: DebounceEventResult| {
            if let Ok(events) = res {
                let paths: Vec<PathBuf> = events.into_iter().map(|e| e.path).collect();
                if !paths.is_empty() {
                    let _ = tx.send(paths);
                }
            }
        })
        .map_err(std::io::Error::other)?;
        let mut problems = Vec::new();
        for d in dirs {
            if let Err(e) = debouncer.watcher().watch(d, RecursiveMode::Recursive) {
                problems.push(format!("Surveillance de {} impossible : {e}", d.display()));
            }
        }
        Ok(Watcher { _debouncer: debouncer, problems })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsutil::test_dir;

    fn watch(inc: &[&str], exc: &[&str]) -> Matcher {
        Matcher::new(&FileWatch {
            dossier: PathBuf::from(r"C:\dev\Northwind"),
            inclure: inc.iter().map(|s| s.to_string()).collect(),
            exclure: exc.iter().map(|s| s.to_string()).collect(),
        })
    }

    #[test]
    fn include_and_exclude() {
        let m = watch(&["docs/**", "*.toml"], &["docs/brouillon/**"]);
        assert!(m.matches(Path::new(r"C:\dev\Northwind\docs\guide.md")));
        assert!(m.matches(Path::new(r"c:\DEV\northwind\crates\a\Cargo.toml")), "casse et nom seul");
        assert!(!m.matches(Path::new(r"C:\dev\Northwind\docs\brouillon\x.md")));
        assert!(!m.matches(Path::new(r"C:\dev\Northwind\src\main.rs")));
        assert!(!m.matches(Path::new(r"C:\dev\Northwind2\docs\guide.md")), "dossier voisin");
    }

    #[test]
    fn everything_but_tooling_when_no_pattern() {
        let m = watch(&[], &[]);
        assert!(m.matches(Path::new(r"C:\dev\Northwind\src\main.rs")));
        assert!(!m.matches(Path::new(r"C:\dev\Northwind\.git\index")));
        assert!(!m.matches(Path::new(r"C:\dev\Northwind\target\debug\x.exe")));
        assert!(!m.matches(Path::new(r"C:\dev\Northwind")));
    }

    #[test]
    fn a_real_change_is_reported_once_grouped() {
        let dir = test_dir("watch");
        let (tx, rx) = std::sync::mpsc::channel();
        let w = Watcher::start(&BTreeSet::from([dir.clone()]), Duration::from_millis(300), tx).unwrap();
        assert!(w.problems.is_empty());
        std::thread::sleep(Duration::from_millis(200));
        for i in 0..5 {
            std::fs::write(dir.join(format!("f{i}.txt")), "x").unwrap();
        }
        let paths = rx.recv_timeout(Duration::from_secs(5)).expect("aucun changement signalé");
        assert!(paths.iter().any(|p| p.ends_with("f0.txt")));
        let mut all = paths;
        while let Ok(more) = rx.recv_timeout(Duration::from_millis(800)) {
            all.extend(more);
        }
        assert!(all.iter().any(|p| p.ends_with("f4.txt")));
    }
}
