//! `config.toml`: preferências com padrões da especificação.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    pub language: String,
    pub http_port: u16,
    pub ignored: Ignored,
    pub context: ContextLimits,
    pub trash_days: i64,
    pub focus: Focus,
    pub ingest: Ingest,
    pub notifications: Notifications,
    /// Threads privadas também podem ir ao MCP? Nunca, por RF-MCP-04; o campo existe só para deixar isso explícito.
    pub mcp_private: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Ignored {
    /// RF-OBS-02: horas sem "Li" para uma thread com material virar ignorada.
    pub thread_hours: i64,
    /// RF-OBS-03: dias sem alteração para uma tarefa sem prazo virar ignorada.
    pub task_days: i64,
    /// RF-OBS-02: por padrão só threads com material contam.
    pub only_with_material: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct ContextLimits {
    pub box_tokens: usize,
    pub neighbor_hops: usize,
    pub max_hops: usize,
    pub max_boxes: usize,
    pub thread_chars: usize,
    pub material_chars: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Focus {
    pub enabled: bool,
    pub catch_up_days: i64,
    /// Perfil usado no resumo do dia; vazio = perfil padrão.
    pub profile: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Ingest {
    pub timeout_secs: u64,
    pub max_bytes: u64,
    pub retries: u32,
    pub user_agent: String,
    pub chrome_fallback: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Notifications {
    pub focus_morning: bool,
    pub task_due: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            language: "pt-BR".into(),
            http_port: 47821,
            ignored: Ignored::default(),
            context: ContextLimits::default(),
            trash_days: 30,
            focus: Focus::default(),
            ingest: Ingest::default(),
            notifications: Notifications::default(),
            mcp_private: false,
        }
    }
}

impl Default for Ignored {
    fn default() -> Self {
        Self { thread_hours: 24, task_days: 7, only_with_material: true }
    }
}

impl Default for ContextLimits {
    fn default() -> Self {
        Self { box_tokens: 400, neighbor_hops: 1, max_hops: 2, max_boxes: 12, thread_chars: 24_000, material_chars: 40_000 }
    }
}

impl Default for Focus {
    fn default() -> Self {
        Self { enabled: true, catch_up_days: 7, profile: String::new() }
    }
}

impl Default for Ingest {
    fn default() -> Self {
        Self {
            timeout_secs: 20,
            max_bytes: 20 * 1024 * 1024,
            retries: 3,
            user_agent: "AnthiveNovo/0.1 (+https://github.com/andre2654/anthive)".into(),
            chrome_fallback: true,
        }
    }
}

impl Default for Notifications {
    fn default() -> Self {
        Self { focus_morning: false, task_due: true }
    }
}
