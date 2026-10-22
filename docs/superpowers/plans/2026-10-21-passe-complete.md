# Passe complète et fonctionnalités — plan d'implémentation

> **Pour les agents d'exécution :** SOUS-SKILL REQUIS : superpowers:subagent-driven-development (recommandé) ou
> superpowers:executing-plans, tâche par tâche. Les étapes utilisent des cases (`- [ ]`) pour le suivi.

**Objectif :** corriger ce que la relecture du premier jet a trouvé (A), traiter les points remontés B1 à B4, puis
ajouter les fonctionnalités C1 à C6 de la feuille de route.

**Architecture :** workspace de deux crates. `patronus-core` porte toute la logique, testée sans interface : format
des tâches (`task.rs`), échéances (`schedule.rs`), fichiers (`watch.rs`), processus (`exec.rs`), boîte de résultats
(`run.rs`), moteur (`engine.rs`), session Claude (`claude.rs`, `pricing.rs`), branche distante (`remote.rs`),
Planificateur de Windows (`winsched.rs`), modèles (`templates.rs`). `patronus-app` est l'interface egui
(`app.rs` et ses vues), avec un exécutable fin (`main.rs`). Les tests d'interface pilotent l'application entière avec
egui_kittest, dans un dossier de données temporaire.

**Pile :** Rust 2024, eframe/egui 0.37 (glow, accesskit), windows-sys 0.61, serde_json, chrono, globset, notify,
egui_kittest 0.37 (tests).

**Spécification :** `docs/feuille-de-route.md` (sections A, B, C : « À faire » et « Critère de fin » font autorité) ;
vision dans `docs/spec.md` ; décisions dans `docs/journal.md`.

## Contraintes globales

1. **Un commit par tâche**, message en français `<ID> — <Verbe au présent> …` (ex. `B2 — Déclenche une tâche quand
   une branche distante avance`), puis exactement ces deux lignes :
   ```
   Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
   Claude-Session: https://claude.ai/code/session_01MgyH2TaDL6S3Ssp2wpR4RU
   ```
   Identité git du dépôt (`AubinCheD`). Jamais de `push`, de `--amend`, de `rebase` ni de `fixup!` ; une correction
   après revue est un commit ordinaire `<ID> — Corrige … (revue)`.
2. **Avant chaque commit :** `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test` — tout vert.
3. **Feuille de route dans le même commit :** le point traité passe à « fait » dans le sommaire.
4. **Rien de lent dans le fil de l'interface :** ni réseau, ni processus attendu (git, schtasks, claude), ni lecture
   de fichier à chaque image.
5. **Confidentialité :** noms fictifs (contoso, fabrikam, northwind), aucun chemin absolu de la machine ni nom réel
   dans les fichiers versionnés.
6. **Aucun vrai `claude -p`** dans les tests ni les essais : le faux exécutable `faux-claude` (`PATRONUS_CLAUDE`).
7. **Essais :** dossier de données temporaire (`PATRONUS_DATA_DIR` ou `--donnees`) ; captures par `PATRONUS_CAPTURE`
   seulement ; le Planificateur de Windows n'est modifié que dans un dossier d'essai, nettoyé ensuite.
8. **Ancien état toujours lisible :** tout nouveau champ de tâche, d'exécution, de `planning.json` ou de `state.json`
   a une valeur par défaut (serde) ; les fichiers du premier jet sont relus sans perte (test).
9. **Interface en français**, raccourcis réglables, rien de relancé ni de perdu en silence.

---

### Tâche 0 : Outillage et documents

**Fichiers :** `.claude/settings.json` (greffon superpowers, comme Fawkes), `docs/feuille-de-route.md`, ce plan ;
tout le code par `cargo fmt` ; avertissements de clippy corrigés.

- [x] `cargo fmt` puis `cargo clippy --all-targets -- -D warnings` : aucun avertissement.
- [x] `cargo test` : tout vert.
- [x] Commit `Outillage — Ajoute superpowers, la feuille de route et le plan ; formate le code`.

### Tâche A1 : Boutons des lignes de tâches

**Fichiers :** `app.rs` (`task_list`), `Cargo.toml` de l'application (`egui_kittest` en dépendance de test,
fonctionnalité `accesskit` d'eframe), `tests/common/mod.rs`, créer `tests/ui_tasks.rs`.

**Interfaces :** `App::with(cc, Options { data_dir, state, problems, quiet })` (`quiet` : ni notification ni
Planificateur ni registre, pour les tests) ; `App::engine()` pour les assertions.

- [x] Tests `run_button_launches_the_task`, `checkbox_pauses_the_task` : échouent avant la correction.
- [x] Ligne : `ui.scope_builder(UiBuilder::new().sense(Sense::click()), …)` autour du cadre.
- [x] Commit `A1 — Rend leurs clics aux boutons des lignes de tâches et teste l'interface par de vrais clics`.

### Tâche A2 : Plus de boucle sur ses propres écritures

**Fichiers :** `engine.rs`.

- [x] `Engine::last_window: HashMap<String, (SystemTime, Option<SystemTime>)>` (début, fin) par tâche ; un fichier
  dont la date de modification est dans `[début, fin + 1 s]` (ou absent, si l'événement arrive dans `debounce + 1 s`
  après la fin) est ignoré et noté dans le journal de l'exécution.
- [x] Tests `own_write_at_the_end_does_not_retrigger`, `later_change_retriggers_once`.
- [x] Commit `A2 — Ne relance plus une tâche sur ses propres écritures`.

### Tâche A3 : Moteur dans son fil

**Fichiers :** `engine.rs` (`Shared = Arc<Mutex<Engine>>`, `spawn_ticker`, `generation`, `shutting_down`), `app.rs`
(instantané, actions sous verrou), `main.rs`.

- [x] Tests `ticker_runs_due_tasks_without_ui`, `shutdown_starts_nothing`.
- [x] Commit `A3 — Fait tourner le moteur dans son propre fil`.

### Tâche A4 : Lancement de `claude`

**Fichiers :** `claude.rs`, `exec.rs` (`run_process_with` : entrée standard depuis un fichier), `faux-claude.rs`
(lit l'entrée standard), `tests/claude.rs`.

- [x] Tests `many_changed_files_are_capped`, `prompt_starting_with_a_dash`, `cmd_shim_is_found`.
- [x] Commit `A4 — Passe la consigne par l'entrée standard et trouve claude.exe ou claude.cmd`.

### Tâche A5 : Tours et budget

**Fichiers :** `claude.rs` (`StreamReader` incrémental, estimation par `pricing.rs`), `run.rs` (`Status::ToursAtteints`,
`Cost::estime`), `faux-claude.rs`, `tests/claude.rs`.

- [x] Tests `max_turns_from_claude_code`, `estimated_budget_stops_the_session`, `budget_from_claude_code`,
  `no_result_is_a_failure`, `denials_are_listed`, `stopped_session_keeps_its_estimated_cost`.
- [x] Commit `A5 — Suit les tours et le budget comme le vrai Claude Code`.

### Tâche A6 : Isolation

**Fichiers :** `task.rs` (validation), `claude.rs` (arguments, environnement), `app.rs` (case, avertissement).

- [x] Tests `write_tools_need_a_worktree`, `unknown_permission_mode_is_refused`, `restricted_args_and_push_guard`.
- [x] Commit `A6 — Isole les sessions Claude : worktree pour écrire, mode restreint, push impossible`.

### Tâche A7 : Objet de tâche

**Fichiers :** `exec.rs`, `Cargo.toml` du cœur (`windows-sys` : `Win32_System_JobObjects`, `Win32_System_Threading`,
`Win32_Security`).

- [x] Test `grandchild_dies_with_the_stop`.
- [x] Commit `A7 — Range chaque processus lancé dans un objet de tâche Windows`.

### Tâche A8 : Plusieurs processus

**Fichiers :** `run.rs` (`Owner { pid, cree }`), `engine.rs` (`Mode::Fenetre | Mode::SansFenetre`, relecture,
verrou inter-processus), `main.rs`.

- [x] Tests `cli_does_not_interrupt_a_live_run`, `cli_reports_nothing_missed`, `window_sees_cli_results`,
  `second_process_refuses_a_running_task`.
- [x] Commit `A8 — Partage proprement le dossier de données entre la fenêtre et la ligne de commande`.

### Tâche A9 : Rétention et worktrees

**Fichiers :** `engine.rs`, `run.rs`, `claude.rs`.

- [x] Tests `retention_setting_is_applied`, `pruned_entry_removes_its_worktree`, `orphan_worktree_is_reconciled`.
- [x] Commit `A9 — Applique la rétention réglée et nettoie les worktrees`.

### Tâche A10 : Interface sobre

**Fichiers :** `app.rs`.

- [x] Tests `idle_window_does_not_repaint_continuously`, `draft_survives_switching_to_the_inbox`,
  `abandoning_a_draft_asks_first`.
- [x] Commit `A10 — Allège le dessin et protège le brouillon de l'éditeur`.

### Tâche A11 : Chemins, cmd, problèmes

**Fichiers :** `watch.rs`, `exec.rs`, `engine.rs`.

- [x] Tests `accented_paths_do_not_panic`, `cmd_output_is_utf8`, `problems_are_not_duplicated`.
- [x] Commit `A11 — Compare les chemins par composants, lit cmd en UTF-8 et dédoublonne les problèmes`.

### Tâche A12 : Formats

**Fichiers :** `task.rs`, `run.rs`, `engine.rs`, `tests/data/` (états de référence).

- [x] Tests `nested_unknown_fields_are_kept`, `reference_task_v1`, `reference_run_v1`, `reference_planning_v1`,
  `deleting_twice_keeps_both`, `malformed_trigger_is_named`, `powershell_variable_is_not_a_secret`.
- [x] Commit `A12 — Garde les champs inconnus à tous les niveaux et fige les formats`.

### Tâche A13 : Échéances

**Fichiers :** `schedule.rs`, `engine.rs`.

- [x] Tests `late_after_sleep_is_missed`, `latest_due_without_cap`, `new_task_has_no_old_missed`,
  `clock_change_keeps_the_heartbeat`.
- [x] Commit `A13 — Traite la veille, les longues absences et le changement d'heure`.

### Tâche B1 : Symboles de l'interface

**Fichiers :** créer `crates/patronus-app/src/icons.rs` ; modifier `app.rs` (plus aucun symbole écrit en dur).

- [x] `pub const` par symbole (`RUN`, `STOP`, `EDIT`, `REMOVE`, `WARN`, `ADD`…) et `pub const ALL: &[&str]`.
- [x] Test `every_symbol_has_a_glyph` : contexte egui avec les polices de l'application (`install_fonts`), chaque
  symbole de `ALL` a un glyphe dans la famille proportionnelle.
- [x] Capture au zoom 200 % (après C6) de la liste des tâches et de la boîte ; résultat dans `journal.md`.
- [x] Commit `B1 — Rassemble les symboles et vérifie qu'ils ont tous un glyphe`.

### Tâche B2 : Branche distante

**Fichiers :** `task.rs` (`Trigger::Branche { branche_distante: RemoteBranch }`, `RemoteBranch { branche, distant,
toutes_les_minutes }`), créer `remote.rs`, `engine.rs`, `exec.rs` (`Context::vars`, `Context::notes`), `claude.rs`
(notes ajoutées à la consigne), `schedule.rs` (`describe`), `app.rs` (éditeur).

**Interfaces :**
- `remote::head(repo, remote, branch) -> Result<String, String>` (`git ls-remote`, 60 s au plus, sans demande
  d'identifiants) ; `remote::fetch(repo, remote, branch) -> Result<(), String>` ; `remote::log(repo, from, to, max)
  -> Vec<String>`.
- `Planning::branches: BTreeMap<String, String>` (clé `<tâche>#<n° du déclencheur>`).
- `Engine::poll_branches_now()` (tests, et bouton *Vérifier maintenant*).

- [x] Tests (`tests/remote.rs`, dépôt nu « distant » et clone temporaires) : `first_read_does_not_trigger`,
  `pushed_commit_triggers_with_the_log`, `working_copy_is_untouched`, `unknown_remote_is_reported_once`.
- [x] Commit `B2 — Déclenche une tâche quand une branche distante avance`.

### Tâche B3 : Planificateur de Windows

**Fichiers :** `task.rs` (`hors_application`), `winsched.rs` (déjà écrit : XML, lecture de `schtasks /Query`,
synchronisation), `engine.rs` (échéance tenue par un autre processus non « manquée »), `main.rs` (`--donnees`,
`planifie <tâche>`), `app.rs` (case dans l'éditeur, synchronisation dans un fil).

**Interfaces :**
- `schedule::latest_due(task, now) -> Option<NaiveDateTime>` : dernière échéance horaire au plus tard à `now`.
- `Engine::already_done(task_id, at) -> bool` : une exécution de la tâche a commencé à `at` ou après (1 min de
  tolérance).

- [x] Tests : XML (déjà), `parse_list` (déjà), `planifie_skips_when_done`, `missed_is_not_reported_when_done`,
  inscription réelle ignorée par défaut (lancée une fois).
- [x] Essai réel (dossier d'essai, tâche à deux minutes, fenêtre fermée), noté dans `journal.md`, puis nettoyé.
- [x] Commit `B3 — Inscrit au Planificateur de Windows les tâches qui tournent application fermée`.

### Tâche B4 : Essai réel préparé

**Fichiers :** `README.md` (section *Premier essai réel*), modèle *Essai Claude (lecture seule)* (C2).

- [x] Commit avec C2.

### Tâche C1 : Actions sur un résultat

**Fichiers :** créer `crates/patronus-core/src/actions.rs` ; `run.rs` (`depot`, `diff_applique`), `engine.rs`
(rétention : worktrees supprimés), `app.rs` (boutons, confirmations).

**Interfaces :**
- `actions::check_apply(repo, patch) -> Result<Vec<String>, String>` (fichiers touchés) ; `actions::apply(repo,
  patch)` ; `actions::remove_worktree(wt)` (dépôt principal retrouvé par `git rev-parse --git-common-dir`) ;
  `actions::resume_command(dir, session, claude) -> (PathBuf, Vec<String>)` (Windows Terminal, sinon PowerShell).

- [x] Tests (dépôt temporaire) : `diff_is_applied_to_the_working_copy_only`, `diverged_copy_is_refused`,
  `worktree_is_removed`, `prune_removes_worktrees`, `resume_command_uses_windows_terminal_when_present`.
- [x] Test d'interface `apply_diff_asks_then_applies`.
- [x] Commit `C1 — Applique le diff, supprime le worktree et reprend la session d'un résultat`.

### Tâche C2 : Modèles

**Fichiers :** `templates.rs` (déjà écrit), `app.rs` (menu de *Nouvelle tâche*).

- [x] Tests : `every_template_is_valid_once_a_repo_is_chosen` ; interface `task_from_template`.
- [x] Commit `C2 — Propose des modèles de tâches`.

### Tâche C3 : Budget mensuel

**Fichiers :** `engine.rs` (`Planning::couts_mensuels`, refus), `state.rs` (`budget_mensuel_usd`), `app.rs`.

- [x] Tests : `monthly_cost_accumulates`, `next_month_starts_at_zero`, `session_over_monthly_budget_is_refused`.
- [x] Commit `C3 — Ajoute un budget mensuel pour les sessions Claude`.

### Tâche C4 : Démarrage et fermeture

**Fichiers :** créer `crates/patronus-app/src/autostart.rs` (clé `Run`, comme Fawkes) ; `main.rs` (`--reduit`) ;
`app.rs` (dialogue de fermeture).

- [x] Tests : registre sur une clé d'essai ; interface `closing_with_a_run_asks_first`.
- [x] Commit `C4 — Lance Patronus à l'ouverture de session et demande avant de fermer pendant une exécution`.

### Tâche C5 : Résumé et liens du script

**Fichiers :** `exec.rs`, `run.rs` (`liens`), `app.rs`.

- [x] Tests : `summary_file_wins`, `links_are_extracted`.
- [x] Commit `C5 — Prend le résumé écrit par le script et relève ses liens`.

### Tâche C6 : Petits plus

**Fichiers :** `engine.rs` (pause), `app.rs`, `state.rs` (zoom), `build.rs` et `icon.rs` (déjà écrits), `main.rs`
(tests de la ressource de version et de l'icône).

- [x] Tests : `paused_engine_starts_nothing_automatically`, `paused_time_is_not_missed`, `inbox_search`,
  `exe_has_version_info`, `exe_has_icon`, `icon_has_a_transparent_background_and_an_opaque_head`.
- [x] Commit `C6 — Ajoute la pause générale, la recherche, la duplication, le zoom et l'icône`.

### Tâche finale

- [x] Revue de tout l'ensemble (superpowers:requesting-code-review), corrections.
- [x] README, `spec.md` (questions ouvertes tranchées), `journal.md` à jour.
