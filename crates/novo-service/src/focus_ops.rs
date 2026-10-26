//! Sessão Foco e Observabilidade no serviço (RF-FOC, RF-OBS, RF-AGT-09).

use crate::views;
use crate::Service;
use anyhow::Result;
use chrono::NaiveDate;
use novo_agent::Request;
use novo_agent::prompts::{self, FOCUS_SCHEMA};
use novo_core::text::{clip, untrusted};
use novo_core::time;
use novo_focus::FocusReport;
use serde_json::{Value, json};

/// Calcula o dia (código), pede resumo e temas ao agente (se der) e grava. Idempotente (RF-FOC-06).
pub async fn run_day(svc: &Service, day: NaiveDate) -> Result<FocusReport> {
    let mut r = novo_focus::compute(&svc.vault, day)?;
    let cfg = svc.vault.config();
    let (start, end) = time::day_window(day);
    let profile = novo_agent::find_profile(&svc.vault, if cfg.focus.profile.is_empty() { None } else { Some(cfg.focus.profile.as_str()) });
    let quiet = r.counts.threads_created == 0 && r.counts.user_posts == 0 && r.counts.reads == 0 && r.ignored_threads.is_empty() && r.ignored_tasks.is_empty();
    if quiet {
        r.summary = "Dia sem atividade.".into();
    }
    if let (Ok(profile), false) = (profile, quiet) {
        // caixas das threads do dia (sem as privadas, se o motor for remoto)
        let mut boxes = String::new();
        let mut used = vec![];
        for st in svc.vault.all_threads() {
            let active = st.last_activity >= start && st.created < end;
            if !active || (st.private && profile.engine.is_remote()) {
                continue;
            }
            if let Ok(Some(b)) = novo_memory::load_box(&svc.vault, &st.id) {
                boxes.push_str(&format!("\n### {}\n{}\n", st.id, untrusted(&format!("memoria:{}", st.id), &b.l1(200))));
                used.push(st.id.clone());
            }
            if boxes.len() > 30_000 {
                break;
            }
        }
        let user = format!(
            "Dia: {}\nNúmeros (fixos): {}\nIgnorados: {}\n\n## Caixas das threads do dia\n{}",
            day,
            serde_json::to_string(&r.counts)?,
            r.ignored_threads.iter().filter(|t| !(t.private && profile.engine.is_remote())).map(|t| clip(&t.title, 80)).chain(r.ignored_tasks.iter().map(|t| format!("tarefa: {}", t.title))).collect::<Vec<_>>().join("; "),
            if boxes.is_empty() { "(nenhuma thread no dia)".into() } else { boxes }
        );
        let (system, version) = prompts::load(&svc.vault, "foco");
        let req = Request::new("focus", system, user.clone(), Some(FOCUS_SCHEMA));
        let res = novo_agent::run(&profile, &req).await;
        let run = novo_agent::Run {
            ts: time::now(),
            purpose: "focus".into(),
            trigger: "schedule".into(),
            thread: None,
            profile: profile.name.clone(),
            engine: profile.engine,
            model: res.as_ref().map(|x| x.model.clone()).unwrap_or_else(|_| profile.model.clone()),
            prompt_version: version,
            boxes_used: used,
            input_tokens: res.as_ref().map(|x| x.input_tokens).unwrap_or(0),
            output_tokens: res.as_ref().map(|x| x.output_tokens).unwrap_or(0),
            cost_usd: res.as_ref().map(|x| x.cost_usd).unwrap_or(0.0),
            duration_ms: res.as_ref().map(|x| x.duration_ms).unwrap_or(0),
            ok: res.is_ok(),
            error: res.as_ref().err().map(|e| format!("{e:#}")),
            output_preview: res.as_ref().map(|x| clip(&crate::jobs::preview(x), 300)).unwrap_or_default(),
            sent_chars: user.len(),
        };
        let _ = novo_agent::log_run(&svc.vault, &run);
        if let Ok(x) = res {
            if let Some(j) = x.json {
                r.summary = j["summary"].as_str().unwrap_or_default().to_string();
                r.themes = j["themes"].as_array().map(|a| a.iter().filter_map(|t| t.as_str().map(|s| s.to_lowercase())).collect()).unwrap_or_default();
                r.agent = Some(format!("{} ({})", profile.name, x.model));
            }
        }
    }
    let _w = svc.write.lock().await;
    novo_focus::save(&svc.vault, &r)?;
    drop(_w);
    svc.emit("focus", &day.to_string());
    Ok(r)
}

/// Dias faltantes (até `catch_up_days`), do mais antigo ao mais novo (RF-FOC-05).
pub async fn catch_up(svc: &Service) -> Result<Vec<NaiveDate>> {
    let cfg = svc.vault.config();
    if !cfg.focus.enabled {
        return Ok(vec![]);
    }
    let today = time::day_of(&time::now());
    let born = svc.vault.created_day();
    let days: Vec<NaiveDate> = novo_focus::missing_days(&svc.vault, today, cfg.focus.catch_up_days).into_iter().filter(|d| *d >= born).collect();
    for d in &days {
        run_day(svc, *d).await?;
    }
    Ok(days)
}

/// O track "O que só foi ignorado", agora (RF-OBS-01: atualizado quando você age).
pub fn ignored_now(svc: &Service, include_private: bool) -> Value {
    let cfg = svc.vault.config();
    let now = time::now();
    let g = svc.graph();
    let threads: Vec<_> = svc.vault.all_threads().into_iter().filter(|s| include_private || !s.private).collect();
    let it = novo_focus::ignored_threads(&threads, &now, &cfg);
    let ik = novo_focus::ignored_tasks(&svc.vault.tasks(), &now, &cfg);
    let cards: Vec<Value> = it
        .iter()
        .filter_map(|i| threads.iter().find(|s| s.id == i.id))
        .map(|st| {
            let summary = novo_memory::load_box(&svc.vault, &st.id).ok().flatten().filter(|b| b.head.edited_by == "agent").map(|b| b.abstract_);
            json!({"card": views::card(svc, &g, st), "summary": summary})
        })
        .collect();
    let last = novo_focus::list_days(&svc.vault).last().copied();
    json!({"threads": cards, "tasks": ik, "as_of": now, "last_report": last,
           "rules": {"thread_hours": cfg.ignored.thread_hours, "task_days": cfg.ignored.task_days, "only_with_material": cfg.ignored.only_with_material}})
}

/// Série por dia para o gráfico (RF-OBS-05) e os tracks de depois (RF-OBS-07, 08).
pub fn series(svc: &Service, days: usize) -> Value {
    let mut rows = vec![];
    for d in novo_focus::list_days(&svc.vault).into_iter().rev().take(days) {
        if let Ok(Some(r)) = novo_focus::load(&svc.vault, d) {
            rows.push(json!({
                "date": d, "ignored_threads": r.ignored_threads.len(), "ignored_tasks": r.ignored_tasks.len(),
                "fomo_7d": r.fomo_7d, "reads": r.counts.reads, "threads_created": r.counts.threads_created,
                "tasks_completed": r.counts.tasks_completed,
            }));
        }
    }
    rows.reverse();
    let latest = novo_focus::list_days(&svc.vault).last().and_then(|d| novo_focus::load(&svc.vault, *d).ok().flatten());
    json!({"series": rows, "latest": latest})
}
