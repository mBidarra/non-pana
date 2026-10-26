//! `novo doctor` (RNF-20): o que esta máquina tem e o que falta.

use crate::client::Client;
use novo_core::Vault;
use std::process::Command;

pub struct Check {
    pub name: &'static str,
    pub ok: bool,
    pub critical: bool,
    pub detail: String,
}

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let o = Command::new(cmd).args(args).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

pub async fn checks(vault: &Vault) -> Vec<Check> {
    let mut v = vec![];
    let probe = vault.runtime_dir().join(".doctor");
    let writable = std::fs::write(&probe, b"ok").is_ok();
    let _ = std::fs::remove_file(&probe);
    v.push(Check { name: "Cofre", ok: writable, critical: true, detail: vault.root.display().to_string() });
    let daemon = Client::open(vault, false).await.is_ok();
    v.push(Check { name: "Daemon (novod)", ok: daemon, critical: false, detail: if daemon { "rodando".into() } else { "parado — `novo daemon` ou o LaunchAgent".into() } });
    let claude = run(&std::env::var("NOVO_CLAUDE_BIN").unwrap_or_else(|_| "claude".into()), &["--version"]);
    v.push(Check { name: "Claude Code", ok: claude.is_some(), critical: false, detail: claude.unwrap_or_else(|| "não encontrado: o perfil padrão precisa dele".into()) });
    let auth = run("claude", &["auth", "status"]).map(|s| s.contains("\"loggedIn\": true") || s.contains("\"loggedIn\":true")).unwrap_or(false);
    v.push(Check { name: "Claude Code logado", ok: auth, critical: false, detail: if auth { "assinatura ativa".into() } else { "rode `claude auth login`".into() } });
    let anthive = run(&novo_service::dev::anthive_bin(), &["--version"]);
    v.push(Check { name: "Anthive", ok: anthive.is_some(), critical: false, detail: anthive.unwrap_or_else(|| "não encontrado: o espaço Desenvolvimento fica vazio".into()) });
    let chrome = novo_ingest::chrome_binary();
    v.push(Check { name: "Chrome (fallback de extração)", ok: chrome.is_some(), critical: false, detail: chrome.map(|p| p.display().to_string()).unwrap_or_else(|| "não encontrado".into()) });
    for p in novo_agent::load_profiles(vault) {
        let ok = p.key.as_deref().map(|k| novo_agent::secrets::resolve(k).is_ok()).unwrap_or(true);
        v.push(Check { name: "Perfil de agente", ok, critical: false, detail: format!("{} ({:?}, {}){}", p.name, p.engine, p.model, if ok { "" } else { " — chave ausente" }) });
    }
    match novo_graph::Graph::load(&vault.graph_path()) {
        Ok(g) => {
            let r = g.check();
            v.push(Check { name: "Grafo", ok: r.is_ok(), critical: true, detail: r.err().map(|e| e.to_string()).unwrap_or_else(|| format!("{} superthreads", g.superthreads().len())) });
        }
        Err(e) => v.push(Check { name: "Grafo", ok: false, critical: true, detail: e.to_string() }),
    }
    let threads = vault.thread_ids().len();
    let indexed = novo_index::Index::open(&vault.index_path()).and_then(|i| i.count()).unwrap_or(-1);
    v.push(Check { name: "Índice", ok: indexed >= threads as i64, critical: false, detail: format!("{indexed} itens indexados, {threads} threads — `novo reindex` reconstrói") });
    let la = crate::install::launch_agent_path().exists();
    v.push(Check { name: "LaunchAgent", ok: la, critical: false, detail: if la { "instalado".into() } else { "não instalado — `novo install-daemon`".into() } });
    let mcp = std::fs::read_to_string(crate::install::claude_desktop_config()).map(|s| s.contains("\"novo\"")).unwrap_or(false);
    v.push(Check { name: "MCP no Claude Desktop", ok: mcp, critical: false, detail: if mcp { "configurado".into() } else { "não configurado — `novo install-mcp --claude-desktop`".into() } });
    v
}
