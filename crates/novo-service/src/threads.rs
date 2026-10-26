//! Rede Social: publicar, feed, responder, "Li", agente, fixar, excluir e restaurar (RF-PUB, RF-THR, RF-MEN-02/03).

use crate::jobs::{self, JobKind};
use crate::views::{self, ThreadCard, ThreadView};
use crate::Service;
use anyhow::{Context, Result, bail};
use novo_core::fsx::{read_json, write_json};
use novo_core::text::{canonical_url, find_urls};
use novo_core::thread::{AgentMode, EventBody, Header, Material, ThreadKind};
use novo_core::time;
use novo_core::ThreadState;
use novo_graph::EdgeEvent;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Deserialize, Default, Debug)]
pub struct CreateParams {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub kind: Option<ThreadKind>,
    #[serde(default)]
    pub links: Vec<String>,
    /// Caminhos absolutos de arquivos a anexar (PDF, HTML, MD, TXT).
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub agent: Option<AgentMode>,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub private: bool,
    #[serde(default)]
    pub source: Option<String>,
    /// Projetos ou tarefas para relacionar já na criação.
    #[serde(default)]
    pub relate_to: Vec<String>,
}

impl Service {
    pub fn thread(&self, id: &str) -> Result<ThreadState> {
        self.vault.load_thread(id)?.with_context(|| format!("thread {id} não existe"))
    }
}

fn ensure_agent_allowed(svc: &Service, private: bool, mode: AgentMode, profile: Option<&str>) -> Result<()> {
    if !mode.is_on() || !private {
        return Ok(());
    }
    let p = novo_agent::find_profile(&svc.vault, profile)?;
    if p.engine.is_remote() {
        bail!("thread privada: o agente fica indisponível, porque ela nunca vai a um LLM remoto (RF-PUB-07). Use um perfil local (Ollama).");
    }
    Ok(())
}

pub async fn create(svc: &Service, p: CreateParams) -> Result<ThreadView> {
    let text = p.text.trim().to_string();
    let mut urls: Vec<String> = find_urls(&text);
    // arquivos locais só entram pelo app, pela CLI ou pela pasta monitorada; MCP e extensão só mandam http(s)
    let source = p.source.clone().unwrap_or_else(|| "app".into());
    let local_ok = !source.starts_with("mcp:") && !source.starts_with("extension");
    for l in &p.links {
        let l = l.trim();
        if l.is_empty() {
            continue;
        }
        let lower = l.to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://") || (local_ok && lower.starts_with("file://"))) {
            bail!("link não aceito: {l} (só http, https{})", if local_ok { " ou file" } else { "" });
        }
        urls.push(l.to_string());
    }
    if !local_ok && !p.files.is_empty() {
        bail!("anexar arquivos locais não é permitido por {source}");
    }
    let mut seen = HashSet::new();
    urls.retain(|u| seen.insert(canonical_url(u).unwrap_or_else(|| u.clone())));
    let files: Vec<PathBuf> = p.files.iter().map(PathBuf::from).collect();
    for f in &files {
        if !f.is_file() {
            bail!("arquivo não encontrado: {}", f.display());
        }
    }
    if text.is_empty() && urls.is_empty() && files.is_empty() {
        bail!("escreva algo ou anexe um link ou arquivo");
    }
    let agent = p.agent.unwrap_or_default();
    ensure_agent_allowed(svc, p.private, agent, p.profile.as_deref())?;
    let kind = p.kind.unwrap_or(if urls.is_empty() && files.is_empty() { ThreadKind::Thinking } else { ThreadKind::Reading });
    let mut material: Vec<Material> = urls
        .iter()
        .map(|u| Material { url: u.clone(), canonical_url: canonical_url(u), kind: "link".into(), status: "pending".into(), ..Default::default() })
        .collect();
    let id = novo_core::new_id("t");
    let _w = svc.write.lock().await;
    if !files.is_empty() {
        let dir = svc.vault.thread_dir(&id).join("attachments");
        std::fs::create_dir_all(&dir)?;
        for f in &files {
            let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("arquivo").to_string();
            let dst = dir.join(&name);
            std::fs::copy(f, &dst).with_context(|| format!("copiar {}", f.display()))?;
            material.push(Material { url: format!("file://{}", dst.display()), title: Some(name), kind: "file".into(), status: "pending".into(), ..Default::default() });
        }
    }
    let body = if text.is_empty() { urls.first().cloned().unwrap_or_else(|| "(anexo)".into()) } else { text };
    let header = Header { kind, agent, profile: p.profile.clone(), private: p.private, source, material: material.clone() };
    let st = svc.vault.create_thread(&id, header, &body, time::now())?;
    novo_memory::save_box(&svc.vault, &id, novo_memory::fallback_draft(&st), "fallback")?;
    for target in &p.relate_to {
        let ch = svc.graph().relate(&id, target, || novo_core::new_id("st"))?;
        svc.commit_graph(&ch)?;
    }
    for i in 0..material.len() {
        jobs::enqueue(svc, JobKind::Extract { thread: id.clone(), index: i })?;
    }
    if material.is_empty() && agent.is_on() {
        jobs::enqueue(svc, JobKind::AgentTurn { thread: id.clone(), followup: false, reply_to: None })?;
        svc.vault.append_event(&id, EventBody::AgentStatus { state: "queued".into(), detail: None }, time::now())?;
    } else if agent.is_on() {
        svc.vault.append_event(&id, EventBody::AgentStatus { state: "queued".into(), detail: Some("esperando o material".into()) }, time::now())?;
    }
    drop(_w);
    svc.reindex_one(&id);
    svc.emit("thread", &id);
    let st = svc.thread(&id)?;
    Ok(views::thread_view(svc, &svc.graph(), &st))
}

pub fn get(svc: &Service, id: &str) -> Result<ThreadView> {
    let st = svc.thread(id)?;
    let _ = svc.vault.log_access(id, "open");
    Ok(views::thread_view(svc, &svc.graph(), &st))
}

#[derive(Deserialize, Default, Debug)]
pub struct FeedParams {
    #[serde(default)]
    pub kind: Option<ThreadKind>,
    /// true = só pendentes de "Li"
    #[serde(default)]
    pub pending: Option<bool>,
    /// true = só com agente ligado
    #[serde(default)]
    pub agent: Option<bool>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub ignored: Option<bool>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
    /// O MCP nunca vê threads privadas (RF-MCP-04).
    #[serde(default = "yes")]
    pub include_private: bool,
    #[serde(default)]
    pub since: Option<String>,
}

fn yes() -> bool {
    true
}

pub fn feed(svc: &Service, p: &FeedParams) -> Result<Vec<ThreadCard>> {
    let g = svc.graph();
    let in_project: Option<HashSet<String>> = p.project.as_ref().map(|proj| {
        let mut set = HashSet::new();
        for x in g.related_to(proj) {
            if x.starts_with("st-") {
                set.extend(g.leaf_threads(&x));
            } else {
                set.insert(x);
            }
        }
        set
    });
    let since = p.since.as_deref().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
    let mut cards: Vec<ThreadCard> = svc
        .vault
        .all_threads()
        .iter()
        .filter(|st| p.include_private || !st.private)
        .filter(|st| p.kind.map(|k| k == st.kind).unwrap_or(true))
        .filter(|st| p.pending.map(|want| want == (st.read_at.is_none() && st.has_material())).unwrap_or(true))
        .filter(|st| p.agent.map(|want| want == st.agent.is_on()).unwrap_or(true))
        .filter(|st| in_project.as_ref().map(|s| s.contains(&st.id)).unwrap_or(true))
        .filter(|st| since.map(|s| st.created >= s).unwrap_or(true))
        .map(|st| views::card(svc, &g, st))
        .filter(|c| p.ignored.map(|want| want == c.ignored).unwrap_or(true))
        .collect();
    cards.sort_by(|a, b| b.pinned.cmp(&a.pinned).then(b.created.cmp(&a.created)));
    let off = p.offset.unwrap_or(0);
    let lim = p.limit.unwrap_or(200);
    Ok(cards.into_iter().skip(off).take(lim).collect())
}

/// Liga o agente se o texto menciona @agente ou @<perfil> (RF-AGT-04). Devolve o perfil mencionado.
fn mention(svc: &Service, body: &str) -> Option<Option<String>> {
    let lower = body.to_lowercase();
    if lower.contains("@agente") {
        return Some(None);
    }
    for prof in novo_agent::load_profiles(&svc.vault) {
        if lower.contains(&format!("@{}", prof.name.to_lowercase())) {
            return Some(Some(prof.name));
        }
    }
    None
}

pub async fn reply(svc: &Service, id: &str, body: &str, reply_to: Option<String>) -> Result<ThreadView> {
    let body = body.trim();
    if body.is_empty() {
        bail!("resposta vazia");
    }
    let _w = svc.write.lock().await;
    let st = svc.thread(id)?;
    if let Some(r) = &reply_to {
        if !st.posts.iter().any(|p| &p.id == r) {
            bail!("post {r} não existe nesta thread");
        }
    }
    let (ev, mut st) = svc.vault.append_event(id, EventBody::Post { author: "voce".into(), body: body.into(), reply_to }, time::now())?;
    if let Some(prof) = mention(svc, body) {
        let mode = if st.agent.is_on() { st.agent } else { AgentMode::Opinion };
        ensure_agent_allowed(svc, st.private, mode, prof.as_deref().or(st.profile.as_deref()))?;
        st = svc.vault.append_event(id, EventBody::AgentOn { mode, profile: prof }, time::now())?.1;
    }
    if st.agent.is_on() {
        svc.vault.append_event(id, EventBody::AgentStatus { state: "queued".into(), detail: None }, time::now())?;
        jobs::enqueue(svc, JobKind::AgentTurn { thread: id.into(), followup: true, reply_to: Some(ev.id.clone()) })?;
    }
    let _ = svc.vault.log_access(id, "reply");
    drop(_w);
    svc.reindex_one(id);
    svc.emit("thread", id);
    get(svc, id)
}

pub async fn edit(svc: &Service, id: &str, post: &str, body: &str) -> Result<ThreadView> {
    let _w = svc.write.lock().await;
    let st = svc.thread(id)?;
    if !st.posts.iter().any(|p| p.id == post && !p.is_agent) {
        bail!("só posts seus podem ser editados");
    }
    svc.vault.append_event(id, EventBody::Edit { target: post.into(), body: body.trim().into() }, time::now())?;
    drop(_w);
    svc.reindex_one(id);
    svc.emit("thread", id);
    get(svc, id)
}

pub async fn mark_read(svc: &Service, id: &str, read: bool) -> Result<ThreadCard> {
    let _w = svc.write.lock().await;
    let st = svc.thread(id)?;
    if read != st.read_at.is_some() {
        svc.vault.append_event(id, if read { EventBody::Read } else { EventBody::Unread }, time::now())?;
        if read {
            let _ = svc.vault.log_access(id, "read");
        }
    }
    drop(_w);
    svc.reindex_one(id);
    svc.emit("thread", id);
    Ok(views::card(svc, &svc.graph(), &svc.thread(id)?))
}

pub async fn set_agent(svc: &Service, id: &str, mode: AgentMode, profile: Option<String>) -> Result<ThreadView> {
    let _w = svc.write.lock().await;
    let st = svc.thread(id)?;
    if mode.is_on() {
        ensure_agent_allowed(svc, st.private, mode, profile.as_deref().or(st.profile.as_deref()))?;
        svc.vault.append_event(id, EventBody::AgentOn { mode, profile }, time::now())?;
        if !st.posts.iter().any(|p| p.is_agent && p.mode.as_deref() != Some("error")) {
            svc.vault.append_event(id, EventBody::AgentStatus { state: "queued".into(), detail: None }, time::now())?;
            jobs::enqueue(svc, JobKind::AgentTurn { thread: id.into(), followup: false, reply_to: None })?;
        }
    } else {
        svc.vault.append_event(id, EventBody::AgentOff, time::now())?;
    }
    drop(_w);
    svc.emit("thread", id);
    get(svc, id)
}

/// RF-AGT-13: tentar de novo depois de uma falha.
pub async fn retry(svc: &Service, id: &str) -> Result<ThreadView> {
    let _w = svc.write.lock().await;
    let st = svc.thread(id)?;
    let followup = st.posts.iter().any(|p| p.is_agent && p.mode.as_deref() != Some("error"));
    let reply_to = st.posts.iter().rev().find(|p| !p.is_agent).map(|p| p.id.clone());
    svc.vault.append_event(id, EventBody::AgentStatus { state: "queued".into(), detail: Some("nova tentativa".into()) }, time::now())?;
    jobs::enqueue(svc, JobKind::AgentTurn { thread: id.into(), followup, reply_to })?;
    // material que falhou também tenta de novo
    for (i, m) in st.material.iter().enumerate() {
        if m.status == "failed" {
            jobs::enqueue(svc, JobKind::Extract { thread: id.into(), index: i })?;
        }
    }
    drop(_w);
    svc.emit("thread", id);
    get(svc, id)
}

pub async fn pin(svc: &Service, id: &str, pinned: bool) -> Result<ThreadCard> {
    let _w = svc.write.lock().await;
    svc.vault.append_event(id, if pinned { EventBody::Pin } else { EventBody::Unpin }, time::now())?;
    drop(_w);
    svc.emit("thread", id);
    Ok(views::card(svc, &svc.graph(), &svc.thread(id)?))
}

/// Material colado à mão quando a extração falha.
pub async fn paste_material(svc: &Service, id: &str, index: usize, text: &str) -> Result<ThreadView> {
    let _w = svc.write.lock().await;
    let st = svc.thread(id)?;
    let mut m = st.material.get(index).cloned().context("material não existe")?;
    let dir = svc.vault.thread_dir(id);
    std::fs::create_dir_all(&dir)?;
    let name = format!("snapshot-{index}.md");
    novo_core::fsx::atomic_write(&dir.join(&name), text.as_bytes())?;
    m.snapshot = Some(name);
    m.status = "ok".into();
    m.error = None;
    m.excerpt = Some(novo_core::text::clip(text.trim(), 280));
    svc.vault.append_event(id, EventBody::MaterialUpdate { index, material: m }, time::now())?;
    let st = svc.thread(id)?;
    if st.agent.is_on() && !st.posts.iter().any(|p| p.is_agent && p.mode.as_deref() != Some("error")) {
        jobs::enqueue(svc, JobKind::AgentTurn { thread: id.into(), followup: false, reply_to: None })?;
    }
    drop(_w);
    svc.reindex_one(id);
    svc.emit("thread", id);
    get(svc, id)
}

#[derive(Serialize, Deserialize, Default)]
struct TrashGraph {
    events: Vec<EdgeEvent>,
    dissolved: Vec<String>,
}

/// Excluir (RF-MEN-02/03): vai para a lixeira, sai da superthread, e o que se desfez fica guardado para o desfazer.
pub async fn delete(svc: &Service, id: &str) -> Result<serde_json::Value> {
    let _w = svc.write.lock().await;
    svc.thread(id)?;
    let ch = svc.graph().detach_node(id);
    let parents: Vec<String> = ch.events.iter().filter(|e| e.etype == novo_graph::EdgeType::BlockOf).map(|e| e.to.clone()).collect();
    svc.commit_graph(&ch)?;
    for st in &ch.dissolved {
        if let Some(mut m) = svc.vault.load_superthread(st)? {
            m.dissolved = true;
            svc.vault.save_superthread(&m)?;
        }
    }
    let dir = svc.vault.trash_thread(id, time::day_of(&time::now()))?;
    write_json(&dir.join("graph.json"), &TrashGraph { events: ch.events.clone(), dissolved: ch.dissolved.clone() })?;
    drop(_w);
    svc.reindex_one(id);
    for p in parents.iter().chain(ch.dissolved.iter()) {
        svc.reindex_one(p);
        let _ = jobs::enqueue(svc, JobKind::SuperBox { st: p.clone() });
    }
    svc.emit("thread", id);
    Ok(serde_json::json!({"deleted": id, "dissolved": ch.dissolved, "undo": id}))
}

pub async fn restore(svc: &Service, id: &str) -> Result<ThreadView> {
    let _w = svc.write.lock().await;
    let saved: TrashGraph = svc.vault.trashed_dir(id).and_then(|d| read_json(&d.join("graph.json")).ok()).unwrap_or_default();
    if !svc.vault.restore_thread(id)? {
        bail!("{id} não está na lixeira");
    }
    std::fs::remove_file(svc.vault.thread_path(id).with_extension("").join("graph.json")).ok();
    // desfaz as arestas na ordem inversa, só entre peças que ainda existem (uma vizinha pode estar na lixeira);
    // a estrutura volta inteira ou não volta, e os vínculos com projeto e tarefa voltam um a um
    let exists = |n: &str| -> bool {
        let v = &svc.vault;
        if n.starts_with("st-") {
            v.load_superthread(n).ok().flatten().is_some()
        } else if n.starts_with("t-") {
            v.load_thread(n).ok().flatten().is_some()
        } else if n.starts_with("k-") {
            v.load_task(n).ok().flatten().is_some()
        } else if n.starts_with("p-") {
            v.load_project(n).ok().flatten().is_some()
        } else {
            true
        }
    };
    let inverse: Vec<EdgeEvent> = saved.events.iter().rev().map(|e| e.inverse()).filter(|e| exists(&e.from) && exists(&e.to)).collect();
    let (structure, relations): (Vec<EdgeEvent>, Vec<EdgeEvent>) = inverse.into_iter().partition(|e| e.etype == novo_graph::EdgeType::BlockOf);
    let mut sim = svc.graph();
    for e in &structure {
        sim.apply(e);
    }
    let structure_ok = sim.check().is_ok();
    let mut events = vec![];
    if structure_ok {
        events.extend(structure);
    } else {
        sim = svc.graph();
    }
    for e in relations {
        let mut t = sim.clone();
        t.apply(&e);
        if t.check().is_ok() {
            sim = t;
            events.push(e);
        }
    }
    if !events.is_empty() {
        svc.commit_graph(&novo_graph::Change { events, ..Default::default() })?;
    }
    for st in &saved.dissolved {
        if sim.children(st).len() >= 2 {
            if let Some(mut m) = svc.vault.load_superthread(st)? {
                m.dissolved = false;
                svc.vault.save_superthread(&m)?;
            }
        }
    }
    drop(_w);
    svc.reindex_one(id);
    for e in &saved.events {
        if e.to.starts_with("st-") {
            svc.reindex_one(&e.to);
        }
    }
    svc.emit("thread", id);
    get(svc, id)
}

pub fn trash(svc: &Service) -> Vec<serde_json::Value> {
    svc.vault
        .trash_list()
        .into_iter()
        .map(|(day, id)| {
            let title = svc
                .vault
                .trashed_dir(&id)
                .and_then(|d| novo_core::fsx::read_jsonl::<novo_core::Event>(&d.join(format!("{id}.jsonl"))).ok())
                .and_then(|(evs, _)| ThreadState::fold(&evs))
                .map(|s| s.title())
                .unwrap_or_else(|| id.clone());
            serde_json::json!({"id": id, "day": day, "title": title})
        })
        .collect()
}
