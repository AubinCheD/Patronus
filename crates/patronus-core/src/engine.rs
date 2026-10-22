//! Le moteur : tâches chargées, départs dus, exécutions en cours et boîte de résultats. Son fil
//! (`Ticker`) le fait avancer (`tick`) ; tout le travail long se fait dans des fils à part.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

use crate::exec::{Context, Outcome, log, run_script};
use crate::fsutil::{read_text, write_atomic};
use crate::lock::TaskLock;
use crate::remote::Poll;
use crate::run::{Owner, Run, RunStore, Status};
use crate::schedule::{MISSED_COUNT_CAP, Missed, missed, next_due};
use crate::task::{Loaded, Task, TaskStore, Trigger};
use crate::watch::{Matcher, Watcher};

/// Ce que le moteur retient d'une session à l'autre (`planning.json`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Planning {
    /// Dernier instant où Patronus surveillait les échéances : ce qui est dû après, et avant
    /// l'ouverture suivante, a été manqué.
    #[serde(default)]
    pub derniere_verification: Option<NaiveDateTime>,
    /// Dernier commit vu de chaque branche distante surveillée (clé `<tâche>#<n° du déclencheur>`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub branches: BTreeMap<String, String>,
    #[serde(flatten)]
    pub autres: BTreeMap<String, serde_json::Value>,
}

/// Retard au-delà duquel une échéance est « manquée » (veille) plutôt que lancée en retard.
const LATE_MINUTES: i64 = 5;

/// Date de modification d'un fichier, en heure locale.
fn file_time(path: &Path) -> Option<NaiveDateTime> {
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
    Some(chrono::DateTime::<chrono::Local>::from(modified).naive_local())
}

/// Le moteur partagé entre son fil et l'interface.
pub type Shared = Arc<Mutex<Engine>>;

/// Verrouille le moteur ; un fil qui a paniqué en le tenant ne le rend pas inutilisable.
pub fn lock(engine: &Shared) -> MutexGuard<'_, Engine> {
    engine.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Le fil du moteur : un tour toutes les `period`, que l'interface soit affichée ou non. Après chaque
/// tour, `on_tick` reçoit le moteur et vrai si quelque chose a changé. Abandonner la valeur arrête le
/// fil (et attend la fin du tour en cours).
pub struct Ticker {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Ticker {
    pub fn start(
        engine: Shared,
        period: Duration,
        clock: fn() -> NaiveDateTime,
        mut on_tick: impl FnMut(&mut Engine, bool) + Send + 'static,
    ) -> Ticker {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = std::thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                {
                    let mut e = lock(&engine);
                    let changed = e.tick(clock());
                    on_tick(&mut e, changed);
                }
                std::thread::sleep(period);
            }
        });
        Ticker { stop, handle: Some(handle) }
    }
}

impl Drop for Ticker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Une action exécutée à la place d'un script ou d'une session Claude : pour les essais.
pub type Executor = Arc<dyn Fn(&str, &Task, &Context, &Path, &AtomicBool) -> Outcome + Send + Sync>;

struct Active {
    run_id: String,
    stop: Arc<AtomicBool>,
}

/// Fenêtre de temps de la dernière exécution d'une tâche : ce qu'elle a écrit pendant ce temps lui
/// est attribué.
struct Window {
    run_id: String,
    start: SystemTime,
    end: Option<SystemTime>,
}

/// Comment ouvrir le moteur.
#[derive(Clone)]
pub struct Options {
    /// Essais : une action exécutée à la place du script ou de la session.
    pub executor: Option<Executor>,
    /// Calme attendu avant de signaler des fichiers modifiés.
    pub debounce: Duration,
    /// Conservation des résultats lus ou archivés, en jours.
    pub retention_days: u32,
    /// Sans fenêtre (`patronus lancer`, Planificateur) : ni rétention, ni échéances manquées, ni
    /// surveillance des fichiers ; le moteur ne fait que lancer ce qu'on lui demande.
    pub headless: bool,
    /// Relecture de la boîte (résultats écrits par un autre processus).
    pub refresh_every: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            executor: None,
            debounce: Duration::from_secs(2),
            retention_days: 30,
            headless: false,
            refresh_every: Duration::from_secs(5),
        }
    }
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
    windows: HashMap<String, Window>,
    /// Date de modification déjà prise en compte, par tâche et par fichier : un même changement
    /// signalé deux fois (création puis écriture, en deux paquets) ne relance pas deux fois.
    seen: HashMap<(String, PathBuf), SystemTime>,
    done_tx: Sender<(String, String, Outcome)>,
    done_rx: Receiver<(String, String, Outcome)>,
    files_tx: Sender<Result<Vec<PathBuf>, String>>,
    files_rx: Receiver<Result<Vec<PathBuf>, String>>,
    watcher: Option<Watcher>,
    watched: BTreeSet<PathBuf>,
    matchers: Vec<(String, Matcher)>,
    planning: Planning,
    /// Exécutions terminées depuis le dernier `take_finished` (pour les notifications).
    finished: Vec<String>,
    executor: Option<Executor>,
    pub debounce: Duration,
    pub retention_days: u32,
    headless: bool,
    refresh_every: Duration,
    last_refresh: Instant,
    /// Rétention et rapprochement des worktrees à faire au prochain tour (dans le fil du moteur).
    maintenance_due: bool,
    /// Fermeture demandée : plus aucun départ.
    shutting_down: bool,
    /// Augmente à chaque relecture des tâches (l'interface resynchronise alors le Planificateur).
    pub tasks_version: u64,
    /// Branches distantes : prochain relevé, relevés en cours, résultats.
    branch_next: HashMap<String, Instant>,
    branch_busy: BTreeSet<String>,
    branch_tx: Sender<(String, String, String, Poll)>,
    branch_rx: Receiver<(String, String, String, Poll)>,
}

impl Engine {
    /// Ouvre le dossier de données : tâches, boîte, échéances manquées depuis la dernière fois.
    pub fn open(data_dir: &Path, now: NaiveDateTime) -> Engine {
        Self::open_opts(data_dir, now, Options::default())
    }

    /// Sans fenêtre : voir `Options::headless`.
    pub fn open_headless(data_dir: &Path, now: NaiveDateTime) -> Engine {
        Self::open_opts(data_dir, now, Options { headless: true, ..Options::default() })
    }

    pub fn open_with(data_dir: &Path, now: NaiveDateTime, executor: Option<Executor>, debounce: Duration) -> Engine {
        Self::open_opts(data_dir, now, Options { executor, debounce, ..Options::default() })
    }

    pub fn open_opts(data_dir: &Path, now: NaiveDateTime, opts: Options) -> Engine {
        let (done_tx, done_rx) = channel();
        let (files_tx, files_rx) = channel();
        let (branch_tx, branch_rx) = channel();
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
            windows: HashMap::new(),
            seen: HashMap::new(),
            done_tx,
            done_rx,
            files_tx,
            files_rx,
            watcher: None,
            watched: BTreeSet::new(),
            matchers: Vec::new(),
            planning,
            finished: Vec::new(),
            executor: opts.executor,
            debounce: opts.debounce,
            retention_days: opts.retention_days,
            headless: opts.headless,
            refresh_every: opts.refresh_every,
            last_refresh: Instant::now(),
            maintenance_due: !opts.headless,
            shutting_down: false,
            tasks_version: 0,
            branch_next: HashMap::new(),
            branch_busy: BTreeSet::new(),
            branch_tx,
            branch_rx,
        };
        e.reload_tasks(now);
        if !e.headless {
            e.report_missed(now);
        }
        e
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Relit les tâches depuis le disque, recalcule les échéances et la surveillance des fichiers.
    pub fn reload_tasks(&mut self, now: NaiveDateTime) {
        self.loaded = self.tasks.load_all();
        self.tasks_version += 1;
        self.next.clear();
        self.matchers.clear();
        for l in &self.loaded {
            let Ok(task) = &l.task else { continue };
            if !task.active || !l.problems.is_empty() {
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
        self.update_watcher();
    }

    /// Surveille les dossiers des déclencheurs « fichiers » qui existent ; rappelé régulièrement, pour
    /// qu'un dossier créé après coup soit surveillé. Les dossiers introuvables sont signalés (une
    /// fois, et plus quand ils apparaissent).
    fn update_watcher(&mut self) {
        if self.headless {
            return;
        }
        let dirs: BTreeSet<PathBuf> =
            self.matchers.iter().map(|(_, m)| m.dossier().to_path_buf()).filter(|d| d.is_dir()).collect();
        if dirs != self.watched {
            self.watcher = None;
            if !dirs.is_empty() {
                match Watcher::start(&dirs, self.debounce, self.files_tx.clone()) {
                    Ok(w) => {
                        for p in w.problems.clone() {
                            self.problem(p);
                        }
                        self.watcher = Some(w);
                    }
                    Err(e) => self.problem(format!("Surveillance des fichiers impossible : {e}")),
                }
            }
            self.watched = dirs;
        }
        const MISSING: &str = "dossier surveillé introuvable";
        self.problems.retain(|p| !p.contains(MISSING));
        let missing: Vec<String> = self
            .matchers
            .iter()
            .filter(|(_, m)| !m.dossier().is_dir())
            .map(|(id, m)| format!("Tâche {id} : {MISSING} ({}).", m.dossier().display()))
            .collect();
        for p in missing {
            self.problem(p);
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

    /// Lance une tâche. Si elle tourne déjà, ici ou dans un autre processus, le départ attend la fin
    /// (une seule fois).
    pub fn launch(&mut self, task_id: &str, ctx: Context, now: NaiveDateTime) {
        let _ = self.start(task_id, ctx, now, true);
    }

    /// Lance une tâche, ou dit pourquoi elle ne part pas : elle tourne déjà (ici ou ailleurs). Pour
    /// la ligne de commande, qui n'attend pas.
    pub fn try_launch(&mut self, task_id: &str, ctx: Context, now: NaiveDateTime) -> Result<(), String> {
        self.start(task_id, ctx, now, false)
    }

    fn locks_dir(&self) -> PathBuf {
        self.data_dir.join("verrous")
    }

    /// La tâche tourne-t-elle dans un autre processus (fenêtre, ligne de commande, Planificateur) ?
    pub fn running_elsewhere(&self, task_id: &str) -> Option<Owner> {
        if self.active.contains_key(task_id) {
            return None;
        }
        crate::lock::holder(&self.locks_dir(), task_id)
    }

    fn queue(&mut self, task_id: &str, ctx: Context) {
        self.pending
            .entry(task_id.to_string())
            .and_modify(|p| {
                p.files.extend(ctx.files.iter().cloned());
                p.files.sort();
                p.files.dedup();
            })
            .or_insert(ctx);
    }

    fn start(&mut self, task_id: &str, ctx: Context, now: NaiveDateTime, queue_if_busy: bool) -> Result<(), String> {
        if self.shutting_down {
            return Err("Patronus se ferme.".into());
        }
        if self.active.contains_key(task_id) {
            if queue_if_busy {
                self.queue(task_id, ctx);
            }
            return Err(format!("La tâche {task_id} est déjà en cours."));
        }
        let lock = match TaskLock::acquire(&self.locks_dir(), task_id) {
            Ok(l) => l,
            Err(owner) => {
                if queue_if_busy {
                    self.queue(task_id, ctx);
                }
                return Err(format!(
                    "La tâche {task_id} est déjà en cours dans un autre processus (n° {}).",
                    owner.pid
                ));
            }
        };
        let name = self.loaded.iter().find(|l| l.id == task_id).map(|l| match &l.task {
            Ok(t) => t.nom.clone(),
            Err(_) => task_id.to_string(),
        });
        let mut run =
            Run::new(self.store.new_id(task_id, now), task_id, name.as_deref().unwrap_or(task_id), &ctx.trigger, now);
        run.proprietaire = Some(Owner::current());
        run.depot = self.task(task_id).and_then(|t| t.depot.clone());
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
                run.resume = why.clone();
                self.record(run);
                return Err(why);
            }
        };
        if let Err(e) = self.store.save(&run) {
            self.problem(format!("Enregistrement de l'exécution impossible : {e}"));
        }
        let stop = Arc::new(AtomicBool::new(false));
        self.active.insert(task_id.to_string(), Active { run_id: run.id.clone(), stop: stop.clone() });
        self.windows
            .insert(task_id.to_string(), Window { run_id: run.id.clone(), start: SystemTime::now(), end: None });
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
            // Le verrou est rendu avant d'annoncer la fin : un départ en attente peut le prendre aussitôt.
            drop(lock);
            let _ = tx.send((tid, rid, outcome));
        });
        Ok(())
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
            if let Some(w) = self.windows.get_mut(&task_id) {
                w.end = Some(SystemTime::now());
            }
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
                    self.problem(format!("Enregistrement de l'exécution impossible : {e}"));
                }
            }
            self.finished.push(run_id);
            if let Some(ctx) = self.pending.remove(&task_id) {
                self.launch(&task_id, ctx, now);
            }
        }
        if self.last_refresh.elapsed() >= self.refresh_every {
            self.last_refresh = Instant::now();
            changed |= self.refresh_runs();
            self.update_watcher();
            // Départs en attente d'une exécution menée par un autre processus, maintenant terminée.
            let ready: Vec<String> = self
                .pending
                .keys()
                .filter(|id| !self.active.contains_key(*id) && self.running_elsewhere(id).is_none())
                .cloned()
                .collect();
            for id in ready {
                if let Some(ctx) = self.pending.remove(&id) {
                    self.launch(&id, ctx, now);
                    changed = true;
                }
            }
        }
        // Sans fenêtre, le moteur ne lance que ce qu'on lui demande.
        if self.headless {
            return changed;
        }
        if self.maintenance_due {
            self.maintenance_due = false;
            changed |= self.maintenance(now);
        }
        let due: Vec<String> = self.next.iter().filter(|(_, at)| **at <= now).map(|(id, _)| id.clone()).collect();
        for id in due {
            let at = self.next[&id];
            let Some(task) = self.task(&id).cloned() else {
                self.next.remove(&id);
                continue;
            };
            match next_due(&task, now) {
                Some(n) => self.next.insert(id.clone(), n),
                None => self.next.remove(&id),
            };
            changed = true;
            // Plus de 5 minutes de retard (ordinateur en veille, en hibernation) : comme une échéance
            // manquée application fermée, rattrapée seulement si la tâche le demande.
            if now - at > chrono::Duration::minutes(LATE_MINUTES) {
                let missed = missed(&task, at - chrono::Duration::seconds(1), now);
                self.handle_missed(
                    &id,
                    &task,
                    &missed,
                    now,
                    "l'ordinateur était en veille ou Patronus ne tournait pas",
                );
                continue;
            }
            self.launch(
                &id,
                Context {
                    trigger: format!("échéance de {}", at.format("%H:%M")), files: vec![], ..Default::default()
                },
                now,
            );
        }
        let mut changed_files: Vec<PathBuf> = Vec::new();
        while let Ok(msg) = self.files_rx.try_recv() {
            match msg {
                Ok(paths) => changed_files.extend(paths),
                Err(e) => self.problem(e),
            }
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
                            .filter(|p| m.matches(p) && crate::watch::strip_prefix_ci(p, &self.data_dir).is_none())
                            .cloned()
                            .collect::<Vec<_>>(),
                    )
                })
                .filter(|(_, files)| !files.is_empty())
                .collect();
            for (id, files) in hits {
                // Une tâche qui écrit dans le dossier qu'elle surveille se relancerait sans fin : ce
                // qui a été modifié pendant sa dernière exécution lui est attribué et ne la relance pas.
                let files: Vec<PathBuf> = files.into_iter().filter(|f| self.is_new_change(&id, f)).collect();
                let (own, files): (Vec<PathBuf>, Vec<PathBuf>) =
                    files.into_iter().partition(|f| self.attributed_to_last_run(&id, f));
                if let (false, Some(w)) = (own.is_empty(), self.windows.get(&id)) {
                    let names: Vec<String> = own.iter().map(|f| f.display().to_string()).collect();
                    log(
                        &self.store.journal_path(&w.run_id),
                        &format!(
                            "[patronus] {} fichier(s) modifié(s) pendant l'exécution, attribué(s) à la tâche et ignoré(s) : {}",
                            own.len(),
                            names.join(", ")
                        ),
                    );
                }
                if files.is_empty() {
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
                self.launch(&id, Context { trigger, files, ..Default::default() }, now);
                changed = true;
            }
        }
        changed |= self.poll_branches(now);
        self.heartbeat(now);
        changed
    }

    /// Ce changement n'a-t-il pas déjà été pris en compte pour cette tâche ? (Un fichier supprimé est
    /// toujours un changement.) Retient la date vue ; oublie celles de plus d'une minute.
    fn is_new_change(&mut self, task_id: &str, file: &Path) -> bool {
        let Ok(modified) = std::fs::metadata(file).and_then(|m| m.modified()) else { return true };
        let now = SystemTime::now();
        self.seen.retain(|_, t| now.duration_since(*t).is_ok_and(|d| d < Duration::from_secs(60)));
        let key = (task_id.to_string(), file.to_path_buf());
        if self.seen.get(&key).is_some_and(|t| modified <= *t) {
            return false;
        }
        self.seen.insert(key, modified);
        true
    }

    /// Le fichier a-t-il été modifié pendant la dernière exécution de la tâche (ou dans la seconde
    /// qui suit) ? Un fichier supprimé l'est s'il est signalé pendant l'exécution, ou au plus tard
    /// le délai de regroupement (plus une seconde) après sa fin.
    fn attributed_to_last_run(&self, task_id: &str, file: &Path) -> bool {
        let Some(w) = self.windows.get(task_id) else { return false };
        let margin = Duration::from_secs(1);
        match std::fs::metadata(file).and_then(|m| m.modified()) {
            Ok(modified) => modified >= w.start && w.end.is_none_or(|end| modified <= end + margin),
            Err(_) => w.end.is_none_or(|end| SystemTime::now() <= end + self.debounce + margin),
        }
    }

    /// Une exécution de la tâche a-t-elle commencé à l'échéance `at` ou après (une minute de tolérance),
    /// quel que soit le processus qui l'a menée ?
    pub fn already_done(&self, task_id: &str, at: NaiveDateTime) -> bool {
        self.runs
            .iter()
            .any(|r| r.tache == task_id && r.statut != Status::Manquee && r.debut >= at - chrono::Duration::minutes(1))
    }

    /// Que faire quand le Planificateur de Windows lance `planifie <tâche>` (fenêtre fermée) : la
    /// dernière échéance de la tâche, si elle reste à tenir, ou pourquoi il n'y a rien à faire.
    pub fn planned_run(&self, task_id: &str, now: NaiveDateTime) -> Result<NaiveDateTime, String> {
        let l = self.loaded.iter().find(|l| l.id == task_id).ok_or_else(|| format!("tâche inconnue : {task_id}"))?;
        let task = l.task.as_ref().map_err(|e| format!("tâche illisible : {e}"))?;
        if !l.problems.is_empty() {
            return Err(format!("tâche invalide : {}", l.problems.join(" ")));
        }
        if !task.active || !task.hors_application {
            return Err("tâche en pause, ou qui ne tourne pas application fermée".into());
        }
        // Le Planificateur peut partir un peu en avance : l'échéance de la minute qui vient compte.
        let at =
            crate::schedule::latest_due(task, now + chrono::Duration::minutes(1)).ok_or("aucune échéance horaire")?;
        if self.already_done(task_id, at) {
            return Err(format!("échéance de {} déjà tenue", at.format("%H:%M")));
        }
        Ok(at)
    }

    fn finished_run(&self, run_id: &str) -> Result<&Run, String> {
        let r = self.runs.iter().find(|r| r.id == run_id).ok_or("exécution introuvable")?;
        if r.statut == Status::EnCours {
            return Err("l'exécution est encore en cours".into());
        }
        Ok(r)
    }

    /// Ce qu'appliquer le diff d'une exécution toucherait (dépôt, branche, fichiers), ou pourquoi ce
    /// n'est pas possible (déjà appliqué, copie de travail qui a divergé…).
    pub fn diff_plan(&self, run_id: &str) -> Result<crate::actions::ApplyPlan, String> {
        let r = self.finished_run(run_id)?;
        if let Some(at) = r.diff_applique {
            return Err(format!("diff déjà appliqué le {}", at.format("%d/%m à %H:%M")));
        }
        let repo = r.depot.as_ref().ok_or("dépôt de l'exécution inconnu")?;
        let patch = self.store.diff_path(run_id);
        if !patch.exists() {
            return Err("cette exécution n'a pas de diff".into());
        }
        crate::actions::check_apply(repo, &patch)
    }

    /// Applique le diff d'une exécution dans la copie de travail de son dépôt (ni index, ni commit), à
    /// la demande de l'utilisateur ; l'entrée note quand.
    pub fn apply_diff(&mut self, run_id: &str, now: NaiveDateTime) -> Result<crate::actions::ApplyPlan, String> {
        let plan = self.diff_plan(run_id)?;
        let repo = self.finished_run(run_id)?.depot.clone().unwrap_or_default();
        crate::actions::apply(&repo, &self.store.diff_path(run_id))?;
        self.update_run(run_id, |r| r.diff_applique = Some(now));
        Ok(plan)
    }

    /// Supprime le worktree gardé d'une exécution ; son diff reste dans l'entrée.
    pub fn drop_worktree(&mut self, run_id: &str) -> Result<(), String> {
        let r = self.finished_run(run_id)?;
        let wt = r.worktree.clone().ok_or("cette exécution n'a pas de worktree")?;
        crate::claude::discard_worktree(&wt, r.depot.as_deref())?;
        self.update_run(run_id, |r| r.worktree = None);
        Ok(())
    }

    /// Le terminal qui reprend la session Claude d'une exécution, dans son dossier (worktree s'il est
    /// gardé, sinon le dépôt).
    pub fn resume_launch(&self, run_id: &str) -> Result<crate::actions::Launch, String> {
        let r = self.finished_run(run_id)?;
        let session = r.session_claude.as_ref().ok_or("pas de session Claude à reprendre")?;
        let dir = r
            .worktree
            .clone()
            .filter(|w| w.is_dir())
            .or_else(|| r.depot.clone())
            .ok_or("dossier de la session introuvable")?;
        let wt = crate::actions::windows_terminal();
        Ok(crate::actions::resume_command(&dir, session, &crate::claude::claude_exe(), wt.as_deref()))
    }

    /// Relève maintenant toutes les branches distantes (bouton « Vérifier maintenant », essais).
    pub fn poll_branches_now(&mut self) {
        self.branch_next.clear();
    }

    /// Lance les relevés de branches distantes dus (chacun dans un fil : réseau) et traite ceux qui
    /// sont revenus. Rend vrai si une tâche est partie.
    fn poll_branches(&mut self, now: NaiveDateTime) -> bool {
        let mut changed = false;
        while let Ok((key, task_id, label, poll)) = self.branch_rx.try_recv() {
            self.branch_busy.remove(&key);
            let prefix = format!("Tâche {task_id} : branche distante {label}");
            match poll {
                Poll::Failed(e) => self.problem(format!("{prefix} illisible : {e}")),
                Poll::Same => self.problems.retain(|p| !p.starts_with(&prefix)),
                Poll::First(sha) => {
                    self.problems.retain(|p| !p.starts_with(&prefix));
                    self.planning.branches.insert(key, sha);
                    self.save_planning();
                }
                Poll::Moved { from, to, log } => {
                    self.problems.retain(|p| !p.starts_with(&prefix));
                    self.planning.branches.insert(key, to.clone());
                    self.save_planning();
                    let short = |s: &str| s.chars().take(7).collect::<String>();
                    let mut notes = format!("Nouveaux commits sur {label} ({} → {}) :", short(&from), short(&to));
                    for c in &log {
                        notes.push_str(&format!("\n- {c}"));
                    }
                    let ctx = Context {
                        trigger: format!("{label} a avancé : {} → {}", short(&from), short(&to)),
                        files: vec![],
                        vars: vec![
                            ("PATRONUS_BRANCHE".into(), label.clone()),
                            ("PATRONUS_COMMIT_AVANT".into(), from),
                            ("PATRONUS_COMMIT_APRES".into(), to),
                        ],
                        notes,
                    };
                    self.launch(&task_id, ctx, now);
                    changed = true;
                }
            }
        }
        let due = Instant::now();
        let mut polls = Vec::new();
        for l in self.loaded.iter().filter(|l| l.problems.is_empty()) {
            let Ok(task) = &l.task else { continue };
            let Some(repo) = task.depot.clone().filter(|_| task.active) else { continue };
            for (i, tr) in task.declencheurs.iter().enumerate() {
                let Trigger::Branche { branche_distante: b } = tr else { continue };
                let key = format!("{}#{i}", l.id);
                if self.branch_busy.contains(&key) || self.branch_next.get(&key).is_some_and(|t| *t > due) {
                    continue;
                }
                let every = Duration::from_secs(b.toutes_les_minutes.max(1) as u64 * 60);
                polls.push((key, l.id.clone(), repo.clone(), b.distant.clone(), b.branche.clone(), every));
            }
        }
        for (key, task_id, repo, remote, branch, every) in polls {
            self.branch_next.insert(key.clone(), due + every);
            self.branch_busy.insert(key.clone());
            let known = self.planning.branches.get(&key).cloned();
            let tx = self.branch_tx.clone();
            std::thread::spawn(move || {
                let poll = crate::remote::poll(&repo, &remote, &branch, known.as_deref());
                let _ = tx.send((key, task_id, format!("{remote}/{branch}"), poll));
            });
        }
        changed
    }

    /// Change la rétention ; elle s'applique au prochain tour.
    pub fn set_retention(&mut self, days: u32) {
        self.retention_days = days;
        self.maintenance_due = true;
    }

    /// Rétention (avec les worktrees des entrées supprimées) et rapprochement des worktrees laissés
    /// par une exécution coupée. Rend vrai si la boîte a changé.
    fn maintenance(&mut self, now: NaiveDateTime) -> bool {
        let removed = self.store.prune(&mut self.runs, self.retention_days, now);
        for r in &removed {
            if let Some(wt) = &r.worktree
                && let Err(e) = crate::claude::discard_worktree(wt, r.depot.as_deref())
            {
                self.problem(format!("Worktree de {} non supprimé ({}) : {e}", r.id, wt.display()));
            }
        }
        self.reconcile_worktrees() || !removed.is_empty()
    }

    /// Chaque worktree de `<données>\worktrees` sans entrée qui le garde est rapproché de son
    /// exécution : diff enregistré s'il y a des changements (l'entrée garde le worktree), supprimé
    /// sinon. Un worktree dont l'exécution est inconnue reste en place et est signalé.
    fn reconcile_worktrees(&mut self) -> bool {
        let Ok(entries) = std::fs::read_dir(self.data_dir.join("worktrees")) else { return false };
        let mut changed = false;
        for path in entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
            let run_id = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let run = self.runs.iter().find(|r| r.id == run_id).cloned();
            // En cours (ici ou ailleurs), ou gardé à dessein avec son diff : on n'y touche pas.
            if run.as_ref().is_some_and(|r| r.statut == Status::EnCours || r.worktree.is_some()) {
                continue;
            }
            let info = crate::claude::worktree_info(&self.store.run_dir(&run_id));
            let (Some(mut run), Some(info)) = (run, info) else {
                self.problem(format!("Worktree sans exécution connue, laissé en place : {}", path.display()));
                continue;
            };
            match crate::claude::worktree_diff(&path, &info.base) {
                Ok(diff) if !diff.trim().is_empty() => {
                    let patch = self.store.diff_path(&run.id);
                    if !patch.exists() {
                        let _ = write_atomic(&patch, diff.as_bytes());
                    }
                    run.worktree = Some(path.clone());
                    run.resume = format!(
                        "{}\n\nDiff récupéré du worktree après l'interruption ({}).",
                        run.resume.trim(),
                        path.display()
                    );
                    run.lu = false;
                    let _ = self.store.save(&run);
                    if let Some(r) = self.runs.iter_mut().find(|r| r.id == run.id) {
                        *r = run;
                    }
                    changed = true;
                }
                Ok(_) => {
                    if let Err(e) = crate::claude::remove_worktree(&info.depot, &path) {
                        self.problem(format!("Worktree vide non supprimé ({}) : {e}", path.display()));
                    }
                }
                Err(e) => self.problem(format!("Worktree illisible ({}) : {e}", path.display())),
            }
        }
        changed
    }

    /// Ajoute un problème à montrer, sans doublon.
    pub fn problem(&mut self, text: String) {
        if !self.problems.contains(&text) {
            self.problems.push(text);
        }
    }

    /// Relit la boîte sur le disque : entrées écrites par un autre processus (ligne de commande,
    /// Planificateur), et exécutions qu'il mène. Une exécution « en cours » dont le processus a
    /// disparu devient « interrompue ». Rend vrai si quelque chose a changé.
    fn refresh_runs(&mut self) -> bool {
        let mut changed = false;
        let ours: Vec<String> = self.active.values().map(|a| a.run_id.clone()).collect();
        for id in self.store.ids() {
            if self.runs.iter().any(|r| r.id == id) {
                continue;
            }
            if let Some(run) = self.store.load_one(&id) {
                if run.statut != Status::EnCours {
                    self.finished.push(run.id.clone());
                }
                self.runs.push(run);
                changed = true;
            }
        }
        for r in self.runs.iter_mut().filter(|r| r.statut == Status::EnCours && !ours.contains(&r.id)) {
            let Some(mut fresh) = self.store.load_one(&r.id) else { continue };
            if fresh.statut == Status::EnCours && !fresh.proprietaire.is_some_and(|o| o.is_alive()) {
                fresh.statut = Status::Interrompue;
                fresh.resume = "Le processus qui menait l'exécution s'est arrêté.".into();
                let _ = self.store.save(&fresh);
            }
            if fresh != *r {
                if fresh.statut != Status::EnCours {
                    self.finished.push(fresh.id.clone());
                }
                *r = fresh;
                changed = true;
            }
        }
        if changed {
            self.runs.sort_by(|a, b| b.debut.cmp(&a.debut).then_with(|| b.id.cmp(&a.id)));
        }
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
                self.problem(format!("Enregistrement de l'exécution impossible : {e}"));
            }
        }
    }

    fn record(&mut self, run: Run) {
        if let Err(e) = self.store.save(&run) {
            self.problem(format!("Enregistrement de l'exécution impossible : {e}"));
        }
        self.finished.push(run.id.clone());
        self.runs.insert(0, run);
    }

    /// Ce qui devait partir pendant que Patronus était fermé : rattrapé si la tâche le demande,
    /// sinon signalé dans la boîte. Une tâche dont le fichier a été créé ou modifié pendant la
    /// fermeture n'a pas de départ manqué d'avant.
    fn report_missed(&mut self, now: NaiveDateTime) {
        if let Some(last) = self.planning.derniere_verification {
            let tasks: Vec<(String, Task, NaiveDateTime)> = self
                .loaded
                .iter()
                .filter(|l| l.problems.is_empty())
                .filter_map(|l| {
                    let task = l.task.as_ref().ok().filter(|t| t.active)?;
                    let from = file_time(&l.path).map_or(last, |m| m.max(last));
                    Some((l.id.clone(), task.clone(), from))
                })
                .collect();
            for (id, task, from) in tasks {
                let missed = missed(&task, from, now);
                self.handle_missed(&id, &task, &missed, now, "Patronus était fermé");
            }
        }
        self.planning.derniere_verification = Some(now);
        self.save_planning();
    }

    /// Départs manqués d'une tâche : le plus récent rattrapé si un déclencheur le demande, sinon une
    /// entrée « manquée » qui dit pourquoi (`why`).
    fn handle_missed(&mut self, id: &str, task: &Task, missed: &[Missed], now: NaiveDateTime, why: &str) {
        // Une échéance tenue ailleurs (Planificateur de Windows, ligne de commande) n'est pas manquée.
        let missed: Vec<Missed> = missed.iter().filter(|m| !self.already_done(id, m.at)).cloned().collect();
        if missed.is_empty() {
            return;
        }
        if let Some(m) = missed.iter().filter(|m| m.catch_up).max_by_key(|m| m.at) {
            let trigger = format!("rattrapage de {}", m.at.format("%d/%m %H:%M"));
            self.launch(id, Context { trigger, files: vec![], ..Default::default() }, now);
            return;
        }
        let latest = missed.iter().map(|m| m.at).max().unwrap_or(now);
        let count: usize = missed.iter().map(|m| m.count).sum();
        let mut run = Run::new(
            self.store.new_id(id, now),
            id,
            &task.nom,
            &format!("échéance de {}", latest.format("%d/%m %H:%M")),
            latest,
        );
        run.statut = Status::Manquee;
        run.fin = Some(latest);
        let at_least = if missed.iter().any(|m| m.count >= MISSED_COUNT_CAP) { "au moins " } else { "" };
        run.resume = if count > 1 {
            format!("{at_least}{count} départs manqués ({why}), le dernier le {}.", latest.format("%d/%m à %H:%M"))
        } else {
            format!("Départ du {} manqué : {why}.", latest.format("%d/%m à %H:%M"))
        };
        self.record(run);
    }

    fn heartbeat(&mut self, now: NaiveDateTime) {
        // En valeur absolue : au passage à l'heure d'hiver, l'heure recule d'une heure.
        let stale = self.planning.derniere_verification.is_none_or(|t| (now - t).num_seconds().abs() >= 30);
        if stale {
            self.planning.derniere_verification = Some(now);
            self.save_planning();
        }
    }

    fn save_planning(&mut self) {
        let json = serde_json::to_vec_pretty(&self.planning).unwrap_or_default();
        if let Err(e) = write_atomic(&self.data_dir.join("planning.json"), &json) {
            self.problem(format!("Enregistrement de planning.json impossible : {e}"));
        }
    }

    /// À la fermeture : arrête ce qui tourne, attend un peu, et note l'heure.
    pub fn shutdown(&mut self, now: NaiveDateTime, wait: Duration) {
        self.shutting_down = true;
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
        t.action.script = Some(Script {
            commande: cmd.into(),
            shell: Shell::Cmd,
            dossier: None,
            env: Default::default(),
            ..Default::default()
        });
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
        e.launch(&id, Context { trigger: "à la main".into(), files: vec![], ..Default::default() }, now);
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
            e.launch(&id, Context { trigger: format!("n{i}"), files: vec![], ..Default::default() }, now);
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
        e.launch(&id, Context { trigger: "à la main".into(), files: vec![], ..Default::default() }, now);
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
        e.launch(&id, Context { trigger: "à la main".into(), files: vec![], ..Default::default() }, now);
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
        backdate(&dir.join("taches"), at(20, 17, 0));
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
        let fw = FileWatch {
            dossier: watched.clone(),
            inclure: vec!["docs/**".into()],
            exclure: vec![],
            ..Default::default()
        };
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

    /// Fait tourner le moteur pendant `d`.
    fn spin(e: &mut Engine, now: NaiveDateTime, d: Duration) {
        let start = Instant::now();
        while start.elapsed() < d {
            e.tick(now);
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Une tâche qui écrit dans le dossier qu'elle surveille : ses propres écritures, même
    /// signalées après sa fin, ne la relancent pas ; une modification faite ensuite la relance.
    #[test]
    fn own_write_at_the_end_does_not_retrigger_but_a_later_change_does() {
        let dir = test_dir("engine-own-write");
        let watched = dir.join("fabrikam");
        std::fs::create_dir_all(&watched).unwrap();
        let now = at(21, 10, 0);
        let mut e = Engine::open_with(&dir.join("data"), now, None, Duration::from_millis(200));
        let mut t = task("echo genere> sortie.txt", vec![]);
        t.action.script.as_mut().unwrap().dossier = Some(watched.clone());
        t.declencheurs =
            vec![Trigger::Fichiers { fichiers: FileWatch { dossier: watched.clone(), ..Default::default() } }];
        let id = e.save_task(None, &t, now).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        e.launch(&id, Context { trigger: "à la main".into(), files: vec![], ..Default::default() }, now);
        wait_idle(&mut e, now);
        spin(&mut e, now, Duration::from_millis(1500));
        assert_eq!(
            e.runs.len(),
            1,
            "relancée par ses propres écritures : {:?}",
            e.runs.iter().map(|r| &r.declencheur).collect::<Vec<_>>()
        );
        assert!(e.store.read_journal(&e.runs[0].id).contains("attribué"), "{}", e.store.read_journal(&e.runs[0].id));
        std::fs::write(watched.join("note.md"), "x").unwrap();
        let start = Instant::now();
        while e.runs.len() < 2 && start.elapsed() < Duration::from_secs(5) {
            e.tick(now);
            std::thread::sleep(Duration::from_millis(20));
        }
        wait_idle(&mut e, now);
        assert_eq!(e.runs.len(), 2);
        assert!(e.runs[0].declencheur.contains("note.md"), "{}", e.runs[0].declencheur);
        spin(&mut e, now, Duration::from_millis(1500));
        assert_eq!(
            e.runs.len(),
            2,
            "la seconde exécution ne se relance pas non plus : {:?}",
            e.runs.iter().map(|r| &r.declencheur).collect::<Vec<_>>()
        );
    }

    /// Le fil du moteur fait partir les échéances et termine les exécutions sans aucune interface.
    #[test]
    fn ticker_runs_due_tasks_without_ui() {
        let dir = test_dir("engine-ticker");
        let (exec, counter) = fake(Duration::ZERO);
        let e = Engine::open_with(&dir, at(21, 7, 58), Some(exec), Duration::from_millis(200));
        let shared: Shared = Arc::new(Mutex::new(e));
        lock(&shared)
            .save_task(
                Some("matin"),
                &task("x", vec![Trigger::Heure { heure: "08:00".into(), jours: vec![], rattrapage: false }]),
                at(21, 7, 58),
            )
            .unwrap();
        let ticks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = ticks.clone();
        let ticker = Ticker::start(
            shared.clone(),
            Duration::from_millis(20),
            || at(21, 8, 0),
            move |_, _| {
                seen.fetch_add(1, Ordering::Relaxed);
            },
        );
        let start = Instant::now();
        while lock(&shared).runs.first().is_none_or(|r| r.statut == Status::EnCours) {
            assert!(start.elapsed() < Duration::from_secs(5), "échéance non tenue");
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(ticker);
        assert_eq!(counter.all(), ["échéance de 08:00"]);
        assert!(ticks.load(Ordering::Relaxed) > 0);
    }

    /// Une fois la fermeture demandée, ni échéance, ni fichier, ni relance en attente ne fait partir
    /// une exécution.
    #[test]
    fn shutdown_starts_nothing() {
        let dir = test_dir("engine-shutdown");
        let (exec, counter) = fake(Duration::from_millis(300));
        let now = at(21, 10, 0);
        let mut e = Engine::open_with(&dir, now, Some(exec), Duration::from_millis(200));
        let id = e.save_task(None, &task("x", vec![Trigger::Intervalle { toutes_les_minutes: 1 }]), now).unwrap();
        e.launch(&id, Context { trigger: "à la main".into(), files: vec![], ..Default::default() }, now);
        e.launch(&id, Context { trigger: "en attente".into(), files: vec![], ..Default::default() }, now);
        e.shutdown(at(21, 10, 5), Duration::from_secs(3));
        assert_eq!(counter.all(), ["à la main"], "ni la relance en attente ni l'échéance");
        e.launch(&id, Context { trigger: "après".into(), files: vec![], ..Default::default() }, now);
        assert_eq!(e.running_count(), 0);
    }

    /// Un autre processus vivant (lancé par le test), pour jouer la fenêtre ou la ligne de commande.
    fn other_process() -> (std::process::Child, Owner) {
        use std::os::windows::process::CommandExt;
        let child = std::process::Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .creation_flags(0x0800_0000)
            .spawn()
            .unwrap();
        let pid = child.id();
        let cree = crate::winproc::created(pid).unwrap();
        (child, Owner { pid, cree })
    }

    /// L'ouverture ne déclare pas « interrompue » une exécution qu'un autre processus vivant mène ;
    /// elle le fait quand ce processus a disparu.
    #[test]
    fn opening_does_not_interrupt_a_run_led_by_a_live_process() {
        let dir = test_dir("engine-foreign");
        let store = RunStore::new(dir.join("executions"));
        let (mut child, owner) = other_process();
        let mut run = Run::new("r1".into(), "revue", "Revue", "ligne de commande", at(21, 10, 0));
        run.proprietaire = Some(owner);
        store.save(&run).unwrap();
        let e = Engine::open(&dir, at(21, 10, 1));
        assert_eq!(e.runs[0].statut, Status::EnCours, "menée par un autre processus vivant");
        drop(e);
        child.kill().unwrap();
        child.wait().unwrap();
        let e = Engine::open(&dir, at(21, 10, 2));
        assert_eq!(e.runs[0].statut, Status::Interrompue);
    }

    /// Sans fenêtre : ni « manquées », ni rattrapage, ni écriture de `planning.json`, ni départ dû.
    #[test]
    fn headless_engine_only_runs_what_it_is_asked() {
        let dir = test_dir("engine-headless");
        let mut e = Engine::open(&dir, at(20, 18, 0));
        e.save_task(
            Some("matin"),
            &task("echo x", vec![Trigger::Heure { heure: "08:00".into(), jours: vec![], rattrapage: true }]),
            at(20, 18, 0),
        )
        .unwrap();
        e.shutdown(at(20, 18, 0), Duration::from_secs(1));
        drop(e);
        let planning = std::fs::read_to_string(dir.join("planning.json")).unwrap();
        let mut h = Engine::open_headless(&dir, at(21, 10, 0));
        h.tick(at(21, 10, 0));
        assert!(h.runs.is_empty(), "ni manquée ni rattrapage : {:?}", h.runs);
        assert_eq!(std::fs::read_to_string(dir.join("planning.json")).unwrap(), planning);
        h.try_launch(
            "matin",
            Context { trigger: "ligne de commande".into(), files: vec![], ..Default::default() },
            at(21, 10, 0),
        )
        .unwrap();
        wait_idle(&mut h, at(21, 10, 0));
        assert_eq!(h.runs.len(), 1);
        assert_eq!(h.runs[0].statut, Status::Reussie);
    }

    /// Une tâche qui tourne dans un autre processus : la ligne de commande refuse, la fenêtre met le
    /// départ en attente et le lance quand le verrou se libère.
    #[test]
    fn a_task_running_elsewhere_is_refused_or_queued() {
        let dir = test_dir("engine-elsewhere");
        let (exec, counter) = fake(Duration::ZERO);
        let opts = Options { executor: Some(exec), refresh_every: Duration::from_millis(50), ..Options::default() };
        let mut e = Engine::open_opts(&dir, at(21, 10, 0), opts);
        e.save_task(Some("revue"), &task("x", vec![]), at(21, 10, 0)).unwrap();
        let (mut child, owner) = other_process();
        std::fs::create_dir_all(dir.join("verrous")).unwrap();
        std::fs::write(dir.join("verrous").join("revue.lock"), serde_json::to_string(&owner).unwrap()).unwrap();
        assert_eq!(e.running_elsewhere("revue"), Some(owner));
        let err = e.try_launch(
            "revue",
            Context { trigger: "ligne de commande".into(), files: vec![], ..Default::default() },
            at(21, 10, 0),
        );
        assert!(err.unwrap_err().contains("autre processus"));
        e.launch("revue", Context { trigger: "à la main".into(), files: vec![], ..Default::default() }, at(21, 10, 0));
        assert!(e.is_pending("revue"));
        spin(&mut e, at(21, 10, 0), Duration::from_millis(200));
        assert!(counter.all().is_empty(), "le verrou est tenu");
        child.kill().unwrap();
        child.wait().unwrap();
        let start = Instant::now();
        while counter.all().is_empty() && start.elapsed() < Duration::from_secs(5) {
            e.tick(at(21, 10, 0));
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(counter.all(), ["à la main"], "lancée dès que l'autre processus a disparu");
    }

    /// La fenêtre voit les résultats écrits par un autre moteur (la ligne de commande) sur le même
    /// dossier, et les annonce.
    #[test]
    fn window_sees_results_written_elsewhere() {
        let dir = test_dir("engine-refresh");
        let opts = Options { refresh_every: Duration::from_millis(50), ..Options::default() };
        let mut window = Engine::open_opts(&dir, at(21, 10, 0), opts);
        window.save_task(Some("bonjour"), &task("echo bonjour", vec![]), at(21, 10, 0)).unwrap();
        let mut cli = Engine::open_headless(&dir, at(21, 10, 0));
        cli.try_launch(
            "bonjour",
            Context { trigger: "ligne de commande".into(), files: vec![], ..Default::default() },
            at(21, 10, 0),
        )
        .unwrap();
        wait_idle(&mut cli, at(21, 10, 0));
        assert!(window.runs.is_empty());
        spin(&mut window, at(21, 10, 0), Duration::from_millis(200));
        assert_eq!(window.runs.len(), 1);
        assert_eq!(window.runs[0].statut, Status::Reussie);
        assert_eq!(window.take_finished().len(), 1, "annoncée comme les autres");
    }

    /// Un même problème n'apparaît qu'une fois, même après plusieurs rechargements ; il disparaît quand
    /// sa cause disparaît.
    #[test]
    fn problems_are_not_duplicated() {
        let dir = test_dir("engine-problems");
        let now = at(21, 10, 0);
        let mut e = Engine::open(&dir.join("data"), now);
        let fw = FileWatch { dossier: dir.join("absent"), ..Default::default() };
        let t = task("x", vec![Trigger::Fichiers { fichiers: fw }]);
        e.save_task(Some("doc"), &t, now).unwrap();
        e.save_task(Some("doc"), &t, now).unwrap();
        e.reload_tasks(now);
        assert_eq!(e.problems.iter().filter(|p| p.contains("introuvable")).count(), 1, "{:?}", e.problems);
        std::fs::create_dir_all(dir.join("absent")).unwrap();
        e.reload_tasks(now);
        assert!(!e.problems.iter().any(|p| p.contains("introuvable")), "{:?}", e.problems);
    }

    /// Un dossier surveillé créé après le chargement de la tâche finit par être surveillé.
    #[test]
    fn watched_folder_created_later_is_watched() {
        let dir = test_dir("engine-later");
        let watched = dir.join("northwind-docs");
        let (exec, counter) = fake(Duration::ZERO);
        let opts = Options {
            executor: Some(exec),
            debounce: Duration::from_millis(200),
            refresh_every: Duration::from_millis(50),
            ..Options::default()
        };
        let now = at(21, 10, 0);
        let mut e = Engine::open_opts(&dir.join("data"), now, opts);
        let fw = FileWatch { dossier: watched.clone(), ..Default::default() };
        e.save_task(None, &task("x", vec![Trigger::Fichiers { fichiers: fw }]), now).unwrap();
        std::fs::create_dir_all(&watched).unwrap();
        spin(&mut e, now, Duration::from_millis(300));
        std::fs::write(watched.join("guide.md"), "x").unwrap();
        let start = Instant::now();
        while counter.all().is_empty() && start.elapsed() < Duration::from_secs(5) {
            e.tick(now);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(counter.all().len(), 1, "{:?}", e.problems);
    }

    /// Donne aux fichiers des tâches la date de modification `t` (ces tests vivent dans un temps
    /// fictif, les fichiers dans le vrai).
    fn backdate(dir: &Path, t: NaiveDateTime) {
        use chrono::TimeZone;
        let when: std::time::SystemTime = chrono::Local.from_local_datetime(&t).unwrap().into();
        for f in std::fs::read_dir(dir).unwrap().flatten() {
            std::fs::File::options().write(true).open(f.path()).unwrap().set_modified(when).unwrap();
        }
    }

    /// Après une veille, une échéance tenue avec plus de 5 minutes de retard est « manquée », sauf si
    /// la tâche demande le rattrapage.
    #[test]
    fn late_after_sleep_is_missed_unless_caught_up() {
        let dir = test_dir("engine-veille");
        let (exec, counter) = fake(Duration::ZERO);
        let mut e = Engine::open_with(&dir, at(21, 7, 0), Some(exec), Duration::from_millis(200));
        let heure = |rattrapage| vec![Trigger::Heure { heure: "08:00".into(), jours: vec![], rattrapage }];
        e.save_task(Some("signale"), &task("x", heure(false)), at(21, 7, 0)).unwrap();
        e.save_task(Some("rattrape"), &task("x", heure(true)), at(21, 7, 0)).unwrap();
        // Réveil à 8 h 20 : le premier tour après la veille.
        e.tick(at(21, 8, 20));
        wait_idle(&mut e, at(21, 8, 20));
        assert_eq!(counter.all(), ["rattrapage de 21/10 08:00"]);
        let m = e.runs.iter().find(|r| r.tache == "signale").unwrap();
        assert_eq!(m.statut, Status::Manquee);
        assert!(m.resume.contains("en veille"), "{}", m.resume);
        assert_eq!(e.next_run("signale"), Some(at(22, 8, 0)));
        // Un petit retard (moins de 5 minutes) part normalement.
        e.save_task(Some("minute"), &task("x", vec![Trigger::Intervalle { toutes_les_minutes: 60 }]), at(21, 8, 20))
            .unwrap();
        e.tick(at(21, 9, 3));
        wait_idle(&mut e, at(21, 9, 3));
        assert!(counter.all().contains(&"échéance de 09:00".to_string()), "{:?}", counter.all());
    }

    /// Une tâche créée pendant la fermeture n'a pas de départ manqué d'avant sa création.
    #[test]
    fn new_task_has_no_old_missed() {
        let dir = test_dir("engine-nouvelle");
        let e = Engine::open(&dir, at(18, 18, 0));
        drop(e);
        // Fichier écrit à la main pendant la fermeture : sa date est maintenant (la vraie).
        let store = TaskStore::new(dir.join("taches"));
        store.save("nouvelle", &task("x", vec![Trigger::Intervalle { toutes_les_minutes: 60 }])).unwrap();
        backdate(&dir.join("taches"), at(21, 9, 30));
        let e = Engine::open(&dir, at(21, 10, 0));
        let m: Vec<&Run> = e.runs.iter().filter(|r| r.statut == Status::Manquee).collect();
        assert_eq!(m.len(), 1, "seulement depuis sa création : {:?}", m.iter().map(|r| &r.resume).collect::<Vec<_>>());
        assert!(m[0].resume.contains("21/10 à 10:00"), "{}", m[0].resume);
    }

    /// Au passage à l'heure d'hiver l'heure recule : la surveillance des échéances continue d'être
    /// notée.
    #[test]
    fn clock_change_keeps_the_heartbeat() {
        let dir = test_dir("engine-heure");
        let mut e = Engine::open(&dir, at(25, 2, 59));
        e.tick(at(25, 2, 1));
        let p: Planning = serde_json::from_str(&std::fs::read_to_string(dir.join("planning.json")).unwrap()).unwrap();
        assert_eq!(p.derniere_verification, Some(at(25, 2, 1)));
    }

    /// Une échéance tenue par le Planificateur de Windows pendant que la fenêtre était fermée n'est pas
    /// signalée « manquée » à son ouverture.
    #[test]
    fn missed_is_not_reported_when_done_elsewhere() {
        let dir = test_dir("engine-tenue");
        let mut e = Engine::open(&dir, at(20, 18, 0));
        let heure = vec![Trigger::Heure { heure: "08:00".into(), jours: vec![], rattrapage: false }];
        e.save_task(Some("matin"), &task("x", heure), at(20, 18, 0)).unwrap();
        e.shutdown(at(20, 18, 0), Duration::from_secs(1));
        drop(e);
        backdate(&dir.join("taches"), at(20, 17, 0));
        // Exécution menée par le Planificateur à 8 h, fenêtre fermée.
        let mut run = Run::new("20261021-080001-matin".into(), "matin", "x", "Planificateur de Windows", at(21, 8, 0));
        run.statut = Status::Reussie;
        RunStore::new(dir.join("executions")).save(&run).unwrap();
        let e = Engine::open(&dir, at(21, 10, 0));
        assert!(
            e.runs.iter().all(|r| r.statut != Status::Manquee),
            "{:?}",
            e.runs.iter().map(|r| &r.resume).collect::<Vec<_>>()
        );
    }

    /// `planifie` : la dernière échéance à tenir, ou pourquoi il n'y a rien à faire.
    #[test]
    fn planned_run_decision() {
        let dir = test_dir("engine-planifie");
        let mut e = Engine::open_headless(&dir, at(21, 8, 0));
        let mut t = task("echo x", vec![Trigger::Heure { heure: "08:00".into(), jours: vec![], rattrapage: false }]);
        e.save_task(Some("matin"), &t, at(21, 8, 0)).unwrap();
        assert!(e.planned_run("matin", at(21, 8, 0)).unwrap_err().contains("application fermée"));
        t.hors_application = true;
        e.save_task(Some("matin"), &t, at(21, 8, 0)).unwrap();
        assert_eq!(e.planned_run("matin", at(21, 8, 0)), Ok(at(21, 8, 0)));
        // Lancé quelques secondes avant l'heure : c'est l'échéance de 8 h.
        assert_eq!(e.planned_run("matin", at(21, 7, 59)), Ok(at(21, 8, 0)));
        let mut run = Run::new("r".into(), "matin", "x", "à la main", at(21, 8, 0));
        run.statut = Status::Reussie;
        e.runs.push(run);
        assert!(e.planned_run("matin", at(21, 8, 1)).unwrap_err().contains("déjà tenue"));
        assert!(e.planned_run("inconnue", at(21, 8, 0)).is_err());
    }
}
