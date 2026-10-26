//! Threads: um arquivo JSONL só com acréscimo; o estado é derivado dobrando os eventos.

use crate::time::Ts;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThreadKind {
    #[default]
    Thinking,
    Reading,
    Watching,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentMode {
    #[default]
    Off,
    Summary,
    Opinion,
    Both,
}

impl AgentMode {
    pub fn is_on(self) -> bool {
        self != AgentMode::Off
    }
    /// Os modos de resposta a gerar num primeiro turno.
    pub fn reply_modes(self) -> Vec<&'static str> {
        match self {
            AgentMode::Off => vec![],
            AgentMode::Summary => vec!["summary"],
            AgentMode::Opinion => vec!["opinion"],
            AgentMode::Both => vec!["summary", "opinion"],
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Material {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,
    /// Caminho relativo à pasta de anexos da thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
    /// `file` para anexos locais (PDF, HTML, MD), `link` para URLs.
    #[serde(default = "default_material_kind")]
    pub kind: String,
    /// pending | ok | failed
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub attempts: u32,
}

fn default_status() -> String {
    "pending".into()
}
fn default_material_kind() -> String {
    "link".into()
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Header {
    pub kind: ThreadKind,
    #[serde(default)]
    pub agent: AgentMode,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub private: bool,
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default)]
    pub material: Vec<Material>,
}

fn default_source() -> String {
    "app".into()
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cost_usd: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type")]
pub enum EventBody {
    #[serde(rename = "thread.created")]
    Created(Header),
    #[serde(rename = "post")]
    Post {
        author: String,
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply_to: Option<String>,
    },
    #[serde(rename = "agent_reply")]
    AgentReply {
        author: String,
        model: String,
        mode: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply_to: Option<String>,
        body: String,
        #[serde(default)]
        quotes: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
    },
    #[serde(rename = "agent_status")]
    AgentStatus {
        state: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    #[serde(rename = "edit")]
    Edit { target: String, body: String },
    #[serde(rename = "read")]
    Read,
    #[serde(rename = "unread")]
    Unread,
    #[serde(rename = "agent_on")]
    AgentOn {
        mode: AgentMode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile: Option<String>,
    },
    #[serde(rename = "agent_off")]
    AgentOff,
    #[serde(rename = "material")]
    MaterialUpdate { index: usize, material: Material },
    #[serde(rename = "pin")]
    Pin,
    #[serde(rename = "unpin")]
    Unpin,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Event {
    pub schema: u32,
    pub id: String,
    pub ts: Ts,
    #[serde(flatten)]
    pub body: EventBody,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PostView {
    pub id: String,
    pub author: String,
    pub body: String,
    pub ts: Ts,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    pub edited: bool,
    /// Preenchidos só em respostas de agente.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub quotes: Vec<String>,
    pub is_agent: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ThreadState {
    pub id: String,
    pub kind: ThreadKind,
    pub created: Ts,
    pub last_activity: Ts,
    pub source: String,
    pub private: bool,
    pub material: Vec<Material>,
    pub posts: Vec<PostView>,
    pub agent: AgentMode,
    pub profile: Option<String>,
    pub agent_state: Option<String>,
    pub agent_detail: Option<String>,
    pub read_at: Option<Ts>,
    pub pinned: bool,
    /// Número de eventos; o próximo evento usa `seq + 1`.
    pub seq: usize,
}

impl ThreadState {
    /// Dobra os eventos de uma thread. O primeiro precisa ser `thread.created`.
    pub fn fold(events: &[Event]) -> Option<ThreadState> {
        let first = events.first()?;
        let EventBody::Created(h) = &first.body else { return None };
        let mut st = ThreadState {
            id: first.id.clone(),
            kind: h.kind,
            created: first.ts,
            last_activity: first.ts,
            source: h.source.clone(),
            private: h.private,
            material: h.material.clone(),
            posts: vec![],
            agent: h.agent,
            profile: h.profile.clone(),
            agent_state: None,
            agent_detail: None,
            read_at: None,
            pinned: false,
            seq: events.len(),
        };
        for ev in &events[1..] {
            st.last_activity = st.last_activity.max(ev.ts);
            match &ev.body {
                EventBody::Created(_) => {}
                EventBody::Post { author, body, reply_to } => st.posts.push(PostView {
                    id: ev.id.clone(),
                    author: author.clone(),
                    body: body.clone(),
                    ts: ev.ts,
                    reply_to: reply_to.clone(),
                    edited: false,
                    model: None,
                    mode: None,
                    quotes: vec![],
                    is_agent: false,
                }),
                EventBody::AgentReply { author, model, mode, reply_to, body, quotes, .. } => st.posts.push(PostView {
                    id: ev.id.clone(),
                    author: author.clone(),
                    body: body.clone(),
                    ts: ev.ts,
                    reply_to: reply_to.clone(),
                    edited: false,
                    model: Some(model.clone()),
                    mode: Some(mode.clone()),
                    quotes: quotes.clone(),
                    is_agent: true,
                }),
                EventBody::AgentStatus { state, detail } => {
                    st.agent_state = Some(state.clone());
                    st.agent_detail = detail.clone();
                }
                EventBody::Edit { target, body } => {
                    if let Some(p) = st.posts.iter_mut().find(|p| &p.id == target && !p.is_agent) {
                        p.body = body.clone();
                        p.edited = true;
                    }
                }
                EventBody::Read => st.read_at = Some(ev.ts),
                EventBody::Unread => st.read_at = None,
                EventBody::AgentOn { mode, profile } => {
                    st.agent = *mode;
                    if profile.is_some() {
                        st.profile = profile.clone();
                    }
                }
                EventBody::AgentOff => st.agent = AgentMode::Off,
                EventBody::MaterialUpdate { index, material } => {
                    if *index < st.material.len() {
                        st.material[*index] = material.clone();
                    } else {
                        st.material.push(material.clone());
                    }
                }
                EventBody::Pin => st.pinned = true,
                EventBody::Unpin => st.pinned = false,
            }
        }
        Some(st)
    }

    pub fn root(&self) -> Option<&PostView> {
        self.posts.iter().find(|p| !p.is_agent)
    }

    /// Thread "com material" para efeito do "Li" (RF-OBS-02).
    pub fn has_material(&self) -> bool {
        !self.material.is_empty() || matches!(self.kind, ThreadKind::Reading | ThreadKind::Watching)
    }

    pub fn title(&self) -> String {
        if let Some(t) = self.material.iter().find_map(|m| m.title.clone()) {
            return t;
        }
        let root = self.root().map(|p| p.body.as_str()).unwrap_or("");
        let line = root.lines().find(|l| !l.trim().is_empty()).unwrap_or("(sem texto)");
        let plain = line.trim().trim_start_matches('#').trim().replace("**", "").replace("__", "").replace('`', "");
        crate::text::clip(&plain, 90)
    }

    /// Id do próximo post (`p<n>`) ou evento (`e<n>`), sequencial dentro da thread.
    pub fn next_id(&self, post: bool) -> String {
        format!("{}{}", if post { "p" } else { "e" }, self.seq + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::now;

    fn ev(id: &str, body: EventBody) -> Event {
        Event { schema: 1, id: id.into(), ts: now(), body }
    }

    #[test]
    fn events_round_trip_and_fold() {
        let header = Header {
            kind: ThreadKind::Reading,
            agent: AgentMode::Opinion,
            profile: Some("claude-assinatura".into()),
            private: false,
            source: "app".into(),
            material: vec![Material { url: "https://x.com".into(), ..Default::default() }],
        };
        let events = vec![
            ev("t-1", EventBody::Created(header)),
            ev("p2", EventBody::Post { author: "voce".into(), body: "Lendo isso".into(), reply_to: None }),
            ev(
                "p3",
                EventBody::AgentReply {
                    author: "profile:claude-assinatura".into(),
                    model: "haiku".into(),
                    mode: "opinion".into(),
                    reply_to: Some("p2".into()),
                    body: "Concordo".into(),
                    quotes: vec!["trecho".into()],
                    usage: None,
                },
            ),
            ev("e4", EventBody::Edit { target: "p2".into(), body: "Lendo isso agora".into() }),
            ev("e5", EventBody::Read),
            ev("e6", EventBody::AgentOff),
        ];
        let lines: Vec<String> = events.iter().map(|e| serde_json::to_string(e).unwrap()).collect();
        assert!(lines[0].contains("\"type\":\"thread.created\""));
        assert!(lines[4].contains("\"type\":\"read\""));
        let back: Vec<Event> = lines.iter().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(back, events);
        let st = ThreadState::fold(&back).unwrap();
        assert_eq!(st.posts.len(), 2);
        assert!(st.posts[0].edited);
        assert_eq!(st.posts[0].body, "Lendo isso agora");
        assert!(st.read_at.is_some());
        assert_eq!(st.agent, AgentMode::Off);
        assert_eq!(st.next_id(true), "p7");
        assert!(st.has_material());
    }
}
