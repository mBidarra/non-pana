//! Formas que a API devolve para a interface, o CLI e o MCP.

use crate::Service;
use novo_core::text::clip;
use novo_core::thread::{AgentMode, PostView, ThreadKind};
use novo_core::time::{self, Ts};
use novo_core::ThreadState;
use novo_graph::{EdgeType, Graph};
use novo_memory::MemoryBox;
use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
pub struct MaterialCard {
    pub url: String,
    pub title: Option<String>,
    pub site: Option<String>,
    pub image: Option<String>,
    pub excerpt: Option<String>,
    pub status: String,
    pub error: Option<String>,
    pub kind: String,
    pub has_snapshot: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct RelRef {
    #[serde(rename = "type")]
    pub rtype: String,
    pub id: String,
    pub title: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct ThreadCard {
    pub id: String,
    pub title: String,
    pub kind: ThreadKind,
    pub created: Ts,
    pub last_activity: Ts,
    pub age_hours: i64,
    pub read: bool,
    pub read_at: Option<Ts>,
    pub agent: AgentMode,
    pub agent_state: Option<String>,
    pub agent_detail: Option<String>,
    pub profile: Option<String>,
    pub private: bool,
    pub pinned: bool,
    pub source: String,
    pub material: Vec<MaterialCard>,
    pub excerpt: String,
    pub replies: usize,
    pub agent_replies: usize,
    pub superthread: Option<String>,
    pub superthread_title: Option<String>,
    pub relations: Vec<RelRef>,
    pub ignored: bool,
    pub has_material: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct ThreadView {
    #[serde(flatten)]
    pub card: ThreadCard,
    pub posts: Vec<PostView>,
    #[serde(rename = "box")]
    pub memory: Option<MemoryBox>,
    pub chat: Option<String>,
}

pub fn title_of(svc: &Service, id: &str) -> String {
    if id.starts_with("t-") {
        return svc.vault.load_thread(id).ok().flatten().map(|s| s.title()).unwrap_or_else(|| id.to_string());
    }
    if id.starts_with("st-") {
        return svc.vault.load_superthread(id).ok().flatten().map(|m| m.title).unwrap_or_else(|| id.to_string());
    }
    if id.starts_with("k-") {
        return svc.vault.load_task(id).ok().flatten().map(|t| t.head.title).unwrap_or_else(|| id.to_string());
    }
    if id.starts_with("p-") {
        return svc.vault.load_project(id).ok().flatten().map(|p| p.head.name).unwrap_or_else(|| id.to_string());
    }
    id.to_string()
}

pub fn relations_of(svc: &Service, g: &Graph, id: &str) -> Vec<RelRef> {
    g.relations(id)
        .into_iter()
        .map(|(t, to)| RelRef { rtype: if t == EdgeType::RelatesProject { "project".into() } else { "task".into() }, title: title_of(svc, &to), id: to })
        .collect()
}

pub fn card(svc: &Service, g: &Graph, st: &ThreadState) -> ThreadCard {
    let cfg = svc.vault.config();
    let now = time::now();
    let dir = svc.vault.thread_dir(&st.id);
    let parent = g.parent(&st.id);
    let user_posts = st.posts.iter().filter(|p| !p.is_agent).count();
    ThreadCard {
        id: st.id.clone(),
        title: st.title(),
        kind: st.kind,
        created: st.created,
        last_activity: st.last_activity,
        age_hours: (now - st.created).num_hours(),
        read: st.read_at.is_some(),
        read_at: st.read_at,
        agent: st.agent,
        agent_state: st.agent_state.clone(),
        agent_detail: st.agent_detail.clone(),
        profile: st.profile.clone(),
        private: st.private,
        pinned: st.pinned,
        source: st.source.clone(),
        material: st
            .material
            .iter()
            .map(|m| MaterialCard {
                url: m.url.clone(),
                title: m.title.clone(),
                site: m.site.clone(),
                image: m.image.clone(),
                excerpt: m.excerpt.clone(),
                status: m.status.clone(),
                error: m.error.clone(),
                kind: m.kind.clone(),
                has_snapshot: m.snapshot.as_ref().map(|s| dir.join(s).exists()).unwrap_or(false),
            })
            .collect(),
        excerpt: st.root().map(|p| clip(p.body.trim(), 400)).unwrap_or_default(),
        replies: user_posts.saturating_sub(1),
        agent_replies: st.posts.iter().filter(|p| p.is_agent).count(),
        superthread_title: parent.as_ref().map(|p| title_of(svc, p)),
        superthread: parent,
        relations: relations_of(svc, g, &st.id),
        ignored: novo_focus::thread_ignored_at(st, &now, &cfg),
        has_material: st.has_material(),
    }
}

pub fn thread_view(svc: &Service, g: &Graph, st: &ThreadState) -> ThreadView {
    ThreadView {
        card: card(svc, g, st),
        posts: st.posts.clone(),
        memory: novo_memory::load_box(&svc.vault, &st.id).ok().flatten(),
        chat: crate::chat::chat_id_for(svc, &st.id),
    }
}
