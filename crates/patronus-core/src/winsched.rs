//! Quand Patronus est fermé : une tâche du Planificateur de tâches Windows par tâche Patronus qui le
//! demande (`hors_application`), aux mêmes échéances horaires. Elle lance
//! `patronus.exe --donnees <dossier> planifie <tâche>`, qui ne fait rien si la fenêtre est ouverte
//! (elle tient déjà ses échéances) ni si l'échéance a déjà été tenue.
//!
//! Les tâches Windows sont rangées dans un dossier du Planificateur propre au dossier de données
//! (`\Patronus\`, ou `\Patronus-<empreinte>\` pour un autre dossier, celui d'un essai par exemple).
//! Elles tournent dans la session de l'utilisateur, sans droit administrateur, et seulement quand il
//! est connecté.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::NaiveDate;

use crate::task::{Jour, Task, Trigger, parse_heure};

/// Dossier du Planificateur pour ce dossier de données, sans barres obliques (`Patronus`).
pub fn folder(data_dir: &Path, default_dir: &Path) -> String {
    let norm = |p: &Path| p.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase();
    if norm(data_dir) == norm(default_dir) {
        return "Patronus".to_string();
    }
    // FNV-1a 32 bits : court, stable d'une version à l'autre (contrairement au hachage de la std).
    let mut h: u32 = 0x811c_9dc5;
    for b in norm(data_dir).bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    format!("Patronus-{h:08x}")
}

/// La tâche doit-elle avoir sa tâche Windows ? Active, valide, demandée, et avec au moins une
/// échéance horaire (les fichiers et la branche distante demandent Patronus ouvert).
pub fn wanted(task: &Task) -> bool {
    task.hors_application && task.active && task.validate().is_empty() && has_time_trigger(task)
}

pub fn has_time_trigger(task: &Task) -> bool {
    task.declencheurs.iter().any(|t| matches!(t, Trigger::Heure { .. } | Trigger::Intervalle { .. }))
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn day_tag(j: Jour) -> &'static str {
    match j {
        Jour::Lun => "Monday",
        Jour::Mar => "Tuesday",
        Jour::Mer => "Wednesday",
        Jour::Jeu => "Thursday",
        Jour::Ven => "Friday",
        Jour::Sam => "Saturday",
        Jour::Dim => "Sunday",
    }
}

/// Arguments de la tâche Windows. Le dossier n'a jamais de `\` final : devant le guillemet fermant,
/// il le neutraliserait.
pub fn arguments(data_dir: &Path, id: &str) -> String {
    let dir = data_dir.to_string_lossy().replace('/', "\\");
    // Entre guillemets : un nom de fichier de tâche écrit à la main peut contenir une espace.
    format!("--donnees \"{}\" planifie \"{id}\"", dir.trim_end_matches('\\'))
}

/// Définition XML (format du Planificateur, version 1.2) de la tâche Windows d'une tâche Patronus.
/// `from` : date de départ des déclencheurs (aujourd'hui, en pratique).
pub fn task_xml(id: &str, task: &Task, exe: &Path, data_dir: &Path, from: NaiveDate) -> String {
    let mut triggers = String::new();
    let day = from.format("%Y-%m-%d");
    for t in &task.declencheurs {
        match t {
            Trigger::Heure { heure, jours, .. } => {
                let Some((h, m)) = parse_heure(heure) else { continue };
                let _ = write!(
                    triggers,
                    "    <CalendarTrigger>\n      <StartBoundary>{day}T{h:02}:{m:02}:00</StartBoundary>\n"
                );
                if jours.is_empty() || jours.len() == 7 {
                    triggers.push_str("      <ScheduleByDay><DaysInterval>1</DaysInterval></ScheduleByDay>\n");
                } else {
                    let days: String = jours.iter().map(|j| format!("<{}/>", day_tag(*j))).collect();
                    let _ = writeln!(
                        triggers,
                        "      <ScheduleByWeek><DaysOfWeek>{days}</DaysOfWeek><WeeksInterval>1</WeeksInterval></ScheduleByWeek>"
                    );
                }
                triggers.push_str("    </CalendarTrigger>\n");
            }
            Trigger::Intervalle { toutes_les_minutes: n } => {
                let _ =
                    writeln!(triggers, "    <CalendarTrigger>\n      <StartBoundary>{day}T00:00:00</StartBoundary>");
                // Compté depuis minuit, comme dans Patronus ; au-delà d'un jour, une fois par jour.
                if *n < 24 * 60 {
                    let _ = writeln!(
                        triggers,
                        "      <Repetition><Interval>PT{n}M</Interval><Duration>P1D</Duration><StopAtDurationEnd>false</StopAtDurationEnd></Repetition>"
                    );
                }
                triggers.push_str(
                    "      <ScheduleByDay><DaysInterval>1</DaysInterval></ScheduleByDay>\n    </CalendarTrigger>\n",
                );
            }
            _ => {}
        }
    }
    let catch_up = task.declencheurs.iter().any(|t| matches!(t, Trigger::Heure { rattrapage: true, .. }));
    // Patronus applique lui-même la durée maximale ; Windows n'intervient qu'au-delà, en dernier recours.
    let limit = task.garde_fous.duree_max_min + 10;
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Patronus : {desc}</Description>
  </RegistrationInfo>
  <Triggers>
{triggers}  </Triggers>
  <Principals>
    <Principal id="Author">
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <StartWhenAvailable>{catch_up}</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT{limit}M</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{exe}</Command>
      <Arguments>{args}</Arguments>
    </Exec>
  </Actions>
</Task>
"#,
        desc = esc(&task.nom),
        exe = esc(&exe.to_string_lossy()),
        args = esc(&arguments(data_dir, id)),
    )
}

fn schtasks(args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new(crate::fsutil::windows_program(r"System32\schtasks.exe"));
    cmd.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map_err(|e| format!("schtasks introuvable : {e}"))?;
    // schtasks écrit dans la page de code OEM : les messages d'erreur peuvent perdre leurs accents.
    let text = |b: &[u8]| String::from_utf8_lossy(b).trim().to_string();
    if out.status.success() { Ok(text(&out.stdout)) } else { Err(text(&out.stderr)) }
}

/// Identifiants des tâches Windows présentes dans le dossier `folder`.
pub fn list(folder: &str) -> Result<Vec<String>, String> {
    let out = schtasks(&["/Query", "/FO", "CSV", "/NH"])?;
    Ok(parse_list(&out, folder))
}

/// Lit la sortie CSV de `schtasks /Query` : la première colonne est le chemin de la tâche.
pub fn parse_list(csv: &str, folder: &str) -> Vec<String> {
    let prefix = format!("\\{folder}\\").to_lowercase();
    let mut ids: Vec<String> = csv
        .lines()
        .filter_map(|l| l.strip_prefix('"')?.split('"').next())
        .filter(|path| path.to_lowercase().starts_with(&prefix))
        .map(|path| path[prefix.len()..].to_string())
        .filter(|id| !id.is_empty() && !id.contains('\\'))
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// Crée ou remplace la tâche Windows `\<folder>\<id>`.
pub fn register(folder: &str, id: &str, xml: &str, scratch: &Path) -> Result<(), String> {
    // Le Planificateur attend un fichier UTF-16 avec marque d'ordre d'octets, comme annoncé dans l'en-tête.
    let path = scratch.join(format!("planificateur-{id}.xml"));
    let bytes: Vec<u8> = [0xFF, 0xFE].into_iter().chain(xml.encode_utf16().flat_map(u16::to_le_bytes)).collect();
    std::fs::create_dir_all(scratch).map_err(|e| e.to_string())?;
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    let name = format!("\\{folder}\\{id}");
    let result = schtasks(&["/Create", "/TN", &name, "/XML", &path.to_string_lossy(), "/F"]);
    let _ = std::fs::remove_file(&path);
    result.map(|_| ())
}

pub fn unregister(folder: &str, id: &str) -> Result<(), String> {
    schtasks(&["/Delete", "/TN", &format!("\\{folder}\\{id}"), "/F"]).map(|_| ())
}

/// Supprime le dossier (vide) du Planificateur : `schtasks` ne sait pas le faire, l'interface COM du
/// Planificateur si (par PowerShell). Un dossier qui contient encore des tâches n'est pas supprimé.
pub fn remove_folder(folder: &str) -> Result<(), String> {
    if !folder.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(format!("nom de dossier inattendu : {folder}"));
    }
    let script = format!(
        "$s = New-Object -ComObject Schedule.Service; $s.Connect(); \
         $f = $s.GetFolder('\\'); try {{ $f.DeleteFolder('{folder}', 0) }} catch {{ exit 1 }}"
    );
    let mut cmd = Command::new(crate::fsutil::windows_program(r"System32\WindowsPowerShell\v1.0\powershell.exe"));
    cmd.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", &script]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() { Ok(()) } else { Err(format!("dossier {folder} non supprimé")) }
}

/// Met le Planificateur en accord avec les tâches : crée ou remplace celles qui doivent exister,
/// supprime les autres de son dossier. Rend ce qui a échoué, en français. Si d'autres comptes du
/// poste peuvent modifier l'exécutable ou son dossier, aucune tâche ne le lance : elles ne sont pas
/// créées, et celles qui existent sont retirées.
pub fn sync(folder: &str, wanted: &[(String, String)], scratch: &Path) -> Vec<String> {
    let mut problems = Vec::new();
    let wanted = match wanted.is_empty() {
        true => wanted,
        false => match crate::exeplace::check(&current_exe()) {
            Ok(()) => wanted,
            Err(e) => {
                problems.push(format!("Tâches Windows non créées : {e}"));
                &[]
            }
        },
    };
    let existing = match list(folder) {
        Ok(ids) => ids,
        Err(e) => {
            problems.push(format!("Planificateur de tâches illisible : {e}"));
            Vec::new()
        }
    };
    if wanted.is_empty() && existing.is_empty() {
        return problems;
    }
    for (id, xml) in wanted {
        if let Err(e) = register(folder, id, xml, scratch) {
            problems.push(format!("Tâche Windows « {id} » non créée : {e}"));
        }
    }
    for id in existing.iter().filter(|id| !wanted.iter().any(|(w, _)| w.eq_ignore_ascii_case(id))) {
        if let Err(e) = unregister(folder, id) {
            problems.push(format!("Tâche Windows « {id} » non supprimée : {e}"));
        }
    }
    // Plus aucune tâche : le dossier du Planificateur ne reste pas, vide.
    if wanted.is_empty() && problems.is_empty() {
        let _ = remove_folder(folder);
    }
    problems
}

/// Tâches Windows voulues pour un ensemble de tâches Patronus : (identifiant, XML).
pub fn wanted_set(tasks: &[(String, Task)], exe: &Path, data_dir: &Path, from: NaiveDate) -> Vec<(String, String)> {
    tasks.iter().filter(|(_, t)| wanted(t)).map(|(id, t)| (id.clone(), task_xml(id, t, exe, data_dir, from))).collect()
}

/// L'exécutable à inscrire : celui qui tourne.
pub fn current_exe() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("patronus.exe"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::Script;

    fn task(triggers: Vec<Trigger>) -> Task {
        let mut t = Task::new_script("Point du matin & co");
        t.action.script = Some(Script { commande: "git log".into(), ..Default::default() });
        t.declencheurs = triggers;
        t.hors_application = true;
        t
    }

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 21).unwrap()
    }

    #[test]
    fn folder_is_shared_by_default_and_distinct_otherwise() {
        let def = Path::new(r"D:\profil\AppData\Roaming\Patronus");
        assert_eq!(folder(Path::new(r"d:\profil\appdata\roaming\patronus\"), def), "Patronus");
        let a = folder(Path::new(r"C:\Temp\essai"), def);
        assert!(a.starts_with("Patronus-") && a.len() == "Patronus-".len() + 8, "{a}");
        assert_eq!(a, folder(Path::new("C:/Temp/essai"), def));
        assert_ne!(a, folder(Path::new(r"C:\Temp\autre"), def));
    }

    #[test]
    fn weekly_daily_and_interval_triggers() {
        let t = task(vec![
            Trigger::Heure { heure: "08:30".into(), jours: vec![Jour::Lun, Jour::Ven], rattrapage: true },
            Trigger::Heure { heure: "18:00".into(), jours: vec![], rattrapage: false },
            Trigger::Intervalle { toutes_les_minutes: 15 },
            Trigger::Intervalle { toutes_les_minutes: 3000 },
        ]);
        let xml = task_xml("point", &t, Path::new(r"C:\outils\patronus.exe"), Path::new(r"C:\donnees\"), day());
        assert!(xml.contains("<StartBoundary>2026-10-21T08:30:00</StartBoundary>"));
        assert!(xml.contains("<DaysOfWeek><Monday/><Friday/></DaysOfWeek>"));
        assert!(xml.contains("<StartBoundary>2026-10-21T18:00:00</StartBoundary>"));
        assert!(xml.contains("<Interval>PT15M</Interval><Duration>P1D</Duration>"));
        assert_eq!(xml.matches("<Repetition>").count(), 1, "pas de répétition au-delà d'un jour");
        assert_eq!(xml.matches("<CalendarTrigger>").count(), 4);
        assert!(xml.contains("<StartWhenAvailable>true</StartWhenAvailable>"));
        assert!(xml.contains("<Description>Patronus : Point du matin &amp; co</Description>"));
        assert!(xml.contains("<Command>C:\\outils\\patronus.exe</Command>"));
        assert!(
            xml.contains("<Arguments>--donnees &quot;C:\\donnees&quot; planifie &quot;point&quot;</Arguments>"),
            "{xml}"
        );
        assert!(xml.contains("<ExecutionTimeLimit>PT40M</ExecutionTimeLimit>"));
    }

    #[test]
    fn only_requested_valid_timed_tasks_get_a_windows_task() {
        let timed = task(vec![Trigger::Intervalle { toutes_les_minutes: 30 }]);
        assert!(wanted(&timed));
        let mut not_asked = timed.clone();
        not_asked.hors_application = false;
        assert!(!wanted(&not_asked));
        let mut paused = timed.clone();
        paused.active = false;
        assert!(!wanted(&paused));
        let files = task(vec![Trigger::Fichiers { fichiers: Default::default() }]);
        assert!(!wanted(&files));
        let tasks = vec![("a".to_string(), timed), ("b".to_string(), not_asked)];
        let set = wanted_set(&tasks, Path::new("p.exe"), Path::new("d"), day());
        assert_eq!(set.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), ["a"]);
    }

    #[test]
    fn list_keeps_only_this_folder() {
        let csv = "\"\\Adobe Update\",\"22/10/2026 15:00:00\",\"Prêt\"\r\n\
                   \"\\Patronus\\point-du-matin\",\"22/10/2026 08:30:00\",\"Prêt\"\r\n\
                   \"\\Patronus\\point-du-matin\",\"22/10/2026 08:30:00\",\"Prêt\"\r\n\
                   \"\\Patronus-1a2b3c4d\\essai\",\"N/A\",\"Prêt\"\r\n\
                   \"\\patronus\\revue\",\"N/A\",\"Désactivée\"\r\n";
        assert_eq!(parse_list(csv, "Patronus"), ["point-du-matin", "revue"]);
        assert_eq!(parse_list(csv, "Patronus-1a2b3c4d"), ["essai"]);
    }

    /// Inscription réelle dans un dossier d'essai du Planificateur, puis suppression. Ignoré par
    /// défaut : il modifie le Planificateur de l'utilisateur (`cargo test -- --ignored`).
    #[test]
    #[ignore]
    fn registers_and_removes_a_real_windows_task() {
        let scratch = crate::fsutil::test_dir("winsched");
        let folder = format!("Patronus-essai-{}", std::process::id());
        let t = task(vec![Trigger::Heure { heure: "03:17".into(), jours: vec![Jour::Dim], rattrapage: false }]);
        let xml = task_xml("essai", &t, &current_exe(), &scratch, day());
        assert!(sync(&folder, &[("essai".into(), xml)], &scratch).is_empty());
        assert_eq!(list(&folder).unwrap(), ["essai"]);
        assert!(sync(&folder, &[], &scratch).is_empty());
        assert!(list(&folder).unwrap().is_empty());
        assert!(remove_folder(&folder).is_err(), "déjà supprimé par la synchronisation");
    }
}
