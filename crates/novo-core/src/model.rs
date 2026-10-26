//! Entidades em markdown com frontmatter (tarefas, projetos, anotações) e metadados de superthread.

use crate::time::Ts;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TaskHead {
    pub kind: String,
    pub schema: u32,
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub parent: Option<String>,
    /// `AAAA-MM-DD` ou RFC3339 com hora.
    #[serde(default)]
    pub due: Option<String>,
    /// 1 (P1, mais alta) a 4 (P4).
    #[serde(default = "default_priority")]
    pub priority: u8,
    #[serde(default)]
    pub labels: Vec<String>,
    /// open | done
    #[serde(default = "default_open")]
    pub status: String,
    #[serde(default)]
    pub completed_at: Option<Ts>,
    pub created: Ts,
    pub updated: Ts,
    /// manual | thread | mcp | agent
    #[serde(default = "default_origin")]
    pub origin: String,
    #[serde(default)]
    pub order: i64,
    /// Nome do agente do Anthive a quem a tarefa foi delegada (RF-DEV-04).
    #[serde(default)]
    pub delegated_to: Option<String>,
}

fn default_priority() -> u8 {
    4
}
fn default_open() -> String {
    "open".into()
}
fn default_origin() -> String {
    "manual".into()
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Task {
    #[serde(flatten)]
    pub head: TaskHead,
    pub description: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ProjectHead {
    pub kind: String,
    pub schema: u32,
    pub id: String,
    pub name: String,
    /// tasks | development
    #[serde(rename = "type", default = "default_ptype")]
    pub ptype: String,
    #[serde(default)]
    pub dir: Option<String>,
    #[serde(default)]
    pub anthive_id: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    pub created: Ts,
    #[serde(default)]
    pub archived: bool,
}

fn default_ptype() -> String {
    "tasks".into()
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Project {
    #[serde(flatten)]
    pub head: ProjectHead,
    pub body: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NoteHead {
    pub kind: String,
    pub schema: u32,
    pub id: String,
    #[serde(default)]
    pub title: String,
    /// `p-…` ou `k-…`; vazio = anotação solta.
    #[serde(default)]
    pub about: Option<String>,
    pub created: Ts,
    pub updated: Ts,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Note {
    #[serde(flatten)]
    pub head: NoteHead,
    pub body: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SuperthreadMeta {
    pub schema: u32,
    pub id: String,
    pub title: String,
    /// agent | voce
    #[serde(default = "default_title_by")]
    pub title_by: String,
    pub created: Ts,
    #[serde(default)]
    pub dissolved: bool,
}

fn default_title_by() -> String {
    "voce".into()
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AccessEvent {
    pub ts: Ts,
    pub thread: String,
    /// open | reply | read
    pub action: String,
}
