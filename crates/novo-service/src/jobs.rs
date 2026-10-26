//! Fila em disco (`.queue/`), sobrevive a reinício e a falta de rede (RNF-07, RNF-13).
//! Duas pistas em paralelo: `io` (extração) e `agent` (respostas, caixas, Foco).

use crate::Service;
use anyhow::{Context, Result, anyhow};
use chrono::{Duration, NaiveDate};
use novo_agent::prompts::{self, REPLY_SCHEMA, SUPERTHREAD_SCHEMA};
use novo_agent::{Request, Run};
use novo_core::fsx::{atomic_write, read_json, write_json};
use novo_core::text::{clip, untrusted};
use novo_core::thread::{EventBody, Material, Usage};
use novo_core::time::{self, Ts};
use novo_memory::{BOX_SCHEMA, BoxDraft};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::sync::Arc;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobKind {
    Extract { thread: String, index: usize },
    AgentTurn { thread: String, followup: bool, reply_to: Option<String> },
    Box { thread: String },
    SuperBox { st: String },
    Focus { day: NaiveDate },
}

impl JobKind {
    fn lane(&self) -> &'static str {
        match self {
            JobKind::Extract { .. } => "io",
            _ => "agent",
        }
    }
    fn max_attempts(&self) -> u32 {
        match self {
            JobKind::Extract { .. } => 4, // 1 + 3 novas tentativas (RF-PUB/ING)
            JobKind::AgentTurn { .. } => 2,
            _ => 2,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Job {
    pub id: String,
    #[serde(flatten)]
    pub kind: JobKind,
    pub attempts: u32,
    pub not_before: Option<Ts>,
    pub created: Ts,
    #[serde(default)]
    pub last_error: Option<String>,
}

/// Erro que adia o job sem gastar tentativa (orçamento do dia acabou).
#[derive(Debug)]
pub struct Defer(pub Ts, pub String);
impl fmt::Display for Defer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "adiado até {}: {}", self.0, self.1)
    }
}
impl std::error::Error for Defer {}

/// Jobs rodando agora: um pedido novo igual a um job em andamento não pode ser absorvido por ele,
/// porque o job já leu o estado antigo (e o arquivo some quando ele termina).
static RUNNING: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<String>>> = std::sync::LazyLock::new(Default::default);

pub fn enqueue(svc: &Service, kind: JobKind) -> Result<String> {
    let running = RUNNING.lock().expect("running").clone();
    if let Some(existing) = pending(svc).into_iter().find(|j| j.kind == kind && j.attempts == 0 && !running.contains(&j.id)) {
        return Ok(existing.id);
    }
    let job = Job { id: novo_core::new_id("j"), kind, attempts: 0, not_before: None, created: time::now(), last_error: None };
    write_json(&svc.vault.queue_dir().join(format!("{}.json", job.id)), &job)?;
    svc.queue_wake.notify_waiters();
    svc.emit("queue", &job.id);
    Ok(job.id)
}

pub fn pending(svc: &Service) -> Vec<Job> {
    let mut jobs: Vec<Job> = fs::read_dir(svc.vault.queue_dir())
        .map(|rd| rd.flatten().filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json")).filter_map(|e| read_json::<Job>(&e.path()).ok()).collect())
        .unwrap_or_default();
    jobs.sort_by(|a, b| a.id.cmp(&b.id));
    jobs
}

fn job_path(svc: &Service, id: &str) -> std::path::PathBuf {
    svc.vault.queue_dir().join(format!("{id}.json"))
}

/// Processa o próximo job pronto de uma pista. Devolve `true` se processou algum.
pub async fn tick(svc: &Arc<Service>, lane: &str) -> bool {
    let now = time::now();
    let Some(mut job) = pending(svc).into_iter().find(|j| j.kind.lane() == lane && j.not_before.map(|t| t <= now).unwrap_or(true)) else { return false };
    let last = job.attempts + 1 >= job.kind.max_attempts();
    RUNNING.lock().expect("running").insert(job.id.clone());
    let res = run_job(svc, &job.kind, last).await;
    RUNNING.lock().expect("running").remove(&job.id);
    let path = job_path(svc, &job.id);
    match res {
        Ok(()) => {
            let _ = fs::remove_file(&path);
        }
        Err(e) => {
            if let Some(d) = e.downcast_ref::<Defer>() {
                job.not_before = Some(d.0);
                job.last_error = Some(d.1.clone());
                let _ = write_json(&path, &job);
            } else {
                job.attempts += 1;
                job.last_error = Some(format!("{e:#}"));
                tracing::warn!("job {} ({:?}) falhou: {e:#}", job.id, job.kind);
                if job.attempts >= job.kind.max_attempts() {
                    let _ = fs::remove_file(&path);
                } else {
                    let backoff = [30, 120, 600][(job.attempts as usize - 1).min(2)];
                    job.not_before = Some(time::now() + Duration::seconds(backoff));
                    let _ = write_json(&path, &job);
                }
            }
        }
    }
    svc.emit("queue", &job.id);
    true
}

pub async fn worker(svc: Arc<Service>, lane: &'static str) {
    loop {
        if !tick(&svc, lane).await {
            let wake = svc.queue_wake.notified();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(15), wake).await;
        }
    }
}

/// Roda tudo o que está pronto, nas duas pistas, até a fila esvaziar (testes e `novo run-queue`).
pub async fn drain(svc: &Arc<Service>) {
    loop {
        let a = tick(svc, "io").await;
        let b = tick(svc, "agent").await;
        if !a && !b {
            break;
        }
    }
}

pub async fn run_job(svc: &Service, kind: &JobKind, last: bool) -> Result<()> {
    match kind {
        JobKind::Extract { thread, index } => extract(svc, thread, *index, last).await,
        JobKind::AgentTurn { thread, followup, reply_to } => agent_turn(svc, thread, *followup, reply_to.clone(), last).await,
        JobKind::Box { thread } => rewrite_box(svc, thread).await,
        JobKind::SuperBox { st } => superthread_box(svc, st).await,
        JobKind::Focus { day } => crate::focus_ops::run_day(svc, *day).await.map(|_| ()),
    }
}

// ------------------------------------------------------------------ extração

async fn extract(svc: &Service, thread: &str, index: usize, last: bool) -> Result<()> {
    let st = svc.thread(thread)?;
    let Some(m) = st.material.get(index).cloned() else { return Ok(()) };
    if m.status == "ok" {
        return after_material(svc, thread).await;
    }
    let cfg = svc.vault.config();
    let result: Result<novo_ingest::Extracted> = if let Some(path) = m.url.strip_prefix("file://") {
        let path = std::path::PathBuf::from(path);
        match tokio::task::spawn_blocking(move || novo_ingest::extract_local(&path)).await? {
            Ok(novo_ingest::Local::Content(e)) => Ok(e),
            Ok(novo_ingest::Local::Url(u)) => {
                let client = novo_ingest::client(&cfg.ingest)?;
                novo_ingest::fetch(&client, &u, &cfg.ingest).await
            }
            Err(e) => Err(e),
        }
    } else {
        let client = novo_ingest::client(&cfg.ingest)?;
        novo_ingest::fetch(&client, &m.url, &cfg.ingest).await
    };
    let _w = svc.write.lock().await;
    // excluída durante o download: não recria a pasta (o restaurar precisa dela vazia)
    if svc.vault.load_thread(thread)?.is_none() {
        return Ok(());
    }
    match result {
        Ok(e) => {
            let dir = svc.vault.thread_dir(thread);
            fs::create_dir_all(&dir)?;
            let snap = format!("snapshot-{index}.md");
            atomic_write(&dir.join(&snap), e.markdown.as_bytes())?;
            if let Some(html) = &e.html {
                atomic_write(&dir.join(format!("snapshot-{index}.html")), html.as_bytes())?;
            }
            let nm = Material {
                url: m.url.clone(),
                canonical_url: e.canonical_url.clone().or(m.canonical_url.clone()),
                title: e.title.clone().or(m.title.clone()),
                site: e.site.clone(),
                author: e.author.clone(),
                published: e.published.clone(),
                image: e.image.clone(),
                lang: e.lang.clone(),
                excerpt: e.excerpt.clone(),
                snapshot: Some(snap),
                kind: m.kind.clone(),
                status: "ok".into(),
                error: None,
                attempts: m.attempts + 1,
            };
            svc.vault.append_event(thread, EventBody::MaterialUpdate { index, material: nm }, time::now())?;
            let st = svc.thread(thread)?;
            if let Ok(Some(b)) = novo_memory::load_box(&svc.vault, thread) {
                if b.head.edited_by == "fallback" {
                    novo_memory::save_box(&svc.vault, thread, novo_memory::fallback_draft(&st), "fallback")?;
                }
            }
            drop(_w);
            svc.reindex_one(thread);
            svc.emit("thread", thread);
            after_material(svc, thread).await
        }
        Err(err) => {
            let mut nm = m.clone();
            nm.attempts += 1;
            nm.error = Some(format!("{err:#}"));
            if last {
                nm.status = "failed".into();
                svc.vault.append_event(thread, EventBody::MaterialUpdate { index, material: nm }, time::now())?;
                drop(_w);
                svc.emit("thread", thread);
                return after_material(svc, thread).await;
            }
            svc.vault.append_event(thread, EventBody::MaterialUpdate { index, material: nm }, time::now())?;
            drop(_w);
            svc.emit("thread", thread);
            Err(err)
        }
    }
}

/// Quando nenhum material está pendente e o agente está ligado sem resposta ainda, o primeiro turno entra na fila.
async fn after_material(svc: &Service, thread: &str) -> Result<()> {
    let st = svc.thread(thread)?;
    if st.material.iter().any(|m| m.status == "pending") {
        return Ok(());
    }
    if st.agent.is_on() && !st.posts.iter().any(|p| p.is_agent && p.mode.as_deref() != Some("error")) {
        let already = pending(svc).iter().any(|j| matches!(&j.kind, JobKind::AgentTurn { thread: t, .. } if t == thread));
        if !already {
            enqueue(svc, JobKind::AgentTurn { thread: thread.into(), followup: false, reply_to: None })?;
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ agente da thread

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase().replace(['“', '”', '"', '\'', '’', '‘'], "")
}

/// Só ficam os trechos que aparecem literalmente no material (RF-AGT-05).
pub fn literal_quotes(quotes: &[String], material: &str) -> Vec<String> {
    let hay = norm(material);
    quotes.iter().map(|q| q.trim().trim_matches('"').trim().to_string()).filter(|q| q.chars().count() >= 8 && hay.contains(&norm(q))).collect()
}

fn material_plain(svc: &Service, st: &novo_core::ThreadState) -> String {
    let dir = svc.vault.thread_dir(&st.id);
    st.material
        .iter()
        .map(|m| m.snapshot.as_ref().and_then(|s| fs::read_to_string(dir.join(s)).ok()).or(m.excerpt.clone()).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n\n")
}

async fn set_status(svc: &Service, thread: &str, state: &str, detail: Option<String>) -> Result<()> {
    let _w = svc.write.lock().await;
    svc.vault.append_event(thread, EventBody::AgentStatus { state: state.into(), detail }, time::now())?;
    drop(_w);
    svc.emit("thread", thread);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn record(svc: &Service, purpose: &str, trigger: &str, thread: Option<&str>, p: &novo_agent::Profile, version: &str, used: Vec<String>, sent: usize, res: &Result<novo_agent::Response>) {
    let (ok, err, r) = match res {
        Ok(r) => (true, None, r.clone()),
        Err(e) => (false, Some(format!("{e:#}")), novo_agent::Response::default()),
    };
    let run = Run {
        ts: time::now(),
        purpose: purpose.into(),
        trigger: trigger.into(),
        thread: thread.map(String::from),
        profile: p.name.clone(),
        engine: p.engine,
        model: if r.model.is_empty() { p.model.clone() } else { r.model.clone() },
        prompt_version: version.into(),
        boxes_used: used,
        input_tokens: r.input_tokens,
        output_tokens: r.output_tokens,
        cost_usd: r.cost_usd,
        duration_ms: r.duration_ms,
        ok,
        error: err,
        output_preview: clip(&preview(&r), 300),
        sent_chars: sent,
    };
    let _ = novo_agent::log_run(&svc.vault, &run);
    svc.emit("journal", thread.unwrap_or(""));
}

/// O texto legível da resposta para o diário: o campo principal do JSON, senão o texto cru.
pub fn preview(r: &novo_agent::Response) -> String {
    if let Some(j) = &r.json {
        for k in ["body", "reply", "summary", "abstract", "title"] {
            if let Some(s) = j[k].as_str() {
                return s.to_string();
            }
        }
    }
    r.text.clone()
}

fn tomorrow() -> Ts {
    novo_focus::next_run_after(&time::now())
}

async fn agent_turn(svc: &Service, thread: &str, followup: bool, reply_to: Option<String>, last: bool) -> Result<()> {
    let st = svc.thread(thread)?;
    if !st.agent.is_on() {
        return set_status(svc, thread, "done", None).await;
    }
    let profile = novo_agent::find_profile(&svc.vault, st.profile.as_deref())?;
    if st.private && profile.engine.is_remote() {
        set_status(svc, thread, "failed", Some("thread privada: o agente remoto não lê threads privadas".into())).await?;
        return Ok(());
    }
    if let Err(e) = novo_agent::check_budget(&svc.vault, &profile, "reply") {
        set_status(svc, thread, "queued", Some(e.to_string())).await?;
        return Err(Defer(tomorrow(), e.to_string()).into());
    }
    set_status(svc, thread, "reading", None).await?;
    let cfg = svc.vault.config();
    let ctx = novo_memory::assemble(&svc.vault, &svc.graph(), &st, &cfg.context, None)?;
    let material = material_plain(svc, &st);
    let modes: Vec<&str> = if followup { vec!["followup"] } else { st.agent.reply_modes() };
    let reply_to = reply_to.or_else(|| st.posts.iter().rev().find(|p| !p.is_agent).map(|p| p.id.clone()));
    set_status(svc, thread, "replying", None).await?;
    for mode in modes {
        // numa nova tentativa, o modo que já respondeu a este post não responde de novo (nem gasta de novo)
        if st.posts.iter().any(|p| p.is_agent && p.mode.as_deref() == Some(mode) && p.reply_to == reply_to) {
            continue;
        }
        let (name, label) = match mode {
            "summary" => ("resumo", "resumo"),
            "opinion" => ("opiniao", "opinião"),
            _ => ("resposta", "responder à última mensagem"),
        };
        let (system, version) = prompts::load(&svc.vault, name);
        let mut user = format!("{}\n\n---\nPedido: {label}.", ctx.render());
        let mut attempt = 0;
        let (body, quotes, res_model, usage) = loop {
            let req = Request::new("reply", system.clone(), user.clone(), Some(REPLY_SCHEMA));
            let res = novo_agent::run(&profile, &req).await;
            record(svc, "reply", if followup { "followup" } else { "publish" }, Some(thread), &profile, &version, ctx.used(), user.len(), &res);
            let r = match res {
                Ok(r) => r,
                Err(e) => {
                    if last {
                        let _w = svc.write.lock().await;
                        svc.vault.append_event(
                            thread,
                            EventBody::AgentReply {
                                author: format!("profile:{}", profile.name),
                                model: profile.model.clone(),
                                mode: "error".into(),
                                reply_to: reply_to.clone(),
                                body: format!("Não consegui responder agora: {}", clip(&format!("{e:#}"), 300)),
                                quotes: vec![],
                                usage: None,
                            },
                            time::now(),
                        )?;
                        svc.vault.append_event(thread, EventBody::AgentStatus { state: "failed".into(), detail: Some(clip(&format!("{e:#}"), 200)) }, time::now())?;
                        drop(_w);
                        svc.emit("thread", thread);
                        return Ok(());
                    }
                    return Err(e);
                }
            };
            let json = r.json.clone().unwrap_or_default();
            let body = json["body"].as_str().unwrap_or(&r.text).trim().to_string();
            let raw: Vec<String> = json["quotes"].as_array().map(|a| a.iter().filter_map(|q| q.as_str().map(String::from)).collect()).unwrap_or_default();
            let quotes = literal_quotes(&raw, &material);
            if quotes.is_empty() && !material.trim().is_empty() && mode != "followup" && attempt == 0 {
                attempt += 1;
                user.push_str("\n\nATENÇÃO: em `quotes`, copie trechos EXATAMENTE como aparecem no material, palavra por palavra.");
                continue;
            }
            let usage = Usage { input_tokens: r.input_tokens, output_tokens: r.output_tokens, cost_usd: r.cost_usd };
            break (body, quotes, r.model.clone(), usage);
        };
        let _w = svc.write.lock().await;
        svc.vault.append_event(
            thread,
            EventBody::AgentReply {
                author: format!("profile:{}", profile.name),
                model: res_model,
                mode: mode.into(),
                reply_to: reply_to.clone(),
                body,
                quotes,
                usage: Some(usage),
            },
            time::now(),
        )?;
        drop(_w);
        svc.emit("thread", thread);
    }
    set_status(svc, thread, "done", None).await?;
    svc.reindex_one(thread);
    enqueue(svc, JobKind::Box { thread: thread.into() })?;
    Ok(())
}

// ------------------------------------------------------------------ caixas

/// RF-MEM-02: o agente reescreve a caixa depois do turno; sem agente utilizável, fica a caixa sem LLM.
async fn rewrite_box(svc: &Service, thread: &str) -> Result<()> {
    let st = svc.thread(thread)?;
    let profile = novo_agent::find_profile(&svc.vault, st.profile.as_deref())?;
    if st.private && profile.engine.is_remote() {
        return Ok(());
    }
    if novo_agent::check_budget(&svc.vault, &profile, "box").is_err() {
        return Ok(());
    }
    let prev = novo_memory::load_box(&svc.vault, thread)?;
    let cfg = svc.vault.config();
    let mut user = String::new();
    if let Some(b) = &prev {
        user.push_str(&format!("## Caixa anterior (editada por: {})\n{}\n\n", b.head.edited_by, untrusted("memoria:anterior", &b.full())));
    }
    user.push_str(&format!("## Thread\n{}\n\n", novo_memory::thread_text(&st, cfg.context.thread_chars / 2)));
    let mat = material_plain(svc, &st);
    if !mat.trim().is_empty() {
        user.push_str(&format!("## Material\n{}\n", untrusted("material", &clip(&mat, 8000))));
    }
    let (system, version) = prompts::load(&svc.vault, "caixa");
    let req = Request::new("box", system, user.clone(), Some(BOX_SCHEMA));
    let res = novo_agent::run(&profile, &req).await;
    record(svc, "box", "after_turn", Some(thread), &profile, &version, vec![], user.len(), &res);
    let r = res?;
    let draft: BoxDraft = serde_json::from_value(r.json.clone().context("caixa sem JSON")?).context("caixa fora do schema")?;
    if draft.abstract_.trim().is_empty() {
        return Err(anyhow!("caixa sem resumo de uma linha"));
    }
    let _w = svc.write.lock().await;
    // uma edição sua feita enquanto o agente pensava vale mais que a reescrita dele
    if edited_meanwhile(svc, thread, prev.as_ref().map(|b| b.head.version)) {
        return Ok(());
    }
    novo_memory::save_box(&svc.vault, thread, draft, "agent")?;
    drop(_w);
    svc.reindex_one(thread);
    svc.emit("box", thread);
    if let Some(parent) = svc.graph().parent(thread) {
        enqueue(svc, JobKind::SuperBox { st: parent })?;
    }
    Ok(())
}

/// RF-MEM-05 e RF-SUP-09: caixa e título da superthread a partir das caixas dos blocos.
/// A caixa ganhou uma versão editada por você depois de `before`?
fn edited_meanwhile(svc: &Service, id: &str, before: Option<u32>) -> bool {
    novo_memory::load_box(&svc.vault, id).ok().flatten().map(|b| b.head.edited_by == "voce" && Some(b.head.version) != before).unwrap_or(false)
}

async fn superthread_box(svc: &Service, st_id: &str) -> Result<()> {
    let g = svc.graph();
    let children = g.children(st_id);
    if children.is_empty() {
        return Ok(());
    }
    let Some(meta) = svc.vault.load_superthread(st_id)? else { return Ok(()) };
    let before = novo_memory::load_box(&svc.vault, st_id)?.map(|b| b.head.version);
    let mut agent_title: Option<String> = None;
    let boxes: Vec<(String, Option<novo_memory::MemoryBox>)> = children.iter().map(|c| (c.clone(), novo_memory::load_box(&svc.vault, c).ok().flatten())).collect();
    let any_private = g.leaf_threads(st_id).iter().any(|t| svc.vault.load_thread(t).ok().flatten().map(|s| s.private).unwrap_or(false));
    let profile = novo_agent::find_profile(&svc.vault, None)?;
    let mut draft = novo_memory::fallback_superthread(&meta.title, &boxes);
    let mut by = "fallback";
    if !(any_private && profile.engine.is_remote()) && novo_agent::check_budget(&svc.vault, &profile, "superthread").is_ok() {
        let mut user = String::from("## Caixas dos blocos\n");
        for (id, b) in &boxes {
            user.push_str(&format!("\n### {id}\n{}\n", untrusted(&format!("memoria:{id}"), &b.as_ref().map(|b| b.l1(400)).unwrap_or_default())));
        }
        let (system, version) = prompts::load(&svc.vault, "superthread");
        let req = Request::new("superthread", system, user.clone(), Some(SUPERTHREAD_SCHEMA));
        let res = novo_agent::run(&profile, &req).await;
        record(svc, "superthread", "assemble", Some(st_id), &profile, &version, children.clone(), user.len(), &res);
        if let Ok(r) = res {
            if let Some(j) = r.json {
                let title = j["title"].as_str().unwrap_or("").trim().to_string();
                if !title.is_empty() {
                    agent_title = Some(clip(&title, 80));
                }
                draft = BoxDraft {
                    abstract_: j["abstract"].as_str().unwrap_or(&meta.title).into(),
                    summary: j["summary"].as_str().unwrap_or_default().into(),
                    key_points: j["key_points"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
                    entities: j["entities"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
                    ..Default::default()
                };
                by = "agent";
            }
        }
    }
    // relê depois da chamada: um nome dado por você ou a superthread desfeita no meio-tempo prevalecem
    let _w = svc.write.lock().await;
    let Some(mut cur) = svc.vault.load_superthread(st_id)? else { return Ok(()) };
    if cur.dissolved || svc.graph().children(st_id).len() < 2 {
        return Ok(());
    }
    if let (Some(t), false) = (agent_title, cur.title_by == "voce") {
        cur.title = t;
        cur.title_by = "agent".into();
        svc.vault.save_superthread(&cur)?;
    }
    if !edited_meanwhile(svc, st_id, before) {
        novo_memory::save_box(&svc.vault, st_id, draft, by)?;
    }
    drop(_w);
    svc.reindex_one(st_id);
    svc.emit("superthread", st_id);
    if let Some(parent) = g.parent(st_id) {
        enqueue(svc, JobKind::SuperBox { st: parent })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_literal_quotes_survive() {
        let mat = "Mostre valor antes de pedir o cadastro. Esta é a ideia central.";
        let q = vec!["mostre  valor antes de pedir o cadastro".to_string(), "uma frase inventada pelo modelo".to_string(), "curta".to_string()];
        assert_eq!(literal_quotes(&q, mat), vec!["mostre  valor antes de pedir o cadastro".to_string()]);
    }
}
