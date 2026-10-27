//! Format d'une tâche : un fichier JSON lisible et versionnable, un par tâche, dans
//! `<données>\taches\<id>.json`. Les champs inconnus sont gardés tels quels, pour qu'une version plus
//! ancienne de Patronus ne perde rien de ce qu'une plus récente a écrit.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::fsutil::{read_text, write_atomic};

/// Une tâche : quand elle part, ce qu'elle fait, ses limites et où va son résultat.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub nom: String,
    /// Dépôt concerné : dossier de travail par défaut, et source du worktree d'une session Claude.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depot: Option<PathBuf>,
    /// En pause quand faux : seuls les lancements à la main partent.
    #[serde(default = "yes")]
    pub active: bool,
    /// Ses échéances horaires partent aussi quand Patronus est fermé (tâche du Planificateur de Windows).
    #[serde(default, skip_serializing_if = "is_default")]
    pub hors_application: bool,
    #[serde(default)]
    pub declencheurs: Vec<Trigger>,
    pub action: Action,
    #[serde(default)]
    pub garde_fous: Guards,
    #[serde(default)]
    pub sortie: Output,
    #[serde(flatten)]
    pub autres: BTreeMap<String, Value>,
}

fn yes() -> bool {
    true
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Jour {
    Lun,
    Mar,
    Mer,
    Jeu,
    Ven,
    Sam,
    Dim,
}

impl Jour {
    pub const ALL: [Jour; 7] = [Jour::Lun, Jour::Mar, Jour::Mer, Jour::Jeu, Jour::Ven, Jour::Sam, Jour::Dim];

    pub fn label(self) -> &'static str {
        match self {
            Jour::Lun => "lun",
            Jour::Mar => "mar",
            Jour::Mer => "mer",
            Jour::Jeu => "jeu",
            Jour::Ven => "ven",
            Jour::Sam => "sam",
            Jour::Dim => "dim",
        }
    }

    /// Lundi = 0.
    pub fn index(self) -> u32 {
        self as u32
    }
}

/// Quand une tâche part. Le lancement à la main est toujours possible et n'a pas besoin d'être déclaré.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Trigger {
    /// À une heure fixe (`"08:00"`), les jours donnés (tous si la liste est vide).
    Heure {
        heure: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        jours: Vec<Jour>,
        /// Une exécution manquée (application fermée) part à l'ouverture, au lieu d'être seulement signalée.
        #[serde(default, skip_serializing_if = "is_default")]
        rattrapage: bool,
    },
    /// Toutes les N minutes, comptées depuis minuit.
    Intervalle { toutes_les_minutes: u32 },
    /// Un fichier modifié dans un dossier.
    Fichiers { fichiers: FileWatch },
    /// La branche d'un dépôt distant avance (nouveau commit, PR fusionnée). Le dépôt est celui de la tâche.
    Branche { branche_distante: RemoteBranch },
}

/// Branche distante surveillée : lue par `git ls-remote` toutes les `toutes_les_minutes` minutes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RemoteBranch {
    pub branche: String,
    #[serde(default = "default_remote")]
    pub distant: String,
    #[serde(default = "default_poll")]
    pub toutes_les_minutes: u32,
    /// Champs inconnus (écrits par une version plus récente), gardés tels quels.
    #[serde(flatten)]
    pub autres: BTreeMap<String, Value>,
}

fn default_remote() -> String {
    "origin".into()
}

fn default_poll() -> u32 {
    5
}

impl RemoteBranch {
    pub fn new(branche: &str) -> RemoteBranch {
        RemoteBranch {
            branche: branche.into(),
            distant: default_remote(),
            toutes_les_minutes: default_poll(),
            autres: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FileWatch {
    pub dossier: PathBuf,
    /// Motifs relatifs au dossier (`docs/**`, `*.toml`) ; tout le dossier si la liste est vide.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inclure: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclure: Vec<String>,
    /// Champs inconnus (écrits par une version plus récente), gardés tels quels.
    #[serde(flatten)]
    pub autres: BTreeMap<String, Value>,
}

/// Ce que fait la tâche : exactement une action parmi `script` et `claude`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Action {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<Script>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude: Option<ClaudeAction>,
    /// Session Claude : travailler dans un worktree dédié de cette branche.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<Worktree>,
    /// Champs inconnus (écrits par une version plus récente), gardés tels quels.
    #[serde(flatten)]
    pub autres: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Script {
    pub commande: String,
    #[serde(default)]
    pub shell: Shell,
    /// Dossier de travail ; à défaut le dépôt de la tâche, à défaut le dossier personnel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dossier: Option<PathBuf>,
    /// Variables d'environnement (jamais de secret ici : voir `validate`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Champs inconnus (écrits par une version plus récente), gardés tels quels.
    #[serde(flatten)]
    pub autres: BTreeMap<String, Value>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Shell {
    #[default]
    Powershell,
    Cmd,
    Bash,
}

impl Shell {
    pub const ALL: [Shell; 3] = [Shell::Powershell, Shell::Cmd, Shell::Bash];

    pub fn label(self) -> &'static str {
        match self {
            Shell::Powershell => "PowerShell",
            Shell::Cmd => "cmd",
            Shell::Bash => "bash",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ClaudeAction {
    pub consigne: String,
    /// Outils permis (`--allowedTools`). Vide : lecture seule (Read, Grep, Glob).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outils: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tours_max: Option<u32>,
    /// Budget en dollars, équivalent API.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode_permission: Option<String>,
    /// Champs inconnus (écrits par une version plus récente), gardés tels quels.
    #[serde(flatten)]
    pub autres: BTreeMap<String, Value>,
}

/// Outils de Claude Code qui ne modifient rien.
pub const READ_ONLY_TOOLS: [&str; 7] = ["Read", "Grep", "Glob", "LS", "WebSearch", "WebFetch", "TodoWrite"];

/// Modes de permission acceptés (`bypassPermissions` ignorerait la liste des outils, `auto` l'élargirait).
pub const PERMISSION_MODES: [&str; 4] = ["default", "acceptEdits", "plan", "dontAsk"];

/// Nom de l'outil d'une règle : `Bash(npm outdated)` → `Bash`.
pub fn tool_name(rule: &str) -> &str {
    let rule = rule.trim();
    rule.split_once('(').map_or(rule, |(name, _)| name.trim())
}

impl Worktree {
    /// Worktree dédié de la branche `branche`.
    pub fn on(branche: &str) -> Worktree {
        Worktree { branche: branche.to_string(), autres: BTreeMap::new() }
    }
}

impl ClaudeAction {
    /// La session peut-elle écrire ? Tout outil au-delà de la lecture le permet.
    pub fn writes(&self) -> bool {
        self.outils.iter().any(|t| !READ_ONLY_TOOLS.contains(&tool_name(t)))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Worktree {
    pub branche: String,
    /// Champs inconnus (écrits par une version plus récente), gardés tels quels.
    #[serde(flatten)]
    pub autres: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Guards {
    /// Durée maximale d'une exécution ; au-delà elle est arrêtée.
    #[serde(default = "default_duration")]
    pub duree_max_min: u32,
    /// Champs inconnus (écrits par une version plus récente), gardés tels quels.
    #[serde(flatten)]
    pub autres: BTreeMap<String, Value>,
}

fn default_duration() -> u32 {
    30
}

impl Default for Guards {
    fn default() -> Self {
        Guards { duree_max_min: default_duration(), autres: BTreeMap::new() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Output {
    /// Notification Windows à la fin.
    #[serde(default = "yes")]
    pub notification: bool,
    /// Champs inconnus (écrits par une version plus récente), gardés tels quels.
    #[serde(flatten)]
    pub autres: BTreeMap<String, Value>,
}

impl Default for Output {
    fn default() -> Self {
        Output { notification: true, autres: BTreeMap::new() }
    }
}

/// `"08:00"` → (8, 0).
pub fn parse_heure(s: &str) -> Option<(u32, u32)> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some((h, m))
}

/// Ce qui ressemble à un secret dans un texte de tâche : jetons connus ou affectation de mot de passe.
pub fn looks_like_secret(text: &str) -> bool {
    let tokens = ["ghp_", "github_pat_", "glpat-", "sk-ant-", "xoxb-", "akia"];
    if text.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-').any(|w| {
        let wl = w.to_lowercase();
        tokens.iter().any(|t| wl.starts_with(t) && w.len() >= t.len() + 12)
    }) {
        return true;
    }
    // `password=…`, `token: …` : un secret seulement si la valeur est écrite en clair, pas si elle
    // vient d'une variable (`$token = Get-Secret …`, `%TOKEN%`) ou d'une commande.
    let lower = text.to_lowercase();
    for key in ["password", "mot de passe", "token", "secret"] {
        let mut from = 0;
        while let Some(i) = lower[from..].find(key) {
            // `$token = …` : variable PowerShell ; seule une valeur entre guillemets est écrite en clair
            // (`$token=Get-Secret` appelle une commande).
            let variable = lower[..from + i].ends_with('$');
            let after = &lower[from + i + key.len()..];
            from += i + key.len();
            let after = after.trim_start();
            let Some(value) = after.strip_prefix('=').or_else(|| after.strip_prefix(':')) else { continue };
            let value = value.trim_start();
            let quoted = value.starts_with(['"', '\'']);
            if variable && !quoted {
                continue;
            }
            let value = value.trim_start_matches(['"', '\'']);
            let literal = value.chars().next().is_some_and(|c| !matches!(c, '$' | '%' | '(' | '{' | '<' | '['));
            if literal && value.split_whitespace().next().is_some_and(|v| v.len() >= 4) {
                return true;
            }
        }
    }
    false
}

impl Task {
    /// Une tâche script minimale, point de départ de l'éditeur.
    pub fn new_script(nom: &str) -> Task {
        Task {
            nom: nom.to_string(),
            depot: None,
            active: true,
            hors_application: false,
            declencheurs: Vec::new(),
            action: Action { script: Some(Script::default()), ..Default::default() },
            garde_fous: Guards::default(),
            sortie: Output::default(),
            autres: BTreeMap::new(),
        }
    }

    /// Mises en garde qui n'empêchent pas la tâche de partir.
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(c) = &self.action.claude
            && c.outils.iter().any(|t| matches!(t.trim(), "Bash" | "Bash(*)" | "PowerShell" | "PowerShell(*)"))
        {
            out.push(
                "Bash sans restriction : la session peut lancer n'importe quelle commande dans son worktree. Le push \
                 reste impossible, mais préférer des commandes précises, comme Bash(npm outdated)."
                    .into(),
            );
        }
        if let Some(c) = &self.action.claude
            && c.outils.iter().any(|t| tool_name(t) == "WebFetch")
        {
            out.push(
                "WebFetch : la session peut envoyer à une adresse web ce qu'elle a lu du dépôt (il suffit d'une \
                 consigne piégée dans un fichier). À réserver aux dépôts de confiance."
                    .into(),
            );
        }
        if let Some(s) = &self.action.script
            && s.shell == Shell::Cmd
            && s.commande.to_ascii_uppercase().contains("%PATRONUS_FICHIERS%")
        {
            out.push(
                "%PATRONUS_FICHIERS% dans cmd : un nom de fichier qui contient &, | ou > y serait exécuté comme une \
                 commande. Lire plutôt la liste dans le fichier %PATRONUS_FICHIERS_LISTE% (for /f \
                 \"usebackq delims=\" %f in (\"%PATRONUS_FICHIERS_LISTE%\") do …), ou passer à PowerShell \
                 ($env:PATRONUS_FICHIERS)."
                    .into(),
            );
        }
        out
    }

    /// Problèmes qui empêchent la tâche de partir, en français. Vide : la tâche est valide.
    pub fn validate(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.nom.trim().is_empty() {
            out.push("La tâche n'a pas de nom.".to_string());
        }
        match (&self.action.script, &self.action.claude) {
            (None, None) => out.push("Aucune action : choisir un script ou une session Claude.".into()),
            (Some(_), Some(_)) => {
                out.push("Deux actions : une tâche fait un script ou une session Claude, pas les deux.".into())
            }
            (Some(s), None) => {
                if s.commande.trim().is_empty() {
                    out.push("La commande du script est vide.".into());
                }
                if looks_like_secret(&s.commande) || s.env.iter().any(|(k, v)| looks_like_secret(&format!("{k}={v}"))) {
                    out.push(
                        "Le script semble contenir un secret : passer par une variable d'environnement du système ou un \
                         gestionnaire de secrets."
                            .into(),
                    );
                }
            }
            (None, Some(c)) => {
                if c.consigne.trim().is_empty() {
                    out.push("La consigne de la session Claude est vide.".into());
                }
                if looks_like_secret(&c.consigne) {
                    out.push("La consigne semble contenir un secret : il n'a rien à y faire.".into());
                }
                match c.budget_usd {
                    None => out.push("Une session Claude demande un budget (en dollars, équivalent API).".into()),
                    Some(b) if b.is_nan() || b <= 0.0 => out.push("Le budget doit être positif.".into()),
                    _ => {}
                }
                if c.tours_max == Some(0) {
                    out.push("Le nombre de tours doit être positif.".into());
                }
                if self.action.worktree.is_some() && self.depot.is_none() {
                    out.push("Un worktree demande un dépôt.".into());
                }
                let mode = c.mode_permission.as_deref().map(str::trim).filter(|m| !m.is_empty());
                if let Some(m) = mode
                    && !PERMISSION_MODES.contains(&m)
                {
                    out.push(format!(
                        "Mode de permission « {m} » refusé : seuls le mode par défaut, acceptEdits, plan et dontAsk \
                         sont permis."
                    ));
                }
                // Une session qui peut écrire ne touche jamais la copie de travail de l'utilisateur.
                if (c.writes() || mode == Some("acceptEdits"))
                    && (self.depot.is_none() || self.action.worktree.is_none())
                {
                    out.push(
                        "Une session qui peut écrire (outils au-delà de la lecture, ou acceptEdits) travaille dans un \
                         worktree dédié : choisir un dépôt et cocher le worktree."
                            .into(),
                    );
                }
            }
        }
        if self.garde_fous.duree_max_min == 0 {
            out.push("La durée maximale doit être positive.".into());
        }
        for t in &self.declencheurs {
            match t {
                Trigger::Heure { heure, .. } => {
                    if parse_heure(heure).is_none() {
                        out.push(format!("Heure illisible : « {heure} » (attendu : HH:MM)."));
                    }
                }
                Trigger::Intervalle { toutes_les_minutes } => {
                    if *toutes_les_minutes == 0 {
                        out.push("Un intervalle doit être d'au moins une minute.".into());
                    }
                }
                Trigger::Fichiers { fichiers } => {
                    if fichiers.dossier.as_os_str().is_empty() {
                        out.push("Le dossier surveillé n'est pas indiqué.".into());
                    }
                    for p in fichiers.inclure.iter().chain(&fichiers.exclure) {
                        if globset::Glob::new(p).is_err() {
                            out.push(format!("Motif illisible : « {p} »."));
                        }
                    }
                }
                Trigger::Branche { branche_distante: b } => {
                    if self.depot.is_none() {
                        out.push(
                            "Une branche distante se surveille dans le dépôt de la tâche : choisir un dépôt.".into(),
                        );
                    }
                    if b.branche.trim().is_empty() || b.distant.trim().is_empty() {
                        out.push("La branche distante surveillée n'est pas indiquée.".into());
                    }
                    if b.toutes_les_minutes == 0 {
                        out.push("Une branche distante se relit au plus toutes les minutes.".into());
                    }
                }
            }
        }
        out
    }
}

/// Identifiant de fichier tiré du nom : minuscules, sans accents, tirets.
pub fn slug(nom: &str) -> String {
    let mut out = String::new();
    for c in nom.chars().flat_map(char::to_lowercase) {
        let c = match c {
            'à' | 'â' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            c => c,
        };
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "tache".to_string() } else { out }
}

/// Une tâche lue sur disque, ou la raison pour laquelle elle ne se lit pas (jamais ignorée en silence).
#[derive(Clone, Debug)]
pub struct Loaded {
    pub id: String,
    pub path: PathBuf,
    pub task: Result<Task, String>,
    /// Ce qui empêche la tâche de partir (`Task::validate`), calculé au chargement.
    pub problems: Vec<String>,
}

/// Les tâches globales, rangées dans `dir`.
pub struct TaskStore {
    pub dir: PathBuf,
}

impl TaskStore {
    pub fn new(dir: PathBuf) -> Self {
        TaskStore { dir }
    }

    pub fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    /// Toutes les tâches, triées par identifiant.
    pub fn load_all(&self) -> Vec<Loaded> {
        let Ok(entries) = fs::read_dir(&self.dir) else { return Vec::new() };
        let mut out: Vec<Loaded> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .map(|path| {
                let id = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                let task = read_task(&path);
                let problems = task.as_ref().map(Task::validate).unwrap_or_default();
                Loaded { id, path, task, problems }
            })
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// Identifiant libre tiré du nom (`revue`, `revue-2`…).
    pub fn free_id(&self, nom: &str) -> String {
        let base = slug(nom);
        let mut id = base.clone();
        let mut n = 2;
        while self.path(&id).exists() {
            id = format!("{base}-{n}");
            n += 1;
        }
        id
    }

    pub fn save(&self, id: &str, task: &Task) -> std::io::Result<()> {
        let json = serde_json::to_vec_pretty(task).map_err(std::io::Error::other)?;
        write_atomic(&self.path(id), &json)
    }

    /// Supprimer une tâche la met de côté (`<id>.json.supprimee`) plutôt que de l'effacer.
    pub fn delete(&self, id: &str) -> std::io::Result<()> {
        let path = self.path(id);
        // Une tâche déjà mise de côté sous ce nom n'est pas écrasée.
        let mut aside = path.with_extension("json.supprimee");
        let mut n = 2;
        while aside.exists() {
            aside = path.with_extension(format!("json.supprimee-{n}"));
            n += 1;
        }
        fs::rename(&path, aside)
    }
}

pub fn read_task(path: &Path) -> Result<Task, String> {
    let text = read_text(path).map_err(|e| format!("lecture impossible : {e}"))?;
    serde_json::from_str::<Task>(&text).map_err(|e| {
        let msg = e.to_string();
        if msg.contains("did not match any variant") {
            explain_triggers(&text).unwrap_or_else(|| format!("déclencheur illisible (ligne {})", e.line()))
        } else {
            format!("fichier illisible : {msg}")
        }
    })
}

/// Le premier déclencheur qui ne se lit pas, et pourquoi : un déclencheur reconnu à sa clé mais mal
/// écrit est relu seul, pour que l'erreur dise ce qui ne va pas (« jour inconnu `lundi` »…).
fn explain_triggers(text: &str) -> Option<String> {
    #[derive(Deserialize)]
    #[allow(dead_code)]
    struct HeureShape {
        heure: String,
        #[serde(default)]
        jours: Vec<Jour>,
        #[serde(default)]
        rattrapage: bool,
    }
    #[derive(Deserialize)]
    #[allow(dead_code)]
    struct IntervalleShape {
        toutes_les_minutes: u32,
    }
    #[derive(Deserialize)]
    #[allow(dead_code)]
    struct FichiersShape {
        fichiers: FileWatch,
    }
    #[derive(Deserialize)]
    #[allow(dead_code)]
    struct BrancheShape {
        branche_distante: RemoteBranch,
    }
    let v: Value = serde_json::from_str(text).ok()?;
    for (i, t) in v["declencheurs"].as_array()?.iter().enumerate() {
        if serde_json::from_value::<Trigger>(t.clone()).is_ok() {
            continue;
        }
        let n = i + 1;
        let why = if t.get("heure").is_some() {
            serde_json::from_value::<HeureShape>(t.clone()).err()
        } else if t.get("toutes_les_minutes").is_some() {
            serde_json::from_value::<IntervalleShape>(t.clone()).err()
        } else if t.get("fichiers").is_some() {
            serde_json::from_value::<FichiersShape>(t.clone()).err()
        } else if t.get("branche_distante").is_some() {
            serde_json::from_value::<BrancheShape>(t.clone()).err()
        } else {
            let keys: Vec<&str> = t.as_object().map(|o| o.keys().map(String::as_str).collect()).unwrap_or_default();
            return Some(format!(
                "déclencheur n° {n} inconnu ({}) : attendu heure, toutes_les_minutes, fichiers ou branche_distante",
                keys.join(", ")
            ));
        };
        return Some(format!("déclencheur n° {n} mal écrit : {}", why.map(|e| e.to_string()).unwrap_or_default()));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsutil::test_dir;

    const EXAMPLE: &str = r#"{
  "nom": "Revue des dépendances",
  "depot": "C:\\dev\\contoso-api",
  "declencheurs": [{ "heure": "08:00", "jours": ["lun", "mar", "mer", "jeu", "ven"] }],
  "action": {
    "claude": {
      "consigne": "Liste les dépendances à mettre à jour.",
      "outils": ["Read", "Grep", "Bash(npm outdated)", "Edit"],
      "tours_max": 20,
      "budget_usd": 1.0
    },
    "worktree": { "branche": "main" }
  },
  "sortie": { "notification": true }
}"#;

    #[test]
    fn the_spec_example_reads_and_is_valid() {
        let t: Task = serde_json::from_str(EXAMPLE).unwrap();
        assert_eq!(t.nom, "Revue des dépendances");
        assert!(t.active);
        assert_eq!(t.declencheurs.len(), 1);
        assert!(
            matches!(&t.declencheurs[0], Trigger::Heure { heure, jours, .. } if heure == "08:00" && jours.len() == 5)
        );
        assert_eq!(t.action.worktree.as_ref().unwrap().branche, "main");
        assert_eq!(t.garde_fous.duree_max_min, 30);
        assert!(t.validate().is_empty(), "{:?}", t.validate());
    }

    #[test]
    fn every_trigger_kind_round_trips() {
        let mut t = Task::new_script("Essai");
        t.action.script.as_mut().unwrap().commande = "echo bonjour".into();
        t.declencheurs = vec![
            Trigger::Heure { heure: "09:30".into(), jours: vec![Jour::Lun], rattrapage: true },
            Trigger::Intervalle { toutes_les_minutes: 15 },
            Trigger::Fichiers {
                fichiers: FileWatch {
                    dossier: "C:\\dev\\fabrikam".into(),
                    inclure: vec!["docs/**".into()],
                    ..Default::default()
                },
            },
        ];
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(serde_json::from_str::<Task>(&json).unwrap(), t);
        assert!(t.validate().is_empty());
    }

    #[test]
    fn unknown_fields_are_kept() {
        let json = r#"{"nom":"x","action":{"script":{"commande":"dir"}},"futur":{"a":1}}"#;
        let t: Task = serde_json::from_str(json).unwrap();
        let back = serde_json::to_value(&t).unwrap();
        assert_eq!(back["futur"]["a"], 1);
    }

    #[test]
    fn validation_says_what_is_wrong() {
        let mut t = Task::new_script(" ");
        t.declencheurs = vec![
            Trigger::Heure { heure: "25:00".into(), jours: vec![], rattrapage: false },
            Trigger::Intervalle { toutes_les_minutes: 0 },
            Trigger::Fichiers {
                fichiers: FileWatch {
                    dossier: "".into(),
                    inclure: vec!["[".into()],
                    exclure: vec![],
                    ..Default::default()
                },
            },
        ];
        let p = t.validate().join("\n");
        for needle in [
            "pas de nom",
            "commande du script est vide",
            "25:00",
            "au moins une minute",
            "dossier surveillé",
            "Motif illisible",
        ] {
            assert!(p.contains(needle), "« {needle} » absent de :\n{p}");
        }
        t.action.claude = Some(ClaudeAction::default());
        assert!(t.validate().join("\n").contains("Deux actions"));
    }

    fn claude_task(outils: &[&str], depot: bool, worktree: bool) -> Task {
        let mut t = Task::new_script("Session");
        t.action.script = None;
        t.action.claude = Some(ClaudeAction {
            consigne: "Relis.".into(),
            outils: outils.iter().map(|s| s.to_string()).collect(),
            budget_usd: Some(0.5),
            ..Default::default()
        });
        t.depot = depot.then(|| PathBuf::from(r"C:\dev\contoso-api"));
        t.action.worktree = worktree.then(|| Worktree::on("main"));
        t
    }

    /// Une session qui peut écrire ne touche jamais la copie de travail : dépôt et worktree exigés.
    #[test]
    fn write_tools_need_a_worktree() {
        let needs = |t: &Task| t.validate().join(" ").contains("worktree dédié");
        assert!(!needs(&claude_task(&[], false, false)), "lecture seule par défaut");
        assert!(!needs(&claude_task(&["Read", "Grep", "WebSearch"], true, false)));
        assert!(needs(&claude_task(&["Read", "Edit"], true, false)), "sans worktree");
        assert!(needs(&claude_task(&["Bash(npm outdated)"], false, false)), "sans dépôt");
        assert!(!needs(&claude_task(&["Edit", "Write", "Bash(npm test)"], true, true)));
        let mut accept = claude_task(&[], true, false);
        accept.action.claude.as_mut().unwrap().mode_permission = Some("acceptEdits".into());
        assert!(needs(&accept), "acceptEdits écrit aussi");
    }

    #[test]
    fn unknown_permission_mode_is_refused() {
        for (mode, ok) in
            [("default", true), ("plan", true), ("dontAsk", true), ("bypassPermissions", false), ("auto", false)]
        {
            let mut t = claude_task(&[], true, true);
            t.action.claude.as_mut().unwrap().mode_permission = Some(mode.into());
            assert_eq!(t.validate().is_empty(), ok, "{mode} : {:?}", t.validate());
        }
    }

    #[test]
    fn unrestricted_bash_is_a_warning_not_an_error() {
        let t = claude_task(&["Bash"], true, true);
        assert!(t.validate().is_empty());
        assert!(t.warnings()[0].contains("Bash sans restriction"));
        assert!(claude_task(&["Bash(npm outdated)"], true, true).warnings().is_empty());
        let web = claude_task(&["Read", "WebFetch(domain:contoso.com)"], true, true).warnings();
        assert!(web.len() == 1 && web[0].starts_with("WebFetch"), "{web:?}");
        assert_eq!(tool_name(" Bash(git log *) "), "Bash");
    }

    /// cmd interprète `&`, `|`, `>` dans la valeur d'une variable développée : un nom de fichier piégé y deviendrait
    /// une commande. Mise en garde pour cmd seulement.
    #[test]
    fn the_file_list_variable_in_cmd_is_a_warning() {
        let script = |shell, commande: &str| {
            let mut t = Task::new_script("x");
            let s = t.action.script.as_mut().unwrap();
            (s.shell, s.commande) = (shell, commande.into());
            t
        };
        let w = script(Shell::Cmd, "echo %Patronus_Fichiers% >> log.txt").warnings();
        assert!(w.len() == 1 && w[0].contains("PATRONUS_FICHIERS_LISTE"), "{w:?}");
        assert!(script(Shell::Cmd, "type \"%PATRONUS_FICHIERS_LISTE%\"").warnings().is_empty());
        assert!(script(Shell::Powershell, "$env:PATRONUS_FICHIERS").warnings().is_empty());
    }

    #[test]
    fn secrets_are_refused() {
        let mut t = Task::new_script("x");
        t.action.script.as_mut().unwrap().commande =
            "curl -H 'Authorization: ghp_abcdefghijklmnop1234' https://contoso.com".into();
        assert!(t.validate().join(" ").contains("secret"));
        assert!(looks_like_secret("password=hunter2"));
        assert!(!looks_like_secret("npm outdated"));
        assert!(!looks_like_secret("ghp_")); // trop court pour être un jeton
    }

    #[test]
    fn heure_parsing() {
        assert_eq!(parse_heure("08:05"), Some((8, 5)));
        assert_eq!(parse_heure(" 23:59 "), Some((23, 59)));
        assert_eq!(parse_heure("24:00"), None);
        assert_eq!(parse_heure("8h"), None);
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Revue des dépendances"), "revue-des-dependances");
        assert_eq!(slug("  Point du matin !"), "point-du-matin");
        assert_eq!(slug("***"), "tache");
    }

    #[test]
    fn store_saves_loads_and_reports_unreadable_files() {
        let dir = test_dir("store");
        let store = TaskStore::new(dir.clone());
        let mut t = Task::new_script("Point du matin");
        t.action.script.as_mut().unwrap().commande = "git log".into();
        let id = store.free_id(&t.nom);
        store.save(&id, &t).unwrap();
        assert_eq!(store.free_id(&t.nom), "point-du-matin-2");
        fs::write(dir.join("casse.json"), "{ pas du json").unwrap();
        fs::write(dir.join("bom.json"), "\u{feff}{\"nom\":\"b\",\"action\":{\"script\":{\"commande\":\"dir\"}}}")
            .unwrap();
        let all = store.load_all();
        assert_eq!(all.iter().map(|l| l.id.as_str()).collect::<Vec<_>>(), ["bom", "casse", "point-du-matin"]);
        assert!(all[0].task.is_ok());
        assert!(all[1].task.as_ref().unwrap_err().contains("illisible"));
        assert_eq!(all[2].task.as_ref().unwrap(), &t);
        store.delete(&id).unwrap();
        assert_eq!(store.load_all().len(), 2);
        assert!(dir.join("point-du-matin.json.supprimee").exists());
    }

    #[test]
    fn unknown_trigger_is_explained() {
        let dir = test_dir("trigger");
        let p = dir.join("t.json");
        fs::write(&p, r#"{"nom":"x","declencheurs":[{"lune":"pleine"}],"action":{"script":{"commande":"dir"}}}"#)
            .unwrap();
        assert!(
            read_task(&p).unwrap_err().contains("déclencheur n° 1 inconnu (lune)"),
            "{}",
            read_task(&p).unwrap_err()
        );
    }

    /// Un déclencheur reconnu mais mal écrit est nommé, avec ce qui ne va pas.
    #[test]
    fn malformed_trigger_is_named() {
        let dir = test_dir("trigger-mal-ecrit");
        let p = dir.join("t.json");
        fs::write(
            &p,
            r#"{"nom":"x","declencheurs":[{"toutes_les_minutes":5},{"heure":"08:00","jours":["lundi"]}],"action":{"script":{"commande":"dir"}}}"#,
        )
        .unwrap();
        let err = read_task(&p).unwrap_err();
        assert!(err.contains("déclencheur n° 2 mal écrit") && err.contains("lundi"), "{err}");
    }

    /// Une valeur lue d'une variable ou d'une commande n'est pas un secret écrit en clair.
    #[test]
    fn powershell_variable_is_not_a_secret() {
        assert!(!looks_like_secret("$token=Get-Secret -Name contoso; git fetch"));
        assert!(!looks_like_secret("$token = $env:CONTOSO_TOKEN"));
        assert!(!looks_like_secret("set TOKEN=%CONTOSO_TOKEN%"));
        assert!(!looks_like_secret("token: (Get-Content jeton.txt)"));
        assert!(looks_like_secret("token=abcd1234efgh"));
        assert!(looks_like_secret("mot de passe : hunter22"));
        assert!(looks_like_secret("SECRET='s3cr3t-valeur'"));
    }

    /// Supprimer deux fois une tâche du même nom garde les deux versions mises de côté.
    #[test]
    fn deleting_twice_keeps_both() {
        let dir = test_dir("supprimer-deux-fois");
        let store = TaskStore::new(dir.clone());
        let mut t = Task::new_script("Tests");
        for commande in ["cargo test", "npm test"] {
            t.action.script.as_mut().unwrap().commande = commande.into();
            store.save("tests", &t).unwrap();
            store.delete("tests").unwrap();
        }
        assert!(fs::read_to_string(dir.join("tests.json.supprimee")).unwrap().contains("cargo test"));
        assert!(fs::read_to_string(dir.join("tests.json.supprimee-2")).unwrap().contains("npm test"));
    }

    /// Les champs inconnus sont gardés à tous les niveaux (action, script, session, worktree,
    /// garde-fous, sortie, surveillance).
    #[test]
    fn nested_unknown_fields_are_kept() {
        let json = r#"{"nom":"x","action":{"script":{"commande":"dir","s":1},"a":2},
            "garde_fous":{"duree_max_min":5,"g":3},"sortie":{"notification":true,"o":4},
            "declencheurs":[{"fichiers":{"dossier":"C:/dev","f":5}}]}"#;
        let t: Task = serde_json::from_str(json).unwrap();
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["action"]["script"]["s"], 1);
        assert_eq!(v["action"]["a"], 2);
        assert_eq!(v["garde_fous"]["g"], 3);
        assert_eq!(v["sortie"]["o"], 4);
        assert_eq!(v["declencheurs"][0]["fichiers"]["f"], 5);
    }
}
