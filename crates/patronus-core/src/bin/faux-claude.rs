//! Faux `claude -p`, pour les essais seulement : imite le flux `stream-json` de Claude Code, sans
//! rien appeler. Le comportement se règle par des marqueurs dans la consigne :
//! `[tours:N]` N messages, `[ecrire]` crée un fichier, `[cout:X]` annonce X $ en cours de route,
//! `[budget-cli]` fin « budget atteint », `[erreur]` échec.

use std::io::{Read, Write};
use std::time::Duration;

fn marker(prompt: &str, name: &str) -> Option<String> {
    let start = prompt.find(&format!("[{name}"))? + name.len() + 1;
    let end = prompt[start..].find(']')? + start;
    Some(prompt[start..end].trim_start_matches(':').to_string())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Comme `claude -p` : la consigne en argument, sinon sur l'entrée standard.
    let mut prompt = args
        .iter()
        .position(|a| a == "-p")
        .and_then(|i| args.get(i + 1))
        .filter(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_default();
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
    let out = std::io::stdout();
    let mut out = out.lock();
    let mut emit = |v: serde_json::Value| {
        writeln!(out, "{v}").unwrap();
        out.flush().unwrap();
    };
    emit(serde_json::json!({"type": "system", "subtype": "init", "session_id": "faux-123"}));
    if marker(&prompt, "erreur").is_some() {
        eprintln!("erreur simulée");
        std::process::exit(1);
    }
    let turns: u32 = marker(&prompt, "tours").and_then(|n| n.parse().ok()).unwrap_or(1);
    for i in 0..turns {
        emit(serde_json::json!({"type": "assistant", "session_id": "faux-123", "message": {"content": [
            {"type": "text", "text": format!("Étape {}.", i + 1)},
            {"type": "tool_use", "name": "Read", "input": {"file_path": "README.md"}}
        ]}}));
        if turns > 1 {
            std::thread::sleep(Duration::from_millis(150));
        }
    }
    if let Some(cost) = marker(&prompt, "cout") {
        emit(serde_json::json!({"type": "progress", "total_cost_usd": cost.parse::<f64>().unwrap_or(0.0)}));
        std::thread::sleep(Duration::from_secs(5));
    }
    if marker(&prompt, "ecrire").is_some() {
        std::fs::write("PATRONUS-ESSAI.md", "Proposition de la session.\n").unwrap();
    }
    let budget = marker(&prompt, "budget-cli").is_some();
    emit(serde_json::json!({
        "type": "result",
        "subtype": if budget { "error_max_budget_usd" } else { "success" },
        "is_error": budget,
        "result": format!("Résumé du faux : outils {} ; consigne : {first_line}", tools.join(" ")),
        "total_cost_usd": 0.0123,
        "usage": {"input_tokens": 100, "cache_read_input_tokens": 900, "output_tokens": 50},
        "session_id": "faux-123"
    }));
}
