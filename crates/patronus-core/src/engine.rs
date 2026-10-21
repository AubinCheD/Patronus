//! Le moteur : tâches chargées, départs dus, exécutions en cours et boîte de résultats. L'interface
//! l'appelle régulièrement (`tick`) ; tout le travail long se fait dans des threads.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

use crate::exec::{Context, Outcome, log, run_script};
use crate::fsutil::{read_text, write_atomic};
use crate::run::{Run, RunStore, Status};
use crate::schedule::{missed, next_due};
use crate::task::{Loaded, Task, TaskStore, Trigger};
use crate::watch::{Matcher, Watcher};

/// Ce que le moteur retient d'une session à l'autre (`planning.json`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Planning {
    /// Dernier instant où Patronus surveillait les échéances : ce qui est dû après, et avant
    /// l'ouverture suivante, a été manqué.
    #[serde(default)]
    pub derniere_verification: Option<NaiveDateTime>,
    #[serde(flatten)]
    pub autres: BTreeMap<String, serde_json::Value>,
}

/// Une action exécutée à la place d'un script ou d'une session Claude : pour les essais.
pub type Executor = Arc<dyn Fn(&str, &Task, &Context, &Path, &AtomicBool) -> Outcome + Send + Sync>;

struct Active {
    run_id: String,
    stop: Arc<AtomicBool>,
}

pub struct Engine {
    data_dir: PathBuf,
    pub tasks: TaskStore,
    pub store: RunStore,
    pub loaded: Vec<Loaded>,
    /// Boîte de résultats, les plus récentes d'abord.
    pub runs: Vec<Run>,
    /// Problèmes à montrer (fichiers illisibles, surveillance impossible…).
    pub problems: Vec<String>,
    active: HashMap<String, Active>,
    /// Un déclenchement pendant une exécution attend (un seul par tâche).
    pending: HashMap<String, Context>,
    next: HashMap<String, NaiveDateTime>,
    done_tx: Sender<(String, String, Outcome)>,
    done_rx: Receiver<(String, String, Outcome)>,
    files_tx: Sender<Vec<PathBuf>>,
    files_rx: Receiver<Vec<PathBuf>>,
    watcher: Option<Watcher>,
    watched: BTreeSet<PathBuf>,
    matchers: Vec<(String, Matcher)>,
    planning: Planning,
    /// Exécutions terminées depuis le dernier `take_finished` (pour les notifications).
    finished: Vec<String>,
    executor: Option<Executor>,
    pub debounce: Duration,
    pub retention_days: u32,
}

impl Engine {
    /// Ouvre le dossier de données : tâches, boîte, échéances manquées depuis la dernière fois.
    pub fn open(data_dir: &Path, now: NaiveDateTime) -> Engine {
        Self::open_with(data_dir, now, None, Duration::from_secs(2))
    }

    pub fn open_with(data_dir: &Path, now: NaiveDateTime, executor: Option<Executor>, debounce: Duration) -> Engine {
        let (done_tx, done_rx) = channel();
        let (files_tx, files_rx) = channel();
        let store = RunStore::new(data_dir.join("executions"));
        let (runs, problems) = store.load_all();
        let planning = read_text(&data_dir.join("planning.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let mut e = Engine {
            data_dir: data_dir.to_path_buf(),
            tasks: TaskStore::new(data_dir.join("taches")),
            store,
            loaded: Vec::new(),
            runs,
            problems,
            active: HashMap::new(),
            pending: HashMap::new(),
            next: HashMap::new(),
            done_tx,
            done_rx,
            files_tx,
            files_rx,
            watcher: None,
            watched: BTreeSet::new(),
            matchers: Vec::new(),
            planning,
            finished: Vec::new(),
            executor,
            debounce,
            retention_days: 30,
        };
        e.store.prune(&mut e.runs, e.retention_days, now);
        e.reload_tasks(now);
        e.report_missed(now);
        e
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Relit les tâches depuis le disque, recalcule les échéances et la surveillance des fichiers.
    pub fn reload_tasks(&mut self, now: NaiveDateTime) {
        self.loaded = self.tasks.load_all();
        self.next.clear();
        self.matchers.clear();
        for l in &self.loaded {
            let Ok(task) = &l.task else { continue };
            if !task.active || !task.validate().is_empty() {
                continue;
            }
            if let Some(at) = next_due(task, now) {
                self.next.insert(l.id.clone(), at);
            }
            for tr in &task.declencheurs {
                if let Trigger::Fichiers { fichiers } = tr {
                    self.matchers.push((l.id.clone(), Matcher::new(fichiers)));
                }
            }
        }
        let dirs: BTreeSet<PathBuf> =
            self.matchers.iter().map(|(_, m)| m.dossier().to_path_buf()).filter(|d| d.is_dir()).collect();
        if dirs != self.watched {
            self.watcher = None;
            if !dirs.is_empty() {
                match Watcher::start(&dirs, self.debounce, self.files_tx.clone()) {
                    Ok(w) => {
                        self.problems.extend(w.problems.iter().cloned());
                        self.watcher = Some(w);
                    }
                    Err(e) => self.problems.push(format!("Surveillance des fichiers impossible : {e}")),
                }
            }
            self.watched = dirs;
        }
        for (id, m) in &self.matchers {
            if !m.dossier().is_dir() {
                self.problems.push(format!("Tâche {id} : dossier surveillé introuvable ({}).", m.dossier().display()));
            }
        }
    }

    pub fn task(&self, id: &str) -> Option<&Task> {
        self.loaded.iter().find(|l| l.id == id).and_then(|l| l.task.as_ref().ok())
    }

    /// Enregistre une tâche (nouvel identifiant tiré du nom si `id` est absent) et la recharge.
    pub fn save_task(&mut self, id: Option<&str>, task: &Task, now: NaiveDateTime) -> std::io::Result<String> {
        let id = id.map(str::to_string).unwrap_or_else(|| self.tasks.free_id(&task.nom));
        self.tasks.save(&id, task)?;
        self.reload_tasks(now);
        Ok(id)
    }

    pub fn delete_task(&mut self, id: &str, now: NaiveDateTime) -> std::io::Result<()> {
        self.tasks.delete(id)?;
        self.pending.remove(id);
        self.reload_tasks(now);
        Ok(())
    }

    pub fn next_run(&self, task_id: &str) -> Option<NaiveDateTime> {
        self.next.get(task_id).copied()
    }

    pub fn is_running(&self, task_id: &str) -> bool {
        self.active.contains_key(task_id)
    }

    pub fn is_pending(&self, task_id: &str) -> bool {
        self.pending.contains_key(task_id)
    }

    pub fn running_count(&self) -> usize {
        self.active.len()
    }

    pub fn unread_count(&self) -> usize {
        self.runs.iter().filter(|r| !r.lu && !r.archive && r.statut != Status::EnCours).count()
    }

    pub fn last_run(&self, task_id: &str) -> Option<&Run> {
        self.runs.iter().find(|r| r.tache == task_id)
    }

    /// Lance une tâche. Si elle tourne déjà, le départ attend la fin (une seule fois).
    pub fn launch(&mut self, task_id: &str, ctx: Context, now: NaiveDateTime) {
        if self.active.contains_key(task_id) {
            self.pending
                .entry(task_id.to_string())
                .and_modify(|p| {
                    p.files.extend(ctx.files.iter().cloned());
                    p.files.sort();
                    p.files.dedup();
                })
                .or_insert(ctx);
            return;
        }
        let name = self.loaded.iter().find(|l| l.id == task_id).map(|l| match &l.task {
            Ok(t) => t.nom.clone(),
            Err(_) => task_id.to_string(),
        });
        let mut run =
            Run::new(self.store.new_id(task_id, now), task_id, name.as_deref().unwrap_or(task_id), &ctx.trigger, now);
        let task = match self.task(task_id) {
            None => Err(format!("Tâche introuvable ou illisible : {task_id}.")),
            Some(t) => {
                let problems = t.validate();
                if problems.is_empty() {
                    Ok(t.clone())
                } else {
                    Err(format!("Tâche invalide : {}", problems.join(" ")))
                }
            }
        };
        let task = match task {
            Ok(t) => t,
            Err(why) => {
                run.statut = Status::Impossible;
                run.fin = Some(now);
                run.resume = why;
                self.record(run);
                return;
            }
        };
        if let Err(e) = self.store.save(&run) {
            self.problems.push(format!("Enregistrement de l'exécution impossible : {e}"));
        }
        let stop = Arc::new(AtomicBool::new(false));
        self.active.insert(task_id.to_string(), Active { run_id: run.id.clone(), stop: stop.clone() });
        let journal = self.store.journal_path(&run.id);
        let tx = self.done_tx.clone();
        let (tid, rid) = (task_id.to_string(), run.id.clone());
        let executor = self.executor.clone();
        let data_dir = self.data_dir.clone();
        self.runs.insert(0, run);
        std::thread::spawn(move || {
            let outcome = match &executor {
                Some(f) => f(&tid, &task, &ctx, &journal, &stop),
                None if task.action.claude.is_some() => {
                    crate::claude::run_claude(&tid, &task, &ctx, &journal, &stop, &data_dir, &rid)
                }
                None => run_script(&tid, &task, &ctx, &journal, &stop),
            };
            let _ = tx.send((tid, rid, outcome));
        });
    }

    /// Demande l'arrêt de l'exécution en cours d'une tâche (et oublie le départ en attente).
    pub fn stop(&mut self, task_id: &str) {
        self.pending.remove(task_id);
        if let Some(a) = self.active.get(task_id) {
            a.stop.store(true, Ordering::Relaxed);
        }
    }

    pub fn stop_all(&mut self) {
        self.pending.clear();
        for a in self.active.values() {
            a.stop.store(true, Ordering::Relaxed);
        }
    }

    pub fn running_run_id(&self, task_id: &str) -> Option<&str> {
        self.active.get(task_id).map(|a| a.run_id.as_str())
    }

    /// Fait avancer le moteur : fins d'exécution, départs dus, fichiers modifiés. Rend vrai si
    /// quelque chose a changé.
    pub fn tick(&mut self, now: NaiveDateTime) -> bool {
        let mut changed = false;
        while let Ok((task_id, run_id, outcome)) = self.done_rx.try_recv() {
            changed = true;
            self.active.remove(&task_id);
            if let Some(run) = self.runs.iter_mut().find(|r| r.id == run_id) {
                run.statut = outcome.status;
                run.code = outcome.code;
                run.resume = outcome.summary;
                run.cout = outcome.cost;
                run.worktree = outcome.worktree;
                run.session_claude = outcome.session;
                run.fin = Some(now.max(run.debut));
                let run = run.clone();
                if let Err(e) = self.store.save(&run) {
                    self.problems.push(format!("Enregistrement de l'exécution impossible : {e}"));
                }
            }
            self.finished.push(run_id);
            if let Some(ctx) = self.pending.remove(&task_id) {
                self.launch(&task_id, ctx, now);
            }
        }
        let due: Vec<String> = self.next.iter().filter(|(_, at)| **at <= now).map(|(id, _)| id.clone()).collect();
        for id in due {
            let at = self.next[&id];
            if let Some(task) = self.task(&id) {
                match next_due(task, now) {
                    Some(n) => self.next.insert(id.clone(), n),
                    None => self.next.remove(&id),
                };
            }
            self.launch(&id, Context { trigger: format!("échéance de {}", at.format("%H:%M")), files: vec![] }, now);
            changed = true;
        }
        let mut changed_files: Vec<PathBuf> = Vec::new();
        while let Ok(paths) = self.files_rx.try_recv() {
            changed_files.extend(paths);
        }
        if !changed_files.is_empty() {
            changed_files.sort();
            changed_files.dedup();
            let hits: Vec<(String, Vec<PathBuf>)> = self
                .matchers
                .iter()
                .map(|(id, m)| {
                    (
                        id.clone(),
                        changed_files
                            .iter()
                            .filter(|p| m.matches(p) && !p.starts_with(&self.data_dir))
                            .cloned()
                            .collect::<Vec<_>>(),
                    )
                })
                .filter(|(_, files)| !files.is_empty())
                .collect();
            for (id, files) in hits {
                // Une tâche qui écrit dans le dossier qu'elle surveille se relancerait sans fin :
                // ce qu'elle modifie pendant qu'elle tourne ne la redéclenche pas.
                if let Some(run_id) = self.running_run_id(&id) {
                    log(
                        &self.store.journal_path(run_id),
                        &format!("[patronus] {} fichier(s) modifié(s) pendant l'exécution, ignoré(s)", files.len()),
                    );
                    continue;
                }
                let base = self
                    .matchers
                    .iter()
                    .find(|(tid, _)| *tid == id)
                    .map(|(_, m)| m.dossier().to_path_buf())
                    .unwrap_or_default();
                let trigger = match files.as_slice() {
                    [one] => format!("fichier modifié : {}", one.strip_prefix(&base).unwrap_or(one).display()),
                    many => format!("{} fichiers modifiés", many.len()),
                };
                self.launch(&id, Context { trigger, files }, now);
                changed = true;
            }
        }
        self.heartbeat(now);
        changed
    }

    /// Exécutions terminées depuis le dernier appel.
    pub fn take_finished(&mut self) -> Vec<Run> {
        let ids = std::mem::take(&mut self.finished);
        ids.iter().filter_map(|id| self.runs.iter().find(|r| &r.id == id).cloned()).collect()
    }

    pub fn mark_read(&mut self, run_id: &str, lu: bool) {
        self.update_run(run_id, |r| r.lu = lu);
    }

    pub fn archive(&mut self, run_id: &str) {
        self.update_run(run_id, |r| {
            r.archive = true;
            r.lu = true;
        });
    }

    fn update_run(&mut self, run_id: &str, f: impl FnOnce(&mut Run)) {
        if let Some(r) = self.runs.iter_mut().find(|r| r.id == run_id) {
            f(r);
            let r = r.clone();
            if let Err(e) = self.store.save(&r) {
                self.problems.push(format!("Enregistrement de l'exécution impossible : {e}"));
            }
        }
    }

    fn record(&mut self, run: Run) {
        if let Err(e) = self.store.save(&run) {
            self.problems.push(format!("Enregistrement de l'exécution impossible : {e}"));
        }
        self.finished.push(run.id.clone());
        self.runs.insert(0, run);
    }

    /// Ce qui devait partir pendant que Patronus était fermé : rattrapé si la tâche le demande,
    /// sinon signalé dans la boîte.
    fn report_missed(&mut self, now: NaiveDateTime) {
        if let Some(last) = self.planning.derniere_verification {
            let tasks: Vec<(String, Task)> = self
                .loaded
                .iter()
                .filter_map(|l| l.task.as_ref().ok().map(|t| (l.id.clone(), t.clone())))
                .filter(|(_, t)| t.active && t.validate().is_empty())
                .collect();
            for (id, task) in tasks {
                let missed = missed(&task, last, now);
                if missed.is_empty() {
                    continue;
                }
                if let Some(m) = missed.iter().filter(|m| m.catch_up).max_by_key(|m| m.at) {
                    let trigger = format!("rattrapage de {}", m.at.format("%d/%m %H:%M"));
                    self.launch(&id, Context { trigger, files: vec![] }, now);
                    continue;
                }
                let latest = missed.iter().map(|m| m.at).max().unwrap_or(now);
                let count: usize = missed.iter().map(|m| m.count).sum();
                let mut run = Run::new(
                    self.store.new_id(&id, now),
                    &id,
                    &task.nom,
                    &format!("échéance de {}", latest.format("%d/%m %H:%M")),
                    latest,
                );
                run.statut = Status::Manquee;
                run.fin = Some(latest);
                run.resume = if count > 1 {
                    format!(
                        "{count} départs manqués pendant que Patronus était fermé, le dernier le {}.",
                        latest.format("%d/%m à %H:%M")
                    )
                } else {
                    format!("Départ du {} manqué : Patronus était fermé.", latest.format("%d/%m à %H:%M"))
                };
                self.record(run);
            }
        }
        self.planning.derniere_verification = Some(now);
        self.save_planning();
    }

    fn heartbeat(&mut self, now: NaiveDateTime) {
        let stale = self.planning.derniere_verification.is_none_or(|t| (now - t).num_seconds() >= 30);
        if stale {
            self.planning.derniere_verification = Some(now);
            self.save_planning();
        }
    }

    fn save_planning(&mut self) {
        let json = serde_json::to_vec_pretty(&self.planning).unwrap_or_default();
        if let Err(e) = write_atomic(&self.data_dir.join("planning.json"), &json) {
            self.problems.push(format!("Enregistrement de planning.json impossible : {e}"));
        }
    }

    /// À la fermeture : arrête ce qui tourne, attend un peu, et note l'heure.
    pub fn shutdown(&mut self, now: NaiveDateTime, wait: Duration) {
        self.stop_all();
        let deadline = std::time::Instant::now() + wait;
        while !self.active.is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            self.tick(now);
        }
        self.planning.derniere_verification = Some(now);
        self.save_planning();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsutil::test_dir;
    use crate::task::{FileWatch, Script, Shell};
    use chrono::NaiveDate;
    use std::time::Instant;

    fn at(d: u32, h: u32, m: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, d).unwrap().and_hms_opt(h, m, 0).unwrap()
    }

    fn task(cmd: &str, triggers: Vec<Trigger>) -> Task {
        let mut t = Task::new_script("Essai northwind");
        t.action.script =
            Some(Script { commande: cmd.into(), shell: Shell::Cmd, dossier: None, env: Default::default() });
        t.declencheurs = triggers;
        t
    }

    /// Faux exécuteur : note les départs, attend `hold` (interruptible) puis réussit.
    fn fake(hold: Duration) -> (Executor, Arc<parking::Counter>) {
        let counter = Arc::new(parking::Counter::default());
        let c = counter.clone();
        let f: Executor = Arc::new(move |_id, _task, ctx, journal, stop| {
            c.add(&ctx.trigger);
            log(journal, &format!("faux : {}", ctx.trigger));
            let start = Instant::now();
            while start.elapsed() < hold {
                if stop.load(Ordering::Relaxed) {
                    return Outcome {
                        status: Status::Arretee,
                        code: None,
                        summary: "arrêtée".into(),
                        cost: None,
                        worktree: None,
                        session: None,
                    };
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Outcome {
                status: Status::Reussie,
                code: Some(0),
                summary: format!("ok : {}", ctx.trigger),
                cost: None,
                worktree: None,
                session: None,
            }
        });
        (f, counter)
    }

    mod parking {
        #[derive(Default)]
        pub struct Counter(std::sync::Mutex<Vec<String>>);
        impl Counter {
            pub fn add(&self, s: &str) {
                self.0.lock().unwrap().push(s.to_string());
            }
            pub fn all(&self) -> Vec<String> {
                self.0.lock().unwrap().clone()
            }
        }
    }

    fn wait_idle(e: &mut Engine, now: NaiveDateTime) {
        let start = Instant::now();
        while e.running_count() > 0 && start.elapsed() < Duration::from_secs(10) {
            e.tick(now);
            std::thread::sleep(Duration::from_millis(20));
        }
        e.tick(now);
    }

    #[test]
    fn manual_launch_lands_in_the_inbox_and_survives_restart() {
        let dir = test_dir("engine-manual");
        let now = at(21, 10, 0);
        let mut e = Engine::open(&dir, now);
        let id = e.save_task(None, &task("echo bonjour fabrikam", vec![]), now).unwrap();
        e.launch(&id, Context { trigger: "à la main".into(), files: vec![] }, now);
        assert!(e.is_running(&id));
        wait_idle(&mut e, now);
        assert_eq!(e.runs.len(), 1);
        assert_eq!(e.runs[0].statut, Status::Reussie, "{}", e.runs[0].resume);
        assert!(e.runs[0].resume.contains("bonjour fabrikam"));
        assert_eq!(e.unread_count(), 1);
        assert_eq!(e.take_finished().len(), 1);
        drop(e);
        let e = Engine::open(&dir, at(21, 10, 5));
        assert_eq!(e.runs.len(), 1);
        assert_eq!(e.runs[0].statut, Status::Reussie);
        assert!(e.store.read_journal(&e.runs[0].id).contains("bonjour fabrikam"));
    }

    #[test]
    fn scheduled_task_starts_when_due_once() {
        let dir = test_dir("engine-due");
        let (exec, counter) = fake(Duration::ZERO);
        let mut e = Engine::open_with(&dir, at(21, 7, 58), Some(exec), Duration::from_millis(200));
        let id = e
            .save_task(
                None,
                &task("x", vec![Trigger::Heure { heure: "08:00".into(), jours: vec![], rattrapage: false }]),
                at(21, 7, 58),
            )
            .unwrap();
        assert_eq!(e.next_run(&id), Some(at(21, 8, 0)));
        e.tick(at(21, 7, 59));
        assert!(counter.all().is_empty());
        e.tick(at(21, 8, 0));
        wait_idle(&mut e, at(21, 8, 0));
        e.tick(at(21, 8, 1));
        assert_eq!(counter.all(), ["échéance de 08:00"]);
        assert_eq!(e.next_run(&id), Some(at(22, 8, 0)));
    }

    #[test]
    fn one_run_at_a_time_and_one_queued() {
        let dir = test_dir("engine-queue");
        let (exec, counter) = fake(Duration::from_millis(400));
        let now = at(21, 10, 0);
        let mut e = Engine::open_with(&dir, now, Some(exec), Duration::from_millis(200));
        let id = e.save_task(None, &task("x", vec![]), now).unwrap();
        for i in 0..4 {
            e.launch(&id, Context { trigger: format!("n{i}"), files: vec![] }, now);
        }
        assert!(e.is_pending(&id));
        wait_idle(&mut e, now);
        assert_eq!(counter.all(), ["n0", "n1"], "le premier en attente gagne, les suivants s'y fondent");
    }

    #[test]
    fn stop_ends_the_run_with_a_stopped_entry() {
        let dir = test_dir("engine-stop");
        let (exec, _) = fake(Duration::from_secs(30));
        let now = at(21, 10, 0);
        let mut e = Engine::open_with(&dir, now, Some(exec), Duration::from_millis(200));
        let id = e.save_task(None, &task("x", vec![]), now).unwrap();
        e.launch(&id, Context { trigger: "à la main".into(), files: vec![] }, now);
        e.stop(&id);
        wait_idle(&mut e, now);
        assert_eq!(e.runs[0].statut, Status::Arretee);
    }

    #[test]
    fn invalid_or_paused_tasks() {
        let dir = test_dir("engine-invalid");
        let now = at(21, 10, 0);
        let mut e = Engine::open(&dir, now);
        let mut bad = task("", vec![Trigger::Intervalle { toutes_les_minutes: 5 }]);
        let id = e.save_task(None, &bad, now).unwrap();
        assert_eq!(e.next_run(&id), None, "une tâche invalide n'est pas planifiée");
        e.launch(&id, Context { trigger: "à la main".into(), files: vec![] }, now);
        assert_eq!(e.runs[0].statut, Status::Impossible);
        assert!(e.runs[0].resume.contains("commande du script est vide"));
        bad.action.script.as_mut().unwrap().commande = "echo x".into();
        bad.active = false;
        e.save_task(Some(&id), &bad, now).unwrap();
        assert_eq!(e.next_run(&id), None, "en pause");
    }

    #[test]
    fn missed_runs_are_reported_or_caught_up_at_opening() {
        let dir = test_dir("engine-missed");
        let (exec, counter) = fake(Duration::ZERO);
        let mut e = Engine::open_with(&dir, at(20, 18, 0), Some(exec.clone()), Duration::from_millis(200));
        e.save_task(
            Some("signale"),
            &task("x", vec![Trigger::Heure { heure: "08:00".into(), jours: vec![], rattrapage: false }]),
            at(20, 18, 0),
        )
        .unwrap();
        e.save_task(
            Some("rattrape"),
            &task("x", vec![Trigger::Heure { heure: "09:00".into(), jours: vec![], rattrapage: true }]),
            at(20, 18, 0),
        )
        .unwrap();
        e.shutdown(at(20, 18, 0), Duration::from_secs(1));
        drop(e);
        // Rouvert le lendemain à 10 h.
        let mut e = Engine::open_with(&dir, at(21, 10, 0), Some(exec), Duration::from_millis(200));
        wait_idle(&mut e, at(21, 10, 0));
        assert_eq!(counter.all(), ["rattrapage de 21/10 09:00"]);
        let m = e.runs.iter().find(|r| r.tache == "signale").unwrap();
        assert_eq!(m.statut, Status::Manquee);
        assert!(m.resume.contains("21/10 à 08:00"), "{}", m.resume);
        assert!(!m.lu);
    }

    #[test]
    fn file_change_triggers_the_task_with_the_files() {
        let dir = test_dir("engine-files");
        let watched = dir.join("northwind");
        std::fs::create_dir_all(watched.join("docs")).unwrap();
        let (exec, counter) = fake(Duration::ZERO);
        let now = at(21, 10, 0);
        let mut e = Engine::open_with(&dir.join("data"), now, Some(exec), Duration::from_millis(200));
        let fw = FileWatch { dossier: watched.clone(), inclure: vec!["docs/**".into()], exclure: vec![] };
        e.save_task(None, &task("x", vec![Trigger::Fichiers { fichiers: fw }]), now).unwrap();
        assert!(e.problems.is_empty(), "{:?}", e.problems);
        std::thread::sleep(Duration::from_millis(200));
        std::fs::write(watched.join("ignore.txt"), "x").unwrap();
        std::fs::write(watched.join("docs").join("guide.md"), "x").unwrap();
        let start = Instant::now();
        while counter.all().is_empty() && start.elapsed() < Duration::from_secs(5) {
            e.tick(now);
            std::thread::sleep(Duration::from_millis(50));
        }
        let all = counter.all();
        assert_eq!(all.len(), 1, "{all:?}");
        assert!(all[0].contains("guide.md"), "{all:?}");
    }
}
