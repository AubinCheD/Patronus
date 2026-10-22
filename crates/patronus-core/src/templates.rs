//! Modèles de tâches proposés par *Nouvelle tâche* : des points de départ courants, à ajuster.

use std::path::PathBuf;

use crate::task::{ClaudeAction, FileWatch, Jour, RemoteBranch, Script, Shell, Task, Trigger, Worktree};

pub struct Template {
    pub nom: &'static str,
    pub description: &'static str,
    build: fn(Option<PathBuf>) -> Task,
}

impl Template {
    /// La tâche du modèle, pour le dépôt donné (le plus souvent nécessaire).
    pub fn task(&self, depot: Option<PathBuf>) -> Task {
        (self.build)(depot)
    }
}

const WEEKDAYS: [Jour; 5] = [Jour::Lun, Jour::Mar, Jour::Mer, Jour::Jeu, Jour::Ven];

fn script(nom: &str, depot: Option<PathBuf>, shell: Shell, commande: &str, triggers: Vec<Trigger>) -> Task {
    let mut t = Task::new_script(nom);
    t.depot = depot;
    t.action.script =
        Some(Script { commande: commande.into(), shell, dossier: None, env: Default::default(), ..Default::default() });
    t.declencheurs = triggers;
    t
}

fn claude(
    nom: &str,
    depot: Option<PathBuf>,
    action: ClaudeAction,
    branch: Option<&str>,
    triggers: Vec<Trigger>,
) -> Task {
    let mut t = Task::new_script(nom);
    t.depot = depot;
    t.action.script = None;
    t.action.claude = Some(action);
    t.action.worktree = branch.map(Worktree::on);
    t.declencheurs = triggers;
    t
}

fn tools(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

pub fn templates() -> Vec<Template> {
    vec![
        Template {
            nom: "Tâche vide",
            description: "Un script, sans déclencheur : à remplir.",
            build: |depot| {
                let mut t = Task::new_script("Nouvelle tâche");
                t.depot = depot;
                t
            },
        },
        Template {
            nom: "Point du matin",
            description: "En semaine à 8 h 30 : commits de la veille, état de la copie de travail.",
            build: |depot| {
                script(
                    "Point du matin",
                    depot,
                    Shell::Powershell,
                    "git log --all --since=\"yesterday 00:00\" --format=\"%h %an : %s\"; git status --short --branch",
                    vec![Trigger::Heure { heure: "08:30".into(), jours: WEEKDAYS.to_vec(), rattrapage: true }],
                )
            },
        },
        Template {
            nom: "Revue des dépendances",
            description: "En semaine à 8 h : Claude liste les mises à jour et propose un diff pour les mineures.",
            build: |depot| {
                claude(
                    "Revue des dépendances",
                    depot,
                    ClaudeAction {
                        consigne:
                            "Liste les dépendances à mettre à jour et les avis de sécurité. Propose un diff pour \
                                   les mises à jour mineures uniquement, sans rien pousser."
                                .into(),
                        outils: tools(&[
                            "Read",
                            "Grep",
                            "Glob",
                            "Edit",
                            "Bash(npm outdated)",
                            "Bash(cargo update --dry-run)",
                        ]),
                        tours_max: Some(30),
                        budget_usd: Some(1.0),
                        mode_permission: None,
                        ..Default::default()
                    },
                    Some("main"),
                    vec![Trigger::Heure { heure: "08:00".into(), jours: WEEKDAYS.to_vec(), rattrapage: false }],
                )
            },
        },
        Template {
            nom: "Doc à jour après une fusion",
            description: "Quand origin/main avance : Claude vérifie que la doc décrit encore le code.",
            build: |depot| {
                claude(
                    "Doc à jour après une fusion",
                    depot,
                    ClaudeAction {
                        consigne: "Les commits listés ci-dessous viennent d'arriver sur main. Vérifie que la \
                                   documentation (README, docs/) décrit encore le code ; corrige-la si besoin, sans \
                                   toucher au code."
                            .into(),
                        outils: tools(&["Read", "Grep", "Glob", "Edit", "Write"]),
                        tours_max: Some(40),
                        budget_usd: Some(1.5),
                        mode_permission: None,
                        ..Default::default()
                    },
                    Some("origin/main"),
                    vec![Trigger::Branche { branche_distante: RemoteBranch::new("main") }],
                )
            },
        },
        Template {
            nom: "Surveiller un dossier",
            description: "Quand un fichier de docs/ change : un script reçoit la liste des fichiers.",
            build: |depot| {
                let dossier = depot.clone().unwrap_or_default();
                script(
                    "Surveiller un dossier",
                    depot,
                    Shell::Powershell,
                    "Write-Output \"Modifié :\"; Write-Output $env:PATRONUS_FICHIERS",
                    vec![Trigger::Fichiers {
                        fichiers: FileWatch {
                            dossier,
                            inclure: vec!["docs/**".into()],
                            exclure: vec![],
                            ..Default::default()
                        },
                    }],
                )
            },
        },
        Template {
            nom: "Tests réguliers",
            description: "Toutes les heures : lance les tests et signale un échec.",
            build: |depot| {
                script(
                    "Tests réguliers",
                    depot,
                    Shell::Powershell,
                    "cargo test",
                    vec![Trigger::Intervalle { toutes_les_minutes: 60 }],
                )
            },
        },
        Template {
            nom: "Essai Claude (lecture seule)",
            description: "À la main : un premier essai de session Claude, lecture seule, 0,10 $ au plus.",
            build: |depot| {
                claude(
                    "Essai Claude (lecture seule)",
                    depot,
                    ClaudeAction {
                        consigne: "Résume en cinq lignes ce que fait ce dépôt, puis signale un point qui mériterait \
                                   d'être amélioré. Ne modifie rien."
                            .into(),
                        outils: Vec::new(),
                        tours_max: Some(5),
                        budget_usd: Some(0.10),
                        mode_permission: None,
                        ..Default::default()
                    },
                    None,
                    Vec::new(),
                )
            },
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_template_is_valid_once_a_repo_is_chosen() {
        let all = templates();
        assert_eq!(all.len(), 7);
        // La tâche vide est à remplir : seule sa commande manque.
        assert_eq!(all[0].task(None).validate(), ["La commande du script est vide."]);
        for t in all.iter().skip(1) {
            let task = t.task(Some(PathBuf::from(r"C:\dev\contoso-api")));
            assert!(task.validate().is_empty(), "{} : {:?}", t.nom, task.validate());
        }
    }

    #[test]
    fn names_are_unique_and_described() {
        let all = templates();
        for (i, t) in all.iter().enumerate() {
            assert!(!t.description.is_empty());
            assert!(all.iter().skip(i + 1).all(|o| o.nom != t.nom), "{}", t.nom);
        }
    }
}
