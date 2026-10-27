//! `novo`: CLI do Non Panã (RF-PUB-06, RF-FOC-07) e servidor MCP (`novo mcp`).

mod client;
mod doctor;
mod install;
mod mcp;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use client::Client;
use novo_core::Vault;
use serde_json::{Value, json};
use std::io::Read;

#[derive(Parser)]
#[command(name = "novo", version, about = "Non Panã: Rede Social, Tasks, Sessão Foco e Observabilidade, local.")]
struct Cli {
    /// Saída em JSON
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Publica uma thread
    Post {
        text: Vec<String>,
        #[arg(long = "link")]
        links: Vec<String>,
        #[arg(long = "file")]
        files: Vec<String>,
        /// thinking | reading | watching
        #[arg(long)]
        kind: Option<String>,
        /// summary | opinion | both
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long)]
        private: bool,
    },
    /// Mostra o feed
    Feed {
        #[arg(long)]
        pending: bool,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Mostra uma thread
    Show { id: String },
    /// Responde numa thread
    Reply { id: String, body: Vec<String> },
    /// Marca Li
    Read { id: String },
    /// Busca
    Search { q: Vec<String> },
    /// Roda a Sessão Foco de um dia (padrão: ontem)
    Foco {
        #[arg(long = "dia")]
        day: Option<String>,
    },
    /// O que só foi ignorado
    Ignorados,
    /// Tarefas
    Task {
        #[command(subcommand)]
        cmd: TaskCmd,
    },
    /// Estado do daemon e do cofre
    Status,
    /// Reconstrói o índice
    Reindex,
    /// Verifica a instalação
    Doctor,
    /// Configura o MCP no Claude Desktop ou num projeto do Anthive
    InstallMcp {
        #[arg(long)]
        claude_desktop: bool,
        /// Diretório do projeto (grava .mcp.json)
        #[arg(long)]
        project: Option<String>,
        /// Caminho alternativo do arquivo de configuração
        #[arg(long)]
        config: Option<String>,
    },
    /// Grava o LaunchAgent do novod (não carrega sem --load)
    InstallDaemon {
        #[arg(long)]
        load: bool,
        #[arg(long)]
        path: Option<String>,
    },
    /// Abre a interface no navegador
    Open,
    /// Guarda uma chave no Keychain (lê o segredo da entrada padrão)
    Key { account: String },
    /// Servidor MCP sobre stdio
    Mcp {
        /// Só ferramentas de leitura (usado pelo chat sobre a thread)
        #[arg(long)]
        read_only: bool,
    },
    /// Roda o daemon neste terminal
    Daemon,
    /// Chamada JSON-RPC direta ao daemon
    Rpc { method: String, params: Option<String> },
}

#[derive(Subcommand)]
enum TaskCmd {
    /// Cria tarefa em linguagem natural: "revisar landing amanhã 9h p1 #marca"
    Add { text: Vec<String> },
    /// Lista: today, next7, inbox, all, done
    List {
        #[arg(default_value = "today")]
        view: String,
    },
    /// Conclui
    Done { id: String },
}

fn print(json_out: bool, v: &Value, human: impl FnOnce(&Value) -> String) {
    if json_out {
        println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
    } else {
        println!("{}", human(v));
    }
}

fn card_line(c: &Value) -> String {
    format!(
        "{} {} {}{}{}",
        c["id"].as_str().unwrap_or(""),
        match c["kind"].as_str() {
            Some("reading") => "📖",
            Some("watching") => "👀",
            _ => "💭",
        },
        c["title"].as_str().unwrap_or(""),
        if c["read"].as_bool().unwrap_or(false) { "  ✓ Li" } else if c["has_material"].as_bool().unwrap_or(false) { "  · pendente" } else { "" },
        c["agent_state"].as_str().map(|s| format!("  · agente: {s}")).unwrap_or_default()
    )
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let vault = Vault::from_env()?;
    let j = cli.json;
    match cli.cmd {
        Cmd::Mcp { read_only } => return mcp::serve(vault, read_only).await,
        Cmd::Daemon => {
            let svc = std::sync::Arc::new(novo_service::Service::open(vault)?);
            return novo_service::server::serve(svc).await;
        }
        Cmd::Doctor => {
            let checks = doctor::checks(&vault).await;
            let mut bad = false;
            for c in &checks {
                println!("{} {:<30} {}", if c.ok { "✓" } else if c.critical { "✗" } else { "!" }, c.name, c.detail);
                bad |= c.critical && !c.ok;
            }
            std::process::exit(if bad { 1 } else { 0 });
        }
        Cmd::InstallMcp { claude_desktop, project, config } => {
            let (path, env): (std::path::PathBuf, Vec<(&str, String)>) = match (claude_desktop, project) {
                (true, _) => (config.map(Into::into).unwrap_or_else(install::claude_desktop_config), vec![]),
                (false, Some(dir)) => {
                    let dir = std::path::PathBuf::from(dir);
                    (config.map(Into::into).unwrap_or_else(|| dir.join(".mcp.json")), vec![])
                }
                _ => bail!("use --claude-desktop ou --project <dir>"),
            };
            let env_ref: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
            let backup = install::install_mcp(&path, &vault.root, &env_ref)?;
            println!("MCP `novo` gravado em {}{}", path.display(), backup.map(|b| format!(" (backup em {})", b.display())).unwrap_or_default());
            if claude_desktop {
                println!("Reinicie o Claude Desktop para carregar o servidor.");
            }
            return Ok(());
        }
        Cmd::InstallDaemon { load, path } => {
            let p = path.map(std::path::PathBuf::from).unwrap_or_else(install::launch_agent_path);
            let plist = install::launch_agent_plist(&client::novod_bin(), &vault.root);
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d)?;
            }
            std::fs::write(&p, plist)?;
            println!("LaunchAgent gravado em {}", p.display());
            if load {
                let uid = std::process::Command::new("id").arg("-u").output()?;
                let uid = String::from_utf8_lossy(&uid.stdout).trim().to_string();
                let st = std::process::Command::new("launchctl").args(["bootstrap", &format!("gui/{uid}"), &p.display().to_string()]).status()?;
                println!("launchctl bootstrap: {st}");
            } else {
                println!("Para carregar: launchctl bootstrap gui/$(id -u) {}", p.display());
            }
            return Ok(());
        }
        Cmd::Open => {
            let mut c = Client::open(&vault, true).await?;
            c.call("status", json!({})).await?;
            let port = std::fs::read_to_string(vault.runtime_dir().join("http.port"))?;
            let token = std::fs::read_to_string(vault.runtime_dir().join("token"))?;
            let url = format!("http://127.0.0.1:{}/#token={}", port.trim(), token.trim());
            std::process::Command::new("open").arg(&url).status()?;
            println!("{}", url.split('#').next().unwrap_or(""));
            return Ok(());
        }
        _ => {}
    }
    let mut c = Client::open(&vault, true).await?;
    match cli.cmd {
        Cmd::Post { text, links, files, kind, agent, profile, private } => {
            let files: Vec<String> = files.iter().map(|f| std::fs::canonicalize(f).map(|p| p.display().to_string()).unwrap_or_else(|_| f.clone())).collect();
            let v = c.call("thread.create", json!({"text": text.join(" "), "links": links, "files": files, "kind": kind, "agent": agent, "profile": profile, "private": private, "source": "cli"})).await?;
            print(j, &v, |v| format!("Publicada: {}", card_line(v)));
        }
        Cmd::Feed { pending, limit } => {
            let v = c.call("feed", json!({"pending": if pending { Some(true) } else { None }, "limit": limit})).await?;
            print(j, &v, |v| v.as_array().map(|a| a.iter().map(card_line).collect::<Vec<_>>().join("\n")).unwrap_or_default());
        }
        Cmd::Show { id } => {
            let v = c.call("thread.get", json!({"id": id})).await?;
            print(j, &v, |v| {
                let mut s = card_line(v);
                for p in v["posts"].as_array().cloned().unwrap_or_default() {
                    s.push_str(&format!("\n\n[{}] {}:\n{}", p["id"].as_str().unwrap_or(""), if p["is_agent"].as_bool().unwrap_or(false) { p["author"].as_str().unwrap_or("agente") } else { "você" }, p["body"].as_str().unwrap_or("")));
                }
                s
            });
        }
        Cmd::Reply { id, body } => {
            let v = c.call("thread.reply", json!({"id": id, "body": body.join(" ")})).await?;
            print(j, &v, |v| format!("Respondido em {}", v["id"].as_str().unwrap_or("")));
        }
        Cmd::Read { id } => {
            let v = c.call("thread.read", json!({"id": id, "read": true})).await?;
            print(j, &v, card_line);
        }
        Cmd::Search { q } => {
            let v = c.call("search", json!({"q": q.join(" "), "filters": {"include_private": true}})).await?;
            print(j, &v, |v| v.as_array().map(|a| a.iter().map(|h| format!("{} [{}] {}", h["id"].as_str().unwrap_or(""), h["kind"].as_str().unwrap_or(""), h["title"].as_str().unwrap_or(""))).collect::<Vec<_>>().join("\n")).unwrap_or_default());
        }
        Cmd::Foco { day } => {
            let v = c.call("focus.run", json!({"day": day})).await?;
            print(j, &v, |v| {
                format!(
                    "Foco de {}: {} threads e {} tarefas ignoradas. {}",
                    v["date"].as_str().unwrap_or(""),
                    v["ignored_threads"].as_array().map(|a| a.len()).unwrap_or(0),
                    v["ignored_tasks"].as_array().map(|a| a.len()).unwrap_or(0),
                    v["summary"].as_str().unwrap_or("")
                )
            });
        }
        Cmd::Ignorados => {
            let v = c.call("obs.ignored", json!({})).await?;
            print(j, &v, |v| {
                let mut s = String::from("Threads:\n");
                for t in v["threads"].as_array().cloned().unwrap_or_default() {
                    s.push_str(&format!("  {} ({} h)\n", card_line(&t["card"]), t["card"]["age_hours"]));
                }
                s.push_str("Tarefas:\n");
                for t in v["tasks"].as_array().cloned().unwrap_or_default() {
                    s.push_str(&format!("  {} {} — {} há {} dia(s)\n", t["id"].as_str().unwrap_or(""), t["title"].as_str().unwrap_or(""), if t["reason"] == "overdue" { "vencida" } else { "parada" }, t["days"]));
                }
                s
            });
        }
        Cmd::Task { cmd } => match cmd {
            TaskCmd::Add { text } => {
                let v = c.call("task.create", json!({"quick": text.join(" ")})).await?;
                print(j, &v, |v| format!("Tarefa {}: {}{}", v["task"]["id"].as_str().unwrap_or(""), v["task"]["title"].as_str().unwrap_or(""), v["task"]["due"].as_str().map(|d| format!(" · prazo {d}")).unwrap_or_default()));
            }
            TaskCmd::List { view } => {
                let v = c.call("task.list", json!({"view": view})).await?;
                print(j, &v, |v| v.as_array().map(|a| a.iter().map(|t| format!("{} P{} {}{}", t["id"].as_str().unwrap_or(""), t["priority"], t["title"].as_str().unwrap_or(""), t["due"].as_str().map(|d| format!(" · {d}")).unwrap_or_default())).collect::<Vec<_>>().join("\n")).unwrap_or_default());
            }
            TaskCmd::Done { id } => {
                let v = c.call("task.complete", json!({"id": id, "done": true})).await?;
                print(j, &v, |v| format!("Concluída: {}", v["task"]["title"].as_str().unwrap_or("")));
            }
        },
        Cmd::Status => {
            let v = c.call("status", json!({})).await?;
            print(j, &v, |v| serde_json::to_string_pretty(v).unwrap_or_default());
        }
        Cmd::Reindex => {
            let v = c.call("reindex", json!({})).await?;
            print(j, &v, |v| format!("{} itens indexados", v["indexed"]));
        }
        Cmd::Key { account } => {
            let mut secret = String::new();
            std::io::stdin().read_to_string(&mut secret)?;
            let v = c.call("profile.set_key", json!({"account": account, "secret": secret.trim()})).await?;
            print(j, &v, |v| format!("Chave guardada. Use no perfil: key = \"{}\"", v["key"].as_str().unwrap_or("")));
        }
        Cmd::Rpc { method, params } => {
            let p: Value = params.map(|s| serde_json::from_str(&s)).transpose()?.unwrap_or(json!({}));
            let v = c.call(&method, p).await?;
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
        _ => unreachable!(),
    }
    Ok(())
}
