//! Échéances des déclencheurs horaires, en heure locale sans fuseau (l'heure du mur, comme on la
//! lit dans la tâche).

use chrono::{Datelike, Duration, NaiveDateTime, NaiveTime, Timelike};

use crate::task::{Task, Trigger, parse_heure};

/// Prochain départ strictement après `t`, pour un déclencheur horaire (`None` pour les autres).
pub fn next_after(trigger: &Trigger, t: NaiveDateTime) -> Option<NaiveDateTime> {
    match trigger {
        Trigger::Heure { heure, jours, .. } => {
            let (h, m) = parse_heure(heure)?;
            let time = NaiveTime::from_hms_opt(h, m, 0)?;
            (0..=7).map(|d| t.date() + Duration::days(d)).find_map(|date| {
                let day_ok = jours.is_empty() || jours.iter().any(|j| j.index() == date.weekday().num_days_from_monday());
                let at = date.and_time(time);
                (day_ok && at > t).then_some(at)
            })
        }
        Trigger::Intervalle { toutes_les_minutes } => {
            let n = (*toutes_les_minutes).max(1) as i64;
            let midnight = t.date().and_hms_opt(0, 0, 0)?;
            let minute = (t.hour() * 60 + t.minute()) as i64;
            let next = (minute / n + 1) * n;
            // Le compte repart de minuit chaque jour.
            Some(if next >= 24 * 60 { midnight + Duration::days(1) } else { midnight + Duration::minutes(next) })
        }
        Trigger::Fichiers { .. } => None,
    }
}

/// Prochain départ horaire de la tâche, tous déclencheurs confondus.
pub fn next_due(task: &Task, t: NaiveDateTime) -> Option<NaiveDateTime> {
    task.declencheurs.iter().filter_map(|tr| next_after(tr, t)).min()
}

/// Départs manqués dans `]from, to]` : par déclencheur horaire, le dernier manqué et s'il faut le
/// rattraper. Plusieurs départs manqués d'un même déclencheur n'en font qu'un (on ne rejoue pas
/// trois jours d'absence trois fois).
pub fn missed(task: &Task, from: NaiveDateTime, to: NaiveDateTime) -> Vec<Missed> {
    let mut out = Vec::new();
    for tr in &task.declencheurs {
        let mut last = None;
        let mut count = 0;
        let mut cur = from;
        while let Some(next) = next_after(tr, cur) {
            if next > to {
                break;
            }
            last = Some(next);
            count += 1;
            cur = next;
            if count > 100_000 {
                break;
            }
        }
        if let Some(at) = last {
            let catch_up = matches!(tr, Trigger::Heure { rattrapage: true, .. });
            out.push(Missed { at, count, catch_up });
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq)]
pub struct Missed {
    /// Le plus récent des départs manqués.
    pub at: NaiveDateTime,
    pub count: usize,
    pub catch_up: bool,
}

/// Description courte d'un déclencheur, pour la liste des tâches.
pub fn describe(trigger: &Trigger) -> String {
    match trigger {
        Trigger::Heure { heure, jours, .. } => {
            if jours.is_empty() || jours.len() == 7 {
                format!("chaque jour à {heure}")
            } else {
                let days: Vec<_> = jours.iter().map(|j| j.label()).collect();
                format!("{} à {heure}", days.join(", "))
            }
        }
        Trigger::Intervalle { toutes_les_minutes: n } => format!("toutes les {n} min"),
        Trigger::Fichiers { fichiers } => {
            let what = if fichiers.inclure.is_empty() { "*".to_string() } else { fichiers.inclure.join(", ") };
            format!("fichiers {what} dans {}", fichiers.dossier.display())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::Jour;
    use chrono::NaiveDate;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, mo, d).unwrap().and_hms_opt(h, mi, 0).unwrap()
    }

    fn heure(h: &str, jours: Vec<Jour>) -> Trigger {
        Trigger::Heure { heure: h.into(), jours, rattrapage: false }
    }

    // Le 21/10/2026 est un mercredi.
    #[test]
    fn daily_hour() {
        let tr = heure("08:00", vec![]);
        assert_eq!(next_after(&tr, at(2026, 10, 21, 7, 59)), Some(at(2026, 10, 21, 8, 0)));
        assert_eq!(next_after(&tr, at(2026, 10, 21, 8, 0)), Some(at(2026, 10, 22, 8, 0)), "strictement après");
    }

    #[test]
    fn weekdays_only() {
        let tr = heure("09:00", vec![Jour::Lun, Jour::Mar, Jour::Mer, Jour::Jeu, Jour::Ven]);
        // Vendredi 23 après 9 h → lundi 26.
        assert_eq!(next_after(&tr, at(2026, 10, 23, 10, 0)), Some(at(2026, 10, 26, 9, 0)));
        let monday = heure("09:00", vec![Jour::Lun]);
        assert_eq!(next_after(&monday, at(2026, 10, 26, 9, 0)), Some(at(2026, 11, 2, 9, 0)));
    }

    #[test]
    fn intervals_count_from_midnight() {
        let tr = Trigger::Intervalle { toutes_les_minutes: 15 };
        assert_eq!(next_after(&tr, at(2026, 10, 21, 10, 7)), Some(at(2026, 10, 21, 10, 15)));
        assert_eq!(next_after(&tr, at(2026, 10, 21, 10, 15)), Some(at(2026, 10, 21, 10, 30)));
        assert_eq!(next_after(&tr, at(2026, 10, 21, 23, 50)), Some(at(2026, 10, 22, 0, 0)));
        let odd = Trigger::Intervalle { toutes_les_minutes: 7 * 60 };
        assert_eq!(next_after(&odd, at(2026, 10, 21, 21, 0)), Some(at(2026, 10, 22, 0, 0)));
    }

    #[test]
    fn next_due_takes_the_earliest() {
        let mut t = Task::new_script("x");
        t.declencheurs = vec![heure("18:00", vec![]), heure("12:00", vec![])];
        assert_eq!(next_due(&t, at(2026, 10, 21, 11, 0)), Some(at(2026, 10, 21, 12, 0)));
        t.declencheurs = vec![];
        assert_eq!(next_due(&t, at(2026, 10, 21, 11, 0)), None);
    }

    #[test]
    fn missed_runs_collapse_per_trigger() {
        let mut t = Task::new_script("x");
        t.declencheurs = vec![
            Trigger::Heure { heure: "08:00".into(), jours: vec![], rattrapage: true },
            Trigger::Intervalle { toutes_les_minutes: 60 },
        ];
        // Fermé du lundi 19 à 18 h au mercredi 21 à 9 h 30.
        let m = missed(&t, at(2026, 10, 19, 18, 0), at(2026, 10, 21, 9, 30));
        assert_eq!(m.len(), 2);
        assert_eq!(m[0], Missed { at: at(2026, 10, 21, 8, 0), count: 2, catch_up: true });
        assert_eq!(m[1].at, at(2026, 10, 21, 9, 0));
        assert!(!m[1].catch_up);
        assert!(missed(&t, at(2026, 10, 21, 9, 1), at(2026, 10, 21, 9, 30)).is_empty());
    }

    #[test]
    fn descriptions() {
        assert_eq!(describe(&heure("08:00", vec![])), "chaque jour à 08:00");
        assert_eq!(describe(&heure("08:00", vec![Jour::Lun, Jour::Ven])), "lun, ven à 08:00");
        assert_eq!(describe(&Trigger::Intervalle { toutes_les_minutes: 5 }), "toutes les 5 min");
    }
}
