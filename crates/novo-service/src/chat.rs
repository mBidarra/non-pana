//! Chat sobre a thread (RF-MEN-04, 05; RF-AGT-08): conversa lateral, ferramentas só de leitura,
//! propostas que só valem com um clique, resumo publicável na thread.

use crate::views;
use crate::Service;
use anyhow::{Context, Result, bail};
use novo_agent::prompts::{self, CHAT_SCHEMA, REPLY_SCHEMA};
use novo_agent::{Engine, Request};
use novo_core::fsx::{append_jsonl, read_jsonl};
use novo_core::text::{clip, untrusted};
use novo_core::thread::EventBody;
use novo_core::time::{self, Ts};
use novo_graph::{is_superthread, is_thread};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Proposal {
    pub kind: String,
    pub title: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub reason: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatEntry {
    Created { ts: Ts, target: String },
    User { ts: Ts, text: String },
    Agent {
        ts: Ts,
        text: String,
        #[serde(default)]
        proposals: Vec<Proposal>,
        #[serde(default)]
        model: String,
    },
    Applied { ts: Ts, proposal: Proposal, result: String },
    Published { ts: Ts, post: String },
}

pub fn chat_id(target: &str) -> String {
    format!("c-{target}")
}

pub fn chat_id_for(svc: &Service, target: &str) -> Option<String> {
    let id = chat_id(target);
    svc.vault.chat_path(&id).exists().then_some(id)
}

fn entries(svc: &Service, id: &str) -> Vec<ChatEntry> {
    read_jsonl(&svc.vault.chat_path(id)).map(|r| r.0).unwrap_or_default()
}

fn target_of(svc: &Service, id: &str) -> Result<String> {
    entries(svc, id)
        .into_iter()
        .find_map(|e| if let ChatEntry::Created { target, .. } = e { Some(target) } else { None })
        .context("chat não existe")
}

pub async fn open(svc: &Service, target: &str) -> Result<Value> {
    if !(is_thread(target) || is_superthread(target)) {
        bail!("o chat é sobre uma thread ou superthread");
    }
    let id = chat_id(target);
    if !svc.vault.chat_path(&id).exists() {
        let _w = svc.write.lock().await;
        append_jsonl(&svc.vault.chat_path(&id), &ChatEntry::Created { ts: time::now(), target: target.into() })?;
    }
    get(svc, &id)
}

pub fn get(svc: &Service, id: &str) -> Result<Value> {
    let target = target_of(svc, id)?;
    Ok(json!({"id": id, "target": target, "title": views::title_of(svc, &target), "entries": entries(svc, id)}))
}

fn private_in(svc: &Service, target: &str) -> bool {
    let ids = if is_superthread(target) { svc.graph().leaf_threads(target) } else { vec![target.to_string()] };
    ids.iter().any(|t| svc.vault.load_thread(t).ok().flatten().map(|s| s.private).unwrap_or(false))
}

fn context_for(svc: &Service, target: &str) -> Result<(String, Vec<String>, Option<String>)> {
    let cfg = svc.vault.config();
    let g = svc.graph();
    if is_thread(target) {
        let st = svc.thread(target)?;
        let ctx = novo_memory::assemble(&svc.vault, &g, &st, &cfg.context, None)?;
        return Ok((ctx.render(), ctx.used(), st.profile.clone()));
    }
    let mut s = String::new();
    if let Some(b) = novo_memory::load_box(&svc.vault, target)? {
        s.push_str(&format!("## Caixa da superthread\n{}\n", untrusted("memoria:superthread", &b.full())));
    }
    for c in g.children(target) {
        let b = novo_memory::load_box(&svc.vault, &c)?.map(|b| b.l1(400)).unwrap_or_else(|| views::title_of(svc, &c));
        s.push_str(&format!("\n### {c}\n{}\n", untrusted(&format!("memoria:{c}"), &b)));
    }
    Ok((s, g.children(target), None))
}

/// Caminho do binário `novo` para dar ao chat as ferramentas de leitura do MCP.
fn novo_bin() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("NOVO_BIN") {
        return Some(p.into());
    }
    let exe = std::env::current_exe().ok()?;
    let cand = exe.parent()?.join("novo");
    cand.exists().then_some(cand)
}

pub async fn send(svc: &Service, id: &str, text: &str) -> Result<Value> {
    let text = text.trim();
    if text.is_empty() {
        bail!("mensagem vazia");
    }
    let target = target_of(svc, id)?;
    {
        let _w = svc.write.lock().await;
        append_jsonl(&svc.vault.chat_path(id), &ChatEntry::User { ts: time::now(), text: text.into() })?;
    }
    svc.emit("chat", id);
    let (ctx, used, prof) = context_for(svc, &target)?;
    let profile = novo_agent::find_profile(&svc.vault, prof.as_deref())?;
    if private_in(svc, &target) && profile.engine.is_remote() {
        bail!("conteúdo privado: o chat remoto não lê threads privadas");
    }
    novo_agent::check_budget(&svc.vault, &profile, "chat")?;
    let mut history = String::new();
    for e in entries(svc, id) {
        match e {
            ChatEntry::User { text, .. } => history.push_str(&format!("usuário: {text}\n")),
            ChatEntry::Agent { text, .. } => history.push_str(&format!("agente:\n{}\n", untrusted("agente:chat", &text))),
            _ => {}
        }
    }
    let user = format!("{ctx}\n\n## Conversa até agora\n{history}\n\nResponda à última mensagem do usuário.");
    let (system, version) = prompts::load(&svc.vault, "chat");
    let mut req = Request::new("chat", system, user.clone(), Some(CHAT_SCHEMA));
    req.max_tokens = 3000;
    if profile.engine == Engine::ClaudeCode {
        if let Some(bin) = novo_bin() {
            req.mcp_config = Some(
                json!({"mcpServers": {"novo": {"command": bin, "args": ["mcp", "--read-only"], "env": {"NOVO_HOME": svc.vault.root}}}}).to_string(),
            );
            req.allowed_tools = vec!["mcp__novo".into()];
        }
    }
    let res = novo_agent::run(&profile, &req).await;
    let _ = novo_agent::log_run(
        &svc.vault,
        &novo_agent::Run {
            ts: time::now(),
            purpose: "chat".into(),
            trigger: "chat".into(),
            thread: Some(target.clone()),
            profile: profile.name.clone(),
            engine: profile.engine,
            model: res.as_ref().map(|r| r.model.clone()).unwrap_or_else(|_| profile.model.clone()),
            prompt_version: version,
            boxes_used: used,
            input_tokens: res.as_ref().map(|r| r.input_tokens).unwrap_or(0),
            output_tokens: res.as_ref().map(|r| r.output_tokens).unwrap_or(0),
            cost_usd: res.as_ref().map(|r| r.cost_usd).unwrap_or(0.0),
            duration_ms: res.as_ref().map(|r| r.duration_ms).unwrap_or(0),
            ok: res.is_ok(),
            error: res.as_ref().err().map(|e| format!("{e:#}")),
            output_preview: res.as_ref().map(|r| clip(&crate::jobs::preview(r), 300)).unwrap_or_default(),
            sent_chars: user.len(),
        },
    );
    let r = res?;
    let j = r.json.clone().unwrap_or_default();
    let reply = j["reply"].as_str().map(String::from).unwrap_or(r.text.clone());
    let proposals: Vec<Proposal> = serde_json::from_value(j["proposals"].clone()).unwrap_or_default();
    {
        let _w = svc.write.lock().await;
        append_jsonl(&svc.vault.chat_path(id), &ChatEntry::Agent { ts: time::now(), text: reply, proposals, model: r.model })?;
    }
    svc.emit("chat", id);
    get(svc, id)
}

/// Aplica uma proposta do chat — só com o clique do usuário (RF-AGT-08).
pub async fn apply(svc: &Service, id: &str, index: usize) -> Result<Value> {
    let target = target_of(svc, id)?;
    let props: Vec<Proposal> = entries(svc, id).into_iter().filter_map(|e| if let ChatEntry::Agent { proposals, .. } = e { Some(proposals) } else { None }).flatten().collect();
    let p = props.get(index).cloned().context("proposta não existe")?;
    let result = match p.kind.as_str() {
        "task" => {
            let project = if p.target.starts_with("p-") { Some(Some(p.target.clone())) } else { None };
            let v = crate::tasks::create(svc, crate::tasks::TaskInput { title: Some(p.title.clone()), project, relate_to: Some(target.clone()), origin: Some("agent".into()), description: Some(p.reason.clone()), ..Default::default() }).await?;
            format!("tarefa {}", v["task"]["id"].as_str().unwrap_or(""))
        }
        "relation" => {
            crate::superthreads::relate(svc, &target, &p.target).await?;
            format!("relacionado a {}", p.target)
        }
        _ => bail!("tipo de proposta desconhecido"),
    };
    {
        let _w = svc.write.lock().await;
        append_jsonl(&svc.vault.chat_path(id), &ChatEntry::Applied { ts: time::now(), proposal: p, result: result.clone() })?;
    }
    svc.emit("chat", id);
    Ok(json!({"result": result}))
}

/// Publica um resumo da conversa como resposta na thread (RF-MEN-05).
pub async fn publish(svc: &Service, id: &str) -> Result<Value> {
    let target = target_of(svc, id)?;
    if !is_thread(&target) {
        bail!("publicar o resumo vale para threads; numa superthread, use a conversa dela");
    }
    let st = svc.thread(&target)?;
    let profile = novo_agent::find_profile(&svc.vault, st.profile.as_deref())?;
    let mut convo = String::new();
    let mut last_agent = String::new();
    for e in entries(svc, id) {
        match e {
            ChatEntry::User { text, .. } => convo.push_str(&format!("usuário: {text}\n")),
            ChatEntry::Agent { text, .. } => {
                convo.push_str(&format!("agente:\n{}\n", untrusted("agente:chat", &text)));
                last_agent = text;
            }
            _ => {}
        }
    }
    if convo.is_empty() {
        bail!("a conversa está vazia");
    }
    let mut body = last_agent;
    let mut model = profile.model.clone();
    if !(st.private && profile.engine.is_remote()) {
        let (system, _) = prompts::load(&svc.vault, "resposta");
        let req = Request::new("reply", system, format!("## Conversa lateral sobre a thread\n{convo}\n\nPedido: resuma essa conversa em até 5 frases para publicar na thread."), Some(REPLY_SCHEMA));
        if let Ok(r) = novo_agent::run(&profile, &req).await {
            if let Some(b) = r.json.as_ref().and_then(|j| j["body"].as_str()) {
                body = b.to_string();
                model = r.model;
            }
        }
    }
    let _w = svc.write.lock().await;
    let (ev, _) = svc.vault.append_event(
        &target,
        EventBody::AgentReply { author: format!("profile:{}", profile.name), model, mode: "chat_summary".into(), reply_to: None, body: format!("**Resumo do chat:** {body}"), quotes: vec![], usage: None },
        time::now(),
    )?;
    append_jsonl(&svc.vault.chat_path(id), &ChatEntry::Published { ts: time::now(), post: ev.id.clone() })?;
    drop(_w);
    svc.reindex_one(&target);
    svc.emit("thread", &target);
    Ok(json!({"post": ev.id}))
}
