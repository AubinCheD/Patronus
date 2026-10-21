//! Coût « équivalent API » estimé pendant une session Claude, d'après les tokens que chaque message
//! annonce. Claude Code ne donne le coût exact (`total_cost_usd`) qu'à la fin : cette estimation sert
//! à arrêter une session qui dépasse son budget avant qu'elle ne se termine. Claude Code applique lui
//! aussi le budget (`--max-budget-usd`) ; l'estimation est le second garde-fou.
//!
//! Tarifs publics par million de tokens (référence du 06/10/2026). L'écriture en cache est comptée à
//! 1,25 × l'entrée (durée de 5 minutes, celle par défaut).

/// Tarif d'un modèle, en dollars par million de tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
}

impl Price {
    pub fn cache_write(&self) -> f64 {
        self.input * 1.25
    }
}

/// Préfixes d'identifiants de modèle, du plus précis au plus général.
const PRICES: [(&str, Price); 12] = [
    ("claude-fable-5-1", Price { input: 10.0, output: 50.0, cache_read: 0.25 }),
    ("claude-mythos-5-1", Price { input: 10.0, output: 50.0, cache_read: 0.25 }),
    ("claude-fable-5", Price { input: 10.0, output: 50.0, cache_read: 1.0 }),
    ("claude-opus-5-5", Price { input: 4.0, output: 20.0, cache_read: 0.20 }),
    ("claude-opus-5", Price { input: 5.0, output: 25.0, cache_read: 0.50 }),
    ("claude-opus-4", Price { input: 5.0, output: 25.0, cache_read: 0.50 }),
    ("claude-sonnet-5-5", Price { input: 2.0, output: 10.0, cache_read: 0.20 }),
    ("claude-sonnet-5", Price { input: 2.0, output: 10.0, cache_read: 0.20 }),
    ("claude-sonnet-4", Price { input: 3.0, output: 15.0, cache_read: 0.30 }),
    ("claude-haiku-5-5", Price { input: 0.10, output: 0.50, cache_read: 0.01 }),
    ("claude-haiku-5", Price { input: 0.10, output: 0.50, cache_read: 0.01 }),
    ("claude-haiku-4", Price { input: 1.0, output: 5.0, cache_read: 0.10 }),
];

/// Le tarif d'un modèle (`claude-opus-5-5`, `claude-opus-5-5[1m]`…), ou rien s'il est inconnu :
/// mieux vaut ne pas estimer que d'estimer faux.
pub fn price_for(model: &str) -> Option<Price> {
    let m = model.trim().to_lowercase();
    // Le plus long préfixe qui correspond, suivi d'une fin ou d'un séparateur (`claude-opus-5` ne doit
    // pas attraper `claude-opus-5-5`, déjà traité avant, ni un `claude-opus-50` imaginaire).
    PRICES
        .iter()
        .filter(|(p, _)| m.starts_with(p) && m[p.len()..].chars().next().is_none_or(|c| !c.is_ascii_digit()))
        .max_by_key(|(p, _)| p.len())
        .map(|(_, price)| *price)
}

/// Tokens d'un message.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    pub input: u64,
    pub cache_write: u64,
    pub cache_read: u64,
    pub output: u64,
}

impl Usage {
    pub fn from_json(v: &serde_json::Value) -> Usage {
        let n = |k: &str| v[k].as_u64().unwrap_or(0);
        Usage {
            input: n("input_tokens"),
            cache_write: n("cache_creation_input_tokens"),
            cache_read: n("cache_read_input_tokens"),
            output: n("output_tokens"),
        }
    }

    /// Le plus grand des deux, champ par champ : un même message annoncé plusieurs fois (un
    /// événement par bloc de contenu) ne compte qu'une fois, à son dernier état.
    pub fn max(self, o: Usage) -> Usage {
        Usage {
            input: self.input.max(o.input),
            cache_write: self.cache_write.max(o.cache_write),
            cache_read: self.cache_read.max(o.cache_read),
            output: self.output.max(o.output),
        }
    }
}

pub fn cost(price: Price, u: Usage) -> f64 {
    (u.input as f64 * price.input
        + u.cache_write as f64 * price.cache_write()
        + u.cache_read as f64 * price.cache_read
        + u.output as f64 * price.output)
        / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longest_prefix_wins() {
        assert_eq!(price_for("claude-opus-5-5").unwrap().input, 4.0);
        assert_eq!(price_for("claude-opus-5-5[1m]").unwrap().input, 4.0);
        assert_eq!(price_for("claude-opus-5").unwrap().input, 5.0);
        assert_eq!(price_for("claude-opus-4-8").unwrap().input, 5.0);
        assert_eq!(price_for("claude-sonnet-4-6").unwrap().output, 15.0);
        assert_eq!(price_for("claude-haiku-5-5").unwrap().cache_read, 0.01);
        assert_eq!(price_for("claude-fable-5-1").unwrap().cache_read, 0.25);
        assert_eq!(price_for("CLAUDE-FABLE-5").unwrap().cache_read, 1.0);
        assert!(price_for("claude-opus-50").is_none());
        assert!(price_for("gpt-x").is_none());
    }

    #[test]
    fn cost_of_a_message() {
        let p = price_for("claude-opus-5-5").unwrap();
        let u = Usage { input: 1_000, cache_write: 10_000, cache_read: 100_000, output: 2_000 };
        // 1k × 4 + 10k × 5 + 100k × 0,2 + 2k × 20, par million.
        let expected = (4_000.0 + 50_000.0 + 20_000.0 + 40_000.0) / 1_000_000.0;
        assert!((cost(p, u) - expected).abs() < 1e-12);
    }

    #[test]
    fn usage_is_read_and_merged() {
        let v = serde_json::json!({"input_tokens": 3, "cache_read_input_tokens": 7, "output_tokens": 1});
        let a = Usage::from_json(&v);
        assert_eq!(a, Usage { input: 3, cache_write: 0, cache_read: 7, output: 1 });
        let b = Usage { output: 40, ..a };
        assert_eq!(a.max(b).output, 40);
    }
}
