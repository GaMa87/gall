//! A deterministic stand-in for a local model, used by `tests/compare_ai.rs` and as the offline smoke test of the
//! protocol in `docs/compare-local-ai.md`. It reads the request from standard input and answers by its first
//! argument (`mode`). It has no model and sends nothing anywhere.
//!
//! Modes: `ok` (a valid answer with evidence), `echo` (the first removed line is in the summary), `sleep` (waits
//! for a minute), `malformed`, `badevidence`, `unknownfield`, `huge`, `fail`, `env` (reports whether
//! `COMPARE_AI_SECRET` is visible), `merge*` (proposals for a merge conflict: `merge`, `mergeecho`, `mergemarkers`, `mergestale`, `mergebig`, `mergeevidence`, `mergesyntax`, `mergecontradict`), `count` (appends a line to the file named by `COMPARE_AI_COUNT`, then `ok`).

use std::io::{Read, Write};

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "ok".into());
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let request: serde_json::Value =
        serde_json::from_str(&input).unwrap_or(serde_json::Value::Null);
    let first = &request["hunks"][0];
    let first_id = first["id"].as_u64().unwrap_or(0);
    let removed = first["left"]["lines"][0].as_str().unwrap_or("").to_string();
    let right_lines_len = first["right"]["lines"].as_array().map_or(0, Vec::len);
    let (ev_side, ev_start) = if right_lines_len > 0 {
        ("right", first["right"]["start"].as_u64().unwrap_or(1))
    } else {
        ("left", first["left"]["start"].as_u64().unwrap_or(1))
    };
    let answer = |summary: &str, evidence: serde_json::Value| {
        serde_json::json!({
            "protocol": 1,
            "summary": summary,
            "why": "synthetic",
            "risk": "none",
            "uncertainty": "this is a fake provider",
            "category": "refactor",
            "confidence": 0.5,
            "items": [{ "hunk": first_id, "category": "formatting", "confidence": 0.25, "note": "n" }],
            "evidence": evidence
        })
    };
    let good_evidence = serde_json::json!([{ "hunk": first_id, "side": ev_side, "startLine": ev_start, "endLine": ev_start }]);
    let out = match mode.as_str() {
        "ok" => answer("fine", good_evidence).to_string(),
        "echo" => answer(&format!("{removed}|{}", request["instructions"].as_str().unwrap_or("")), good_evidence).to_string(),
        "sleep" => {
            std::thread::sleep(std::time::Duration::from_secs(60));
            answer("late", good_evidence).to_string()
        }
        "malformed" => "this is not json".to_string(),
        "badevidence" => answer("x", serde_json::json!([{ "hunk": 0, "side": "right", "startLine": 9999, "endLine": 10000 }])).to_string(),
        "unknownfield" => {
            let mut a = answer("x", good_evidence);
            a["tool_call"] = serde_json::json!("rm -rf /");
            a.to_string()
        }
        "huge" => "x".repeat(2 * 1024 * 1024),
        "fail" => {
            eprintln!("model not loaded");
            std::process::exit(3);
        }
        "env" => answer(if std::env::var_os("COMPARE_AI_SECRET").is_some() { "set" } else { "unset" }, good_evidence).to_string(),
        m if m.starts_with("merge") => {
            let c = &request["conflict"];
            let lines = |key: &str| -> Vec<String> {
                c[key].as_array().map(|a| a.iter().filter_map(|l| l.as_str().map(String::from)).collect()).unwrap_or_default()
            };
            let (left, right, base) = (lines("leftLines"), lines("rightLines"), lines("baseLines"));
            let mut replacement: Vec<String> = left.iter().chain(right.iter()).cloned().collect();
            let mut revision = request["revision"].as_str().unwrap_or("").to_string();
            let mut evidence = serde_json::json!([{ "source": "left", "startLine": 1, "endLine": left.len().max(1) }]);
            let mut summary = "combined".to_string();
            match m {
                "mergeecho" => summary = format!("{}|{}", left.first().cloned().unwrap_or_default(), request["instructions"].as_str().unwrap_or("")),
                "mergemarkers" => replacement.push("<<<<<<< ours".into()),
                "mergestale" => revision = "0000000000000000".into(),
                "mergebig" => replacement = (0..5000).map(|i| format!("line {i}")).collect(),
                "mergeevidence" => evidence = serde_json::json!([{ "source": "right", "startLine": 50, "endLine": 60 }]),
                "mergesyntax" => replacement = vec!["fn broken( {".into()],
                "mergecontradict" => {
                    replacement = base.clone();
                    summary = "kept the base: the sides contradict each other".into();
                }
                _ => {}
            }
            serde_json::json!({
                "protocol": 1,
                "revision": revision,
                "replacement": replacement,
                "explanation": summary,
                "risk": "check both sides",
                "uncertainty": "synthetic",
                "confidence": 0.5,
                "evidence": evidence
            })
            .to_string()
        }
        "orphan" => {
            // A wrapper that starts the model in the background and exits: the child inherits standard output.
            if let Ok(exe) = std::env::current_exe() {
                let _ = std::process::Command::new(exe).arg("sleep").spawn();
            }
            answer("answered", good_evidence).to_string()
        }
        "dupitems" => {
            let mut a = answer("x", good_evidence);
            a["items"] = serde_json::json!([
                { "hunk": 0, "category": "bugfix", "confidence": 0.5, "note": "" },
                { "hunk": 0, "category": "feature", "confidence": 0.5, "note": "" }
            ]);
            a.to_string()
        }
        "count" => {
            if let Some(path) = std::env::var_os("COMPARE_AI_COUNT") {
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                    let _ = writeln!(f, "call");
                }
            }
            answer("counted", good_evidence).to_string()
        }
        _ => "unknown mode".to_string(),
    };
    let _ = std::io::stdout().write_all(out.as_bytes());
}
