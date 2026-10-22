//! Faux `claude -p`, pour les essais seulement : imite le flux `stream-json` de Claude Code, sans
//! rien appeler. Comme le vrai :
//! - la consigne arrive en argument ou sur l'entrée standard ;
//! - chaque réponse de Claude donne un événement `assistant` par bloc de contenu, tous avec le même
//!   `message.id` et le même usage, dont les tokens de sortie sont provisoires ;
//! - le coût n'est donné qu'à la fin (`result`), qui peut être `error_max_turns` (`--max-turns`) ou
//!   `error_max_budget_usd` (`--max-budget-usd`).
//!
//! Marqueurs dans la consigne : `[tours:N]` N réponses (une par défaut), `[lent]` 300 ms entre deux,
//! `[gros]` 100 000 tokens d'entrée par réponse, `[ignorer-budget]` ne pas appliquer
//! `--max-budget-usd` (pour que Patronus l'applique), `[ecrire]` crée un fichier, `[erreur]` échec,
//! `[sans-resultat]` fin sans `result`, `[refus]` un appel refusé, `[trace]` résumé qui cite les arguments
//! reçus et la configuration git passée par l'environnement (`GIT_CONFIG_*`).

use std::io::{Read, Write};
use std::time::Duration;

/// Modèle imité (Claude Opus 5.5) et son tarif, en dollars par million de tokens.
const MODEL: &str = "claude-opus-5-5";
const INPUT: f64 = 4.0;
const OUTPUT: f64 = 20.0;

fn marker(prompt: &str, name: &str) -> Option<String> {
    let start = prompt.find(&format!("[{name}"))? + name.len() + 1;
    let end = prompt[start..].find(']')? + start;
    Some(prompt[start..end].trim_start_matches(':').to_string())
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

/// Les arguments reçus, puis la configuration git de l'environnement (`clé=valeur`), une par ligne.
fn trace(args: &[String]) -> String {
    let n: usize = std::env::var("GIT_CONFIG_COUNT").ok().and_then(|c| c.parse().ok()).unwrap_or(0);
    let config = (0..n).map(|i| {
        let get = |k: &str| std::env::var(format!("{k}_{i}")).unwrap_or_default();
        format!("\ngit : {}={}", get("GIT_CONFIG_KEY"), get("GIT_CONFIG_VALUE"))
    });
    format!("\narguments : {}{}", args.join(" "), config.collect::<String>())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Comme `claude -p` : la consigne en argument, sinon sur l'entrée standard.
    let mut prompt = flag(&args, "-p").filter(|a| !a.starts_with("--")).unwrap_or_default();
    if prompt.is_empty() {
        std::io::stdin().read_to_string(&mut prompt).unwrap();
    }
    let first_line = prompt.lines().next().unwrap_or("").to_string();
    let tools: Vec<String> = args
        .iter()
        .skip_while(|a| *a != "--allowedTools")
        .skip(1)
        .take_while(|a| !a.starts_with("--"))
        .cloned()
        .collect();
    let max_turns: Option<u32> = flag(&args, "--max-turns").and_then(|v| v.parse().ok());
    let budget: Option<f64> = flag(&args, "--max-budget-usd").and_then(|v| v.parse().ok());
    let out = std::io::stdout();
    let mut out = out.lock();
    let mut emit = |v: serde_json::Value| {
        writeln!(out, "{v}").unwrap();
        out.flush().unwrap();
    };
    emit(serde_json::json!({"type": "system", "subtype": "init", "session_id": "faux-123", "model": MODEL}));
    if marker(&prompt, "erreur").is_some() {
        eprintln!("erreur simulée");
        std::process::exit(1);
    }
    let turns: u32 = marker(&prompt, "tours").and_then(|n| n.parse().ok()).unwrap_or(1);
    let input_per_turn: u64 = if marker(&prompt, "gros").is_some() { 100_000 } else { 1_000 };
    let (mut cost, mut input_total, mut output_total) = (0.0f64, 0u64, 0u64);
    let mut stop_reason = None;
    for i in 0..turns {
        if max_turns.is_some_and(|m| i >= m) {
            stop_reason = Some("error_max_turns");
            break;
        }
        let id = format!("msg_{i:03}");
        let usage =
            serde_json::json!({"input_tokens": input_per_turn, "cache_read_input_tokens": 0, "output_tokens": 1});
        for block in [
            serde_json::json!({"type": "text", "text": format!("Étape {}.", i + 1)}),
            serde_json::json!({"type": "tool_use", "name": "Read", "input": {"file_path": "README.md"}}),
        ] {
            emit(serde_json::json!({"type": "assistant", "session_id": "faux-123",
                "message": {"id": id, "model": MODEL, "content": [block], "usage": usage}}));
        }
        emit(serde_json::json!({"type": "user", "session_id": "faux-123",
            "message": {"content": [{"type": "tool_result", "content": "contoso"}]}}));
        input_total += input_per_turn;
        output_total += 50;
        cost += (input_per_turn as f64 * INPUT + 50.0 * OUTPUT) / 1_000_000.0;
        if marker(&prompt, "ignorer-budget").is_none() && budget.is_some_and(|b| cost >= b) {
            stop_reason = Some("error_max_budget_usd");
            break;
        }
        if marker(&prompt, "lent").is_some() {
            std::thread::sleep(Duration::from_millis(300));
        }
    }
    if marker(&prompt, "ecrire").is_some() {
        std::fs::write("PATRONUS-ESSAI.md", "Proposition de la session.\n").unwrap();
    }
    if marker(&prompt, "sans-resultat").is_some() {
        return;
    }
    let usage =
        serde_json::json!({"input_tokens": input_total, "cache_read_input_tokens": 0, "output_tokens": output_total});
    let denials = if marker(&prompt, "refus").is_some() {
        serde_json::json!([{"tool_name": "Bash", "tool_use_id": "t1", "tool_input": {"command": "git -C . push"}}])
    } else {
        serde_json::json!([])
    };
    let result = match stop_reason {
        Some(sub) => serde_json::json!({"type": "result", "subtype": sub, "is_error": true,
            "errors": [if sub == "error_max_turns" { "Reached maximum number of turns" } else { "Budget limit reached" }],
            "num_turns": turns, "total_cost_usd": cost, "usage": usage, "permission_denials": denials,
            "session_id": "faux-123"}),
        None => serde_json::json!({"type": "result", "subtype": "success", "is_error": false,
            "result": format!("Résumé du faux : outils {} ; consigne : {first_line}{}", tools.join(" "),
                if marker(&prompt, "trace").is_some() { trace(&args) } else { String::new() }),
            "num_turns": turns, "total_cost_usd": cost, "usage": usage, "permission_denials": denials,
            "session_id": "faux-123"}),
    };
    emit(result);
}
