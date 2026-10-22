//! Raccourcis clavier réglables : un par commande, enregistré sous forme lisible (`Ctrl+N`).

use std::collections::BTreeMap;

use egui::{Key, Modifiers};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Command {
    NewTask,
    Inbox,
    RunSelected,
    StopAll,
    Palette,
    Settings,
    NextUnread,
    PauseAll,
}

impl Command {
    pub const ALL: [Command; 8] = [
        Command::NewTask,
        Command::Inbox,
        Command::RunSelected,
        Command::StopAll,
        Command::Palette,
        Command::Settings,
        Command::NextUnread,
        Command::PauseAll,
    ];

    /// Clé dans `state.json` : stable, ne pas traduire.
    pub fn key(self) -> &'static str {
        match self {
            Command::NewTask => "nouvelle_tache",
            Command::Inbox => "boite",
            Command::RunSelected => "lancer",
            Command::StopAll => "tout_arreter",
            Command::Palette => "palette",
            Command::Settings => "reglages",
            Command::NextUnread => "non_lu_suivant",
            Command::PauseAll => "tout_suspendre",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Command::NewTask => "Nouvelle tâche",
            Command::Inbox => "Boîte de résultats",
            Command::RunSelected => "Lancer la tâche sélectionnée",
            Command::StopAll => "Tout arrêter",
            Command::Palette => "Palette de commandes",
            Command::Settings => "Réglages",
            Command::NextUnread => "Résultat non lu suivant",
            Command::PauseAll => "Tout mettre en pause ou reprendre",
        }
    }

    pub fn default_binding(self) -> Binding {
        let ctrl = Modifiers::COMMAND;
        let ctrl_shift = Modifiers { shift: true, ..Modifiers::COMMAND };
        match self {
            Command::NewTask => Binding::new(ctrl, Key::N),
            Command::Inbox => Binding::new(ctrl, Key::B),
            Command::RunSelected => Binding::new(Modifiers::NONE, Key::F5),
            Command::StopAll => Binding::new(ctrl_shift, Key::X),
            Command::Palette => Binding::new(ctrl, Key::P),
            Command::Settings => Binding::new(ctrl, Key::Comma),
            Command::NextUnread => Binding::new(ctrl, Key::J),
            Command::PauseAll => Binding::new(ctrl_shift, Key::P),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    pub mods: Modifiers,
    pub key: Key,
}

impl Binding {
    pub fn new(mods: Modifiers, key: Key) -> Binding {
        Binding { mods, key }
    }

    /// `Ctrl+Maj+X`.
    pub fn text(&self) -> String {
        let mut parts = Vec::new();
        if self.mods.ctrl || self.mods.command {
            parts.push("Ctrl".to_string());
        }
        if self.mods.alt {
            parts.push("Alt".to_string());
        }
        if self.mods.shift {
            parts.push("Maj".to_string());
        }
        parts.push(self.key.name().to_string());
        parts.join("+")
    }

    pub fn parse(s: &str) -> Option<Binding> {
        let mut mods = Modifiers::NONE;
        let mut key = None;
        for part in s.split('+').map(str::trim) {
            match part.to_lowercase().as_str() {
                "ctrl" => mods = Modifiers { ctrl: true, command: true, ..mods },
                "alt" => mods.alt = true,
                "maj" | "shift" => mods.shift = true,
                _ => key = Some(Key::from_name(part)?),
            }
        }
        Some(Binding { mods, key: key? })
    }

    fn same_mods(&self, m: Modifiers) -> bool {
        (self.mods.ctrl || self.mods.command) == (m.ctrl || m.command)
            && self.mods.alt == m.alt
            && self.mods.shift == m.shift
    }

    pub fn matches(&self, key: Key, m: Modifiers) -> bool {
        self.key == key && self.same_mods(m)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shortcuts {
    map: BTreeMap<Command, Option<Binding>>,
}

impl Default for Shortcuts {
    fn default() -> Self {
        Shortcuts { map: Command::ALL.iter().map(|c| (*c, Some(c.default_binding()))).collect() }
    }
}

impl Shortcuts {
    pub fn get(&self, c: Command) -> Option<Binding> {
        self.map.get(&c).copied().flatten()
    }

    pub fn set(&mut self, c: Command, b: Option<Binding>) {
        self.map.insert(c, b);
    }

    pub fn text(&self, c: Command) -> String {
        self.get(c).map(|b| b.text()).unwrap_or_default()
    }

    /// Commandes qui partagent le raccourci de `c`.
    pub fn conflicts(&self, c: Command) -> Vec<Command> {
        let Some(b) = self.get(c) else { return Vec::new() };
        Command::ALL
            .iter()
            .copied()
            .filter(|o| *o != c && self.get(*o).is_some_and(|ob| ob.key == b.key && ob.same_mods(b.mods)))
            .collect()
    }

    /// Enregistrement : seulement ce qui diffère des valeurs par défaut (`""` pour « aucun »).
    pub fn to_saved(&self) -> BTreeMap<String, String> {
        Command::ALL
            .iter()
            .filter(|c| self.get(**c) != Some(c.default_binding()))
            .map(|c| (c.key().to_string(), self.text(*c)))
            .collect()
    }

    /// Relecture ; ce qui ne se lit pas garde la valeur par défaut et est signalé.
    pub fn from_saved(saved: &BTreeMap<String, String>) -> (Shortcuts, Vec<String>) {
        let mut s = Shortcuts::default();
        let mut problems = Vec::new();
        for (k, v) in saved {
            let Some(c) = Command::ALL.iter().find(|c| c.key() == k) else { continue };
            if v.is_empty() {
                s.set(*c, None);
            } else if let Some(b) = Binding::parse(v) {
                s.set(*c, Some(b));
            } else {
                problems.push(format!("Raccourci illisible pour « {} » : {v} (valeur par défaut gardée).", c.label()));
            }
        }
        (s, problems)
    }

    /// La commande déclenchée par un appui, s'il y en a une.
    pub fn command_for(&self, key: Key, m: Modifiers) -> Option<Command> {
        Command::ALL.iter().copied().find(|c| self.get(*c).is_some_and(|b| b.matches(key, m)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trip() {
        for c in Command::ALL {
            let b = c.default_binding();
            let back = Binding::parse(&b.text()).unwrap();
            assert!(back.matches(b.key, b.mods), "{}", b.text());
        }
        assert_eq!(Command::StopAll.default_binding().text(), "Ctrl+Maj+X");
    }

    #[test]
    fn defaults_have_no_conflict() {
        let s = Shortcuts::default();
        for c in Command::ALL {
            assert!(s.conflicts(c).is_empty(), "{c:?}");
        }
    }

    #[test]
    fn saved_keeps_only_changes_and_reports_garbage() {
        let mut s = Shortcuts::default();
        s.set(Command::Inbox, Binding::parse("Ctrl+I"));
        s.set(Command::Palette, None);
        let saved = s.to_saved();
        assert_eq!(saved.len(), 2);
        assert_eq!(saved["boite"], "Ctrl+I");
        assert_eq!(saved["palette"], "");
        let (back, problems) = Shortcuts::from_saved(&saved);
        assert_eq!(back, s);
        assert!(problems.is_empty());
        let (_, problems) = Shortcuts::from_saved(&BTreeMap::from([("boite".to_string(), "Ctrl+Truc".to_string())]));
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn conflicts_are_found_and_dispatch_works() {
        let mut s = Shortcuts::default();
        s.set(Command::Inbox, Some(Command::NewTask.default_binding()));
        assert_eq!(s.conflicts(Command::Inbox), [Command::NewTask]);
        let m = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
        assert_eq!(s.command_for(Key::P, m), Some(Command::Palette));
        assert_eq!(s.command_for(Key::P, Modifiers::NONE), None);
    }
}
