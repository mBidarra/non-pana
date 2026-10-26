//! Agentes do Novo (spec 5): perfis e motores (5.5), prompts versionados, saída estruturada,
//! diário (RF-AGT-11) e orçamento por perfil (RF-AGT-10).

pub mod engines;
pub mod prompts;
pub mod secrets;

use anyhow::{Result, bail};
use chrono::NaiveDate;
use novo_core::fsx::{append_jsonl, atomic_write, read_jsonl};
use novo_core::time::{self, Ts};
use novo_core::Vault;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;

pub use engines::run;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    ClaudeCode,
    OpenrouterHttp,
    AnthropicApi,
    Ollama,
}

impl Engine {
    /// Motores que mandam dados para fora da máquina (threads privadas nunca usam — RF-AGT-12).
    pub fn is_remote(self) -> bool {
        !matches!(self, Engine::Ollama)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Profile {
    pub name: String,
    pub engine: Engine,
    pub model: String,
    /// subscription | openrouter | anthropic_api | local
    #[serde(default = "default_auth")]
    pub auth: String,
    /// `keychain:<conta>` (serviço `anthive-novo`) ou `env:<VAR>`.
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub daily_budget_usd: Option<f64>,
    #[serde(default)]
    pub daily_budget_turns: Option<u32>,
    /// Base da API (testes e proxies). Padrões: OpenRouter `https://openrouter.ai/api/v1`, Anthropic `https://api.anthropic.com`, Ollama `http://127.0.0.1:11434`.
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
}

fn default_auth() -> String {
    "subscription".into()
}

impl Profile {
    pub fn claude_default() -> Profile {
        Profile {
            name: "claude-assinatura".into(),
            engine: Engine::ClaudeCode,
            model: "haiku".into(),
            auth: "subscription".into(),
            key: None,
            default: true,
            daily_budget_usd: None,
            daily_budget_turns: Some(200),
            base_url: None,
            label: Some("Claude pela assinatura".into()),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ProfilesFile {
    #[serde(default, rename = "profile")]
    pub profiles: Vec<Profile>,
}

pub fn load_profiles(vault: &Vault) -> Vec<Profile> {
    let mut ps: Vec<Profile> = fs::read_to_string(vault.profiles_path())
        .ok()
        .and_then(|s| toml::from_str::<ProfilesFile>(&s).ok())
        .map(|f| f.profiles)
        .unwrap_or_default();
    if ps.is_empty() {
        ps.push(Profile::claude_default());
    }
    if !ps.iter().any(|p| p.default) {
        ps[0].default = true;
    }
    ps
}

pub fn save_profiles(vault: &Vault, profiles: &[Profile]) -> Result<()> {
    let mut names = std::collections::HashSet::new();
    for p in profiles {
        if p.name.trim().is_empty() || !names.insert(p.name.clone()) {
            bail!("nome de perfil vazio ou repetido: {:?}", p.name);
        }
        if let Some(k) = &p.key {
            if !(k.starts_with("keychain:") || k.starts_with("env:")) {
                bail!("a chave do perfil {} precisa ser uma referência (keychain:… ou env:…), nunca o segredo", p.name);
            }
        }
    }
    let f = ProfilesFile { profiles: profiles.to_vec() };
    atomic_write(&vault.profiles_path(), toml::to_string_pretty(&f)?.as_bytes())
}

pub fn find_profile(vault: &Vault, name: Option<&str>) -> Result<Profile> {
    let ps = load_profiles(vault);
    match name {
        Some(n) if !n.is_empty() => ps.into_iter().find(|p| p.name == n).ok_or_else(|| anyhow::anyhow!("perfil {n} não existe")),
        _ => Ok(ps.iter().find(|p| p.default).cloned().unwrap_or_else(|| ps[0].clone())),
    }
}

// ------------------------------------------------------------------ chamadas

#[derive(Clone, Debug)]
pub struct Request {
    pub system: String,
    pub user: String,
    /// JSON Schema da saída esperada.
    pub schema: Option<String>,
    pub max_tokens: u32,
    /// reply | box | superthread | focus | chat
    pub purpose: String,
    /// Chat: servidores MCP (JSON do `--mcp-config`) e ferramentas permitidas.
    pub mcp_config: Option<String>,
    pub allowed_tools: Vec<String>,
}

impl Request {
    pub fn new(purpose: &str, system: String, user: String, schema: Option<&str>) -> Request {
        Request { system, user, schema: schema.map(String::from), max_tokens: 2000, purpose: purpose.into(), mcp_config: None, allowed_tools: vec![] }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Response {
    pub text: String,
    pub json: Option<Value>,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub duration_ms: u64,
}

/// Tira o JSON de um texto: aceita cercas ```json e texto em volta.
pub fn extract_json(text: &str) -> Option<Value> {
    let t = text.trim();
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        return Some(v);
    }
    let unfenced = t.trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```").trim();
    if let Ok(v) = serde_json::from_str::<Value>(unfenced) {
        return Some(v);
    }
    let start = t.find('{')?;
    let end = t.rfind('}')?;
    serde_json::from_str(&t[start..=end]).ok()
}

// ------------------------------------------------------------------ diário e orçamento

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Run {
    pub ts: Ts,
    pub purpose: String,
    pub trigger: String,
    #[serde(default)]
    pub thread: Option<String>,
    pub profile: String,
    pub engine: Engine,
    pub model: String,
    pub prompt_version: String,
    #[serde(default)]
    pub boxes_used: Vec<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub duration_ms: u64,
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub output_preview: String,
    /// Bytes de contexto enviados (o app mostra o que foi enviado — RNF-10).
    #[serde(default)]
    pub sent_chars: usize,
}

pub fn log_run(vault: &Vault, run: &Run) -> Result<()> {
    append_jsonl(&vault.runs_path(time::day_of(&run.ts)), run)
}

pub fn runs_of(vault: &Vault, day: NaiveDate) -> Vec<Run> {
    read_jsonl(&vault.runs_path(day)).map(|r| r.0).unwrap_or_default()
}

/// Recusa a chamada se o orçamento do dia acabou. A Sessão Foco tem cota própria (RF-AGT-10).
pub fn check_budget(vault: &Vault, p: &Profile, purpose: &str) -> Result<()> {
    if purpose == "focus" {
        return Ok(());
    }
    let today = runs_of(vault, time::day_of(&time::now()));
    let mine: Vec<&Run> = today.iter().filter(|r| r.profile == p.name && r.purpose != "focus").collect();
    if let Some(limit) = p.daily_budget_usd {
        let spent: f64 = mine.iter().map(|r| r.cost_usd).sum();
        if spent >= limit {
            bail!("orçamento diário do perfil {} acabou (US$ {spent:.2} de {limit:.2}); a fila continua amanhã", p.name);
        }
    }
    if let Some(limit) = p.daily_budget_turns {
        if mine.len() as u32 >= limit {
            bail!("limite diário de {limit} chamadas do perfil {} atingido; a fila continua amanhã", p.name);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_default_and_secret_guard() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        let ps = load_profiles(&v);
        assert_eq!(ps[0].name, "claude-assinatura");
        assert!(ps[0].default);
        let mut bad = Profile::claude_default();
        bad.key = Some("sk-or-v1-segredo".into());
        assert!(save_profiles(&v, &[bad]).is_err(), "segredo nunca vai para o arquivo");
        let mut or = Profile::claude_default();
        or.name = "outro".into();
        or.engine = Engine::OpenrouterHttp;
        or.key = Some("keychain:novo-openrouter".into());
        or.default = false;
        save_profiles(&v, &[Profile::claude_default(), or.clone()]).unwrap();
        assert_eq!(find_profile(&v, Some("outro")).unwrap(), or);
        assert_eq!(find_profile(&v, None).unwrap().name, "claude-assinatura");
    }

    #[test]
    fn budget_counts_only_non_focus_runs() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        let mut p = Profile::claude_default();
        p.daily_budget_turns = Some(2);
        let run = |purpose: &str| Run {
            ts: time::now(),
            purpose: purpose.into(),
            trigger: "test".into(),
            thread: None,
            profile: p.name.clone(),
            engine: Engine::ClaudeCode,
            model: "haiku".into(),
            prompt_version: "x".into(),
            boxes_used: vec![],
            input_tokens: 1,
            output_tokens: 1,
            cost_usd: 0.0,
            duration_ms: 1,
            ok: true,
            error: None,
            output_preview: String::new(),
            sent_chars: 0,
        };
        log_run(&v, &run("focus")).unwrap();
        log_run(&v, &run("reply")).unwrap();
        assert!(check_budget(&v, &p, "reply").is_ok());
        log_run(&v, &run("reply")).unwrap();
        assert!(check_budget(&v, &p, "reply").is_err());
        assert!(check_budget(&v, &p, "focus").is_ok(), "Foco nunca é bloqueado pelo uso das threads");
    }

    #[test]
    fn json_extraction_is_lenient() {
        assert_eq!(extract_json("```json\n{\"a\":1}\n```").unwrap()["a"], 1);
        assert_eq!(extract_json("Claro! {\"a\": 2} pronto").unwrap()["a"], 2);
        assert!(extract_json("sem json").is_none());
    }
}
