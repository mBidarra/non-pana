//! Relacionar e superthreads (RF-MEN-06 a 09, RF-SUP).

use crate::jobs::{self, JobKind};
use crate::views;
use crate::Service;
use anyhow::{Context, Result, bail};
use novo_core::fsx::{append_jsonl, read_jsonl};
use novo_core::model::SuperthreadMeta;
use novo_core::text::{clip, untrusted};
use novo_core::time::{self, Ts};
use novo_graph::{is_project, is_superthread, is_task, is_thread};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

fn exists(svc: &Service, id: &str) -> bool {
    if is_thread(id) {
        return svc.vault.load_thread(id).ok().flatten().is_some();
    }
    if is_superthread(id) {
        return svc.vault.load_superthread(id).ok().flatten().map(|m| !m.dissolved).unwrap_or(false);
    }
    if is_task(id) {
        return svc.vault.load_task(id).ok().flatten().is_some();
    }
    if is_project(id) {
        return svc.vault.load_project(id).ok().flatten().is_some();
    }
    false
}

/// Relaciona duas peças. Thread ↔ thread monta superthread; projeto e tarefa viram vínculo.
pub async fn relate(svc: &Service, a: &str, b: &str) -> Result<Value> {
    for id in [a, b] {
        if !exists(svc, id) {
            bail!("{id} não existe");
        }
    }
    let _w = svc.write.lock().await;
    let ch = svc.graph().relate(a, b, || novo_core::new_id("st"))?;
    svc.commit_graph(&ch)?;
    if let Some(st) = &ch.created {
        // o título automático aparece em cartões, no MCP e no contexto de outras threads: privada entra sem título
        let label = |n: &str| {
            let private = n.starts_with("t-") && svc.vault.load_thread(n).ok().flatten().map(|s| s.private).unwrap_or(false);
            if private { "(thread privada)".to_string() } else { views::title_of(svc, n) }
        };
        let ta = label(&ch.events[0].from);
        let tb = label(&ch.events[1].from);
        svc.vault.save_superthread(&SuperthreadMeta {
            schema: 1,
            id: st.clone(),
            title: clip(&format!("{} + {}", clip(&ta, 40), clip(&tb, 40)), 90),
            title_by: "auto".into(),
            created: time::now(),
            dissolved: false,
        })?;
    }
    drop(_w);
    let g = svc.graph();
    let mut touched = vec![a.to_string(), b.to_string()];
    if let Some(st) = &ch.created {
        touched.push(st.clone());
    }
    for id in &touched {
        svc.reindex_one(id);
    }
    // a composição mudou: a caixa do conjunto de cima é refeita
    let top = g.top(a);
    if is_superthread(&top) {
        jobs::enqueue(svc, JobKind::SuperBox { st: top.clone() })?;
    }
    svc.emit("graph", a);
    Ok(json!({
        "created": ch.created,
        "superthread": if is_superthread(&top) { Some(top) } else { None },
        "link": if is_project(b) || is_task(b) { Some(b) } else { None },
    }))
}

pub async fn unrelate(svc: &Service, a: &str, b: &str) -> Result<Value> {
    let _w = svc.write.lock().await;
    let ch = svc.graph().unrelate(a, b)?;
    svc.commit_graph(&ch)?;
    drop(_w);
    svc.reindex_one(a);
    svc.emit("graph", a);
    Ok(json!({"ok": true}))
}

pub async fn disassemble(svc: &Service, block: &str) -> Result<Value> {
    let _w = svc.write.lock().await;
    let g = svc.graph();
    let parent = g.parent(block).context("essa peça não está em superthread")?;
    let ch = g.disassemble(block)?;
    svc.commit_graph(&ch)?;
    for st in &ch.dissolved {
        if let Some(mut m) = svc.vault.load_superthread(st)? {
            m.dissolved = true;
            svc.vault.save_superthread(&m)?;
        }
    }
    drop(_w);
    for id in [block.to_string(), parent.clone()].iter().chain(ch.dissolved.iter()) {
        svc.reindex_one(id);
    }
    if !ch.dissolved.contains(&parent) {
        jobs::enqueue(svc, JobKind::SuperBox { st: parent.clone() })?;
    } else if let Some(gp) = svc.graph().parent(&svc.graph().top(block)) {
        jobs::enqueue(svc, JobKind::SuperBox { st: gp })?;
    }
    svc.emit("graph", block);
    Ok(json!({"dissolved": ch.dissolved}))
}

pub async fn reorder(svc: &Service, id: &str, order: &[String]) -> Result<Value> {
    let _w = svc.write.lock().await;
    let ch = svc.graph().reorder(id, order)?;
    svc.commit_graph(&ch)?;
    drop(_w);
    svc.emit("superthread", id);
    Ok(json!({"ok": true}))
}

pub async fn rename(svc: &Service, id: &str, title: &str) -> Result<Value> {
    let title = title.trim();
    if title.is_empty() {
        bail!("título vazio");
    }
    let _w = svc.write.lock().await;
    let mut m = svc.vault.load_superthread(id)?.context("superthread não existe")?;
    m.title = title.into();
    m.title_by = "voce".into();
    svc.vault.save_superthread(&m)?;
    drop(_w);
    svc.reindex_one(id);
    svc.emit("superthread", id);
    Ok(json!({"ok": true}))
}

/// "Li" numa superthread marca todas as threads dela (RF-SUP-11).
pub async fn read_all(svc: &Service, id: &str) -> Result<Value> {
    let leaves = svc.graph().leaf_threads(id);
    for t in &leaves {
        crate::threads::mark_read(svc, t, true).await?;
    }
    Ok(json!({"read": leaves.len()}))
}

fn block_json(svc: &Service, g: &novo_graph::Graph, id: &str, depth: usize) -> Value {
    if is_thread(id) {
        return match svc.vault.load_thread(id).ok().flatten() {
            Some(st) => json!({"kind": "thread", "card": views::card(svc, g, &st), "posts": st.posts}),
            None => json!({"kind": "missing", "id": id}),
        };
    }
    let meta = svc.vault.load_superthread(id).ok().flatten();
    let b = novo_memory::load_box(&svc.vault, id).ok().flatten();
    json!({
        "kind": "superthread",
        "id": id,
        "title": meta.map(|m| m.title).unwrap_or_else(|| id.into()),
        "abstract": b.map(|b| b.abstract_),
        "blocks": if depth < 4 { g.children(id).iter().map(|c| block_json(svc, g, c, depth + 1)).collect::<Vec<_>>() } else { vec![] },
    })
}

pub fn get(svc: &Service, id: &str) -> Result<Value> {
    let meta = svc.vault.load_superthread(id)?.context("superthread não existe")?;
    let g = svc.graph();
    Ok(json!({
        "id": id,
        "title": meta.title,
        "title_by": meta.title_by,
        "created": meta.created,
        "dissolved": meta.dissolved,
        "parent": g.parent(id),
        "blocks": g.children(id).iter().map(|c| block_json(svc, &g, c, 0)).collect::<Vec<_>>(),
        "box": novo_memory::load_box(&svc.vault, id)?,
        "relations": views::relations_of(svc, &g, id),
        "leaf_threads": g.leaf_threads(id),
        "conversation": conversation(svc, id),
        "chat": crate::chat::chat_id_for(svc, id),
    }))
}

pub fn list(svc: &Service) -> Vec<Value> {
    let g = svc.graph();
    let mut v: Vec<Value> = g
        .superthreads()
        .into_iter()
        .filter(|s| g.parent(s).is_none())
        .map(|s| {
            let meta = svc.vault.load_superthread(&s).ok().flatten();
            json!({"id": s, "title": meta.as_ref().map(|m| m.title.clone()).unwrap_or_else(|| s.clone()), "blocks": g.children(&s).len(), "threads": g.leaf_threads(&s).len(), "created": meta.map(|m| m.created)})
        })
        .collect();
    v.sort_by(|a, b| b["created"].as_str().cmp(&a["created"].as_str()));
    v
}

// ------------------------------------------------------------------ conversa da superthread (RF-SUP-10)

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ConvPost {
    pub ts: Ts,
    pub author: String,
    pub body: String,
    #[serde(default)]
    pub model: Option<String>,
}

fn conv_path(svc: &Service, id: &str) -> std::path::PathBuf {
    svc.vault.root.join("superthreads").join(format!("{id}.conversation.jsonl"))
}

pub fn conversation(svc: &Service, id: &str) -> Vec<ConvPost> {
    read_jsonl(&conv_path(svc, id)).map(|r| r.0).unwrap_or_default()
}

pub async fn converse(svc: Arc<Service>, id: &str, body: &str, ask_agent: bool) -> Result<Value> {
    if body.trim().is_empty() {
        bail!("mensagem vazia");
    }
    {
        let _w = svc.write.lock().await;
        append_jsonl(&conv_path(&svc, id), &ConvPost { ts: time::now(), author: "voce".into(), body: body.trim().into(), model: None })?;
    }
    svc.emit("superthread", id);
    if ask_agent {
        let svc2 = svc.clone();
        let id2 = id.to_string();
        tokio::spawn(async move {
            if let Err(e) = agent_conversation(&svc2, &id2).await {
                tracing::warn!("conversa da superthread {id2}: {e:#}");
            }
        });
    }
    Ok(json!({"ok": true}))
}

async fn agent_conversation(svc: &Service, id: &str) -> Result<()> {
    let g = svc.graph();
    let profile = novo_agent::find_profile(&svc.vault, None)?;
    if g.leaf_threads(id).iter().any(|t| svc.vault.load_thread(t).ok().flatten().map(|s| s.private).unwrap_or(false)) && profile.engine.is_remote() {
        bail!("há threads privadas nesta superthread");
    }
    novo_agent::check_budget(&svc.vault, &profile, "reply")?;
    let mut user = String::new();
    if let Some(b) = novo_memory::load_box(&svc.vault, id)? {
        user.push_str(&format!("## Caixa da superthread\n{}\n\n", untrusted("memoria:superthread", &b.full())));
    }
    user.push_str("## Caixas dos blocos\n");
    for c in g.children(id) {
        let b = novo_memory::load_box(&svc.vault, &c)?.map(|b| b.l1(400)).unwrap_or_else(|| views::title_of(svc, &c));
        user.push_str(&format!("\n### {c}\n{}\n", untrusted(&format!("memoria:{c}"), &b)));
    }
    user.push_str("\n## Conversa\n");
    for p in conversation(svc, id) {
        if p.author == "voce" {
            user.push_str(&format!("usuário: {}\n", p.body));
        } else {
            user.push_str(&format!("agente:\n{}\n", untrusted("agente:superthread", &p.body)));
        }
    }
    user.push_str("\nPedido: responda à última mensagem do usuário comparando as threads da superthread.");
    let (system, version) = novo_agent::prompts::load(&svc.vault, "resposta");
    let req = novo_agent::Request::new("reply", system, user.clone(), Some(novo_agent::prompts::REPLY_SCHEMA));
    let res = novo_agent::run(&profile, &req).await;
    let run = novo_agent::Run {
        ts: time::now(),
        purpose: "reply".into(),
        trigger: "superthread".into(),
        thread: Some(id.into()),
        profile: profile.name.clone(),
        engine: profile.engine,
        model: res.as_ref().map(|r| r.model.clone()).unwrap_or_else(|_| profile.model.clone()),
        prompt_version: version,
        boxes_used: g.children(id),
        input_tokens: res.as_ref().map(|r| r.input_tokens).unwrap_or(0),
        output_tokens: res.as_ref().map(|r| r.output_tokens).unwrap_or(0),
        cost_usd: res.as_ref().map(|r| r.cost_usd).unwrap_or(0.0),
        duration_ms: res.as_ref().map(|r| r.duration_ms).unwrap_or(0),
        ok: res.is_ok(),
        error: res.as_ref().err().map(|e| format!("{e:#}")),
        output_preview: res.as_ref().map(|r| clip(&crate::jobs::preview(r), 300)).unwrap_or_default(),
        sent_chars: user.len(),
    };
    let _ = novo_agent::log_run(&svc.vault, &run);
    let r = res?;
    let body = r.json.as_ref().and_then(|j| j["body"].as_str().map(String::from)).unwrap_or(r.text.clone());
    let _w = svc.write.lock().await;
    append_jsonl(&conv_path(svc, id), &ConvPost { ts: time::now(), author: format!("profile:{}", profile.name), body, model: Some(r.model) })?;
    drop(_w);
    svc.emit("superthread", id);
    Ok(())
}
