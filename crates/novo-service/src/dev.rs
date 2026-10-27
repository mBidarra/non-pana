//! Espaço Desenvolvimento (spec 4.8, RF-DEV): o Anthive como está, lido por `anthive status --json`
//! e chamado como cliente do `anthive mcp` para `note_write` e `agent_create`.

use crate::views;
use crate::Service;
use anyhow::{Context, Result, anyhow, bail};
use novo_core::text::{clip, untrusted};
use serde_json::{Value, json};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub fn anthive_bin() -> String {
    std::env::var("NOVO_ANTHIVE_BIN").unwrap_or_else(|_| "anthive".into())
}

pub fn anthive_home() -> std::path::PathBuf {
    std::env::var("ANTHIVE_HOME").map(Into::into).unwrap_or_else(|_| std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".anthive"))
}

/// O registro completo de projetos do Anthive (`projects.json`), inclusive os que ainda não têm agentes.
pub fn registry() -> Vec<Value> {
    std::fs::read_to_string(anthive_home().join("projects.json")).ok().and_then(|s| serde_json::from_str::<Vec<Value>>(&s).ok()).unwrap_or_default()
}

/// Lê o registro e o snapshot do Anthive e cadastra os projetos dele como projetos de desenvolvimento (RF-DEV-01, 03).
pub async fn status(svc: &Service) -> Result<Value> {
    let reg = registry();
    let snapshot = match tokio::time::timeout(Duration::from_secs(20), tokio::process::Command::new(anthive_bin()).args(["status", "--json"]).output()).await {
        Ok(Ok(out)) if out.status.success() => serde_json::from_slice::<Value>(&out.stdout).ok(),
        _ => None,
    };
    if reg.is_empty() && snapshot.is_none() {
        bail!("Anthive não encontrado (nem {} nem `anthive status`)", anthive_home().join("projects.json").display());
    }
    let mut synced = 0;
    let mut seen = std::collections::HashSet::new();
    let from_status = snapshot.as_ref().and_then(|v| v["projects"].as_array().cloned()).unwrap_or_default();
    for p in reg.iter().chain(from_status.iter()) {
        let (Some(id), Some(name)) = (p["id"].as_str(), p["name"].as_str()) else { continue };
        if !seen.insert(id.to_string()) {
            continue;
        }
        let pid = format!("p-anthive-{}", novo_core::text::slug(id, 40));
        if svc.vault.load_project(&pid)?.is_none() {
            crate::tasks::create_project(svc, name, None, Some("development".into()), p["cwd"].as_str().map(String::from), Some(id.to_string())).await?;
            synced += 1;
        }
    }
    let mut v = snapshot.unwrap_or_else(|| json!({"running": 0, "needs": 0, "limits": null, "projects": [], "at": 0}));
    // projetos do registro sem agentes também aparecem
    let listed: std::collections::HashSet<String> = v["projects"].as_array().map(|a| a.iter().filter_map(|p| p["id"].as_str().map(String::from)).collect()).unwrap_or_default();
    if let Some(arr) = v["projects"].as_array_mut() {
        for p in &reg {
            if let Some(id) = p["id"].as_str() {
                if !listed.contains(id) {
                    arr.push(json!({"id": id, "name": p["name"], "cwd": p["cwd"], "running": 0, "agents": [], "needs": []}));
                }
            }
        }
    }
    Ok(json!({"anthive": v, "synced": synced}))
}

/// O mesmo `slugify` do Anthive (`src/core/store.ts`): palavras sem acento juntas por hífen, até `max` caracteres.
fn anthive_slug(s: &str, max: usize) -> String {
    let plain: String = s.to_lowercase().chars().map(|c| match c {
        'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'ç' => 'c',
        'ñ' => 'n',
        c if c.is_ascii_alphanumeric() => c,
        _ => ' ',
    }).collect();
    let mut out: Vec<&str> = vec![];
    for w in plain.split_whitespace() {
        if !out.is_empty() && out.join("-").len() + 1 + w.len() > max {
            break;
        }
        out.push(w);
    }
    let s: String = out.join("-").chars().take(max).collect();
    if s.is_empty() { "nota".into() } else { s }
}

/// Cria um projeto no Anthive como a tela inicial dele (`createProject`): cria a pasta e grava no `projects.json`.
/// Só aceita pastas dentro da home; a mesma pasta já registrada devolve o projeto existente.
pub async fn create(svc: &Service, name: &str, dir: Option<&str>) -> Result<Value> {
    let name = name.trim();
    if name.is_empty() {
        bail!("o projeto precisa de um nome");
    }
    let home = std::path::PathBuf::from(std::env::var("HOME").context("HOME não definido")?);
    let raw = dir.map(str::trim).filter(|d| !d.is_empty()).map(String::from).unwrap_or_else(|| format!("~/Documents/{}", anthive_slug(name, 32)));
    let path = match raw.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => home.join(rest.trim_start_matches('/')),
        _ => std::path::PathBuf::from(&raw),
    };
    if !path.is_absolute() || path.components().any(|c| matches!(c, std::path::Component::ParentDir)) || !path.starts_with(&home) || path == home {
        bail!("a pasta precisa ficar dentro de {} (recebi {raw})", home.display());
    }
    std::fs::create_dir_all(&path).with_context(|| format!("criar {}", path.display()))?;
    // o Claude Code resolve links simbólicos antes de nomear as sessões: o Anthive guarda o caminho real
    let cwd = std::fs::canonicalize(&path).unwrap_or(path).to_string_lossy().to_string();
    let reg_path = anthive_home().join("projects.json");
    let mut list = registry();
    let existing = list.iter().find(|p| p["cwd"].as_str() == Some(cwd.as_str())).cloned();
    let project = match existing.clone() {
        Some(p) => p,
        None => {
            let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis() as u64;
            let mut id = anthive_slug(name, 32);
            if list.iter().any(|p| p["id"].as_str() == Some(id.as_str())) {
                let b36 = radix36(ms);
                id = format!("{id}-{}", &b36[b36.len().saturating_sub(4)..]);
            }
            let p = json!({"id": id, "name": name, "cwd": cwd, "created": ms});
            list.push(p.clone());
            std::fs::create_dir_all(anthive_home())?;
            novo_core::fsx::atomic_write(&reg_path, serde_json::to_string_pretty(&list)?.as_bytes())?;
            p
        }
    };
    let synced = status(svc).await?;
    Ok(json!({"project": project, "existing": existing.is_some(), "anthive": synced["anthive"]}))
}

fn radix36(mut n: u64) -> String {
    const D: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = vec![];
    loop {
        out.push(D[(n % 36) as usize]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

/// Aspas simples do shell POSIX: nada dentro delas é interpretado.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Abre o projeto no Anthive, no terminal (RF-DEV-02): Ghostty quando houver, Terminal senão — como a barra de menus do Anthive.
/// Só abre pastas de projetos de desenvolvimento já cadastrados.
pub fn open(svc: &Service, dir: &str) -> Result<Value> {
    let known = svc.vault.projects().into_iter().any(|p| p.head.anthive_id.is_some() && p.head.dir.as_deref() == Some(dir));
    if !known || !std::path::Path::new(dir).is_dir() {
        bail!("{dir} não é a pasta de um projeto do Anthive cadastrado");
    }
    let script = svc.vault.runtime_dir().join("open-anthive.sh");
    let cmd = format!("cd {} && exec {} {}", sh_quote(dir), sh_quote(&anthive_bin()), sh_quote(dir));
    std::fs::write(&script, format!("#!/bin/sh\n{cmd}\n"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))?;
    }
    let ghostty = "/Applications/Ghostty.app/Contents/MacOS/ghostty";
    if std::path::Path::new(ghostty).exists() {
        std::process::Command::new(ghostty).arg("-e").arg(&script).spawn().context("abrir o Ghostty")?;
        return Ok(json!({"opened": "ghostty"}));
    }
    // o Terminal recebe só o caminho do script, entre aspas do shell e escapado para o AppleScript
    let run = sh_quote(&script.to_string_lossy());
    let src = format!("tell application \"Terminal\"\nactivate\ndo script \"{}\"\nend tell", run.replace('\\', "\\\\").replace('"', "\\\""));
    std::process::Command::new("osascript").arg("-e").arg(src).spawn().context("abrir o Terminal")?;
    Ok(json!({"opened": "terminal"}))
}

/// Chama uma ferramenta do barramento do Anthive como cliente MCP, identificado como o agente `novo`.
pub async fn anthive_tool(anthive_project: &str, cwd: &str, tool: &str, args: Value) -> Result<String> {
    let mut child = tokio::process::Command::new(anthive_bin())
        .arg("mcp")
        .current_dir(cwd)
        .env("ANTHIVE_AGENT", "novo")
        .env("ANTHIVE_PROJECT", anthive_project)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("iniciar anthive mcp")?;
    let mut stdin = child.stdin.take().context("stdin")?;
    let mut lines = BufReader::new(child.stdout.take().context("stdout")?).lines();
    let msgs = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "non-pana", "version": "0.1.0"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": tool, "arguments": args}}),
    ];
    for m in &msgs {
        stdin.write_all(format!("{m}\n").as_bytes()).await?;
    }
    stdin.flush().await?;
    let res = tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(line) = lines.next_line().await? {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if v["id"] == 2 {
                return Ok::<Value, anyhow::Error>(v);
            }
        }
        Err(anyhow!("anthive mcp encerrou sem responder"))
    })
    .await
    .map_err(|_| anyhow!("anthive mcp passou do tempo"))??;
    drop(stdin);
    let _ = child.kill().await;
    if let Some(err) = res.get("error") {
        bail!("anthive mcp: {}", err["message"].as_str().unwrap_or("erro"));
    }
    let text = res["result"]["content"][0]["text"].as_str().unwrap_or_default().to_string();
    if res["result"]["isError"].as_bool().unwrap_or(false) {
        bail!("{text}");
    }
    Ok(text)
}

fn dev_project(svc: &Service, project: &str) -> Result<(String, String)> {
    let p = svc.vault.load_project(project)?.with_context(|| format!("projeto {project} não existe"))?;
    let aid = p.head.anthive_id.clone().context("esse projeto não é do Anthive (tipo desenvolvimento)")?;
    let dir = p.head.dir.clone().context("projeto do Anthive sem diretório")?;
    Ok((aid, dir))
}

/// Uma thread (ou superthread) vira nota do Anthive no projeto (RF-DEV-05).
pub async fn note(svc: &Service, item: &str, project: &str) -> Result<Value> {
    let (aid, dir) = dev_project(svc, project)?;
    if item.starts_with("t-") && svc.thread(item)?.private {
        bail!("thread privada não sai do Novo");
    }
    let b = novo_memory::load_box(&svc.vault, item)?.map(|b| b.full()).unwrap_or_default();
    let title = views::title_of(svc, item);
    let text = format!("{b}\n\nOrigem: Non Panã ({item}).");
    let out = anthive_tool(&aid, &dir, "note_write", json!({"title": clip(&title, 80), "text": text})).await?;
    Ok(json!({"result": out}))
}

/// Delegar uma tarefa a um agente do Anthive (RF-DEV-04): só com confirmação explícita.
pub async fn delegate(svc: &Service, task: &str, agent_name: &str, confirm: bool) -> Result<Value> {
    if !confirm {
        bail!("delegar cria um agente de verdade no Anthive: confirme com confirm=true depois de revisar o briefing");
    }
    let t = svc.vault.load_task(task)?.with_context(|| format!("tarefa {task} não existe"))?;
    let project = t.head.project.clone().context("a tarefa não tem projeto")?;
    let (aid, dir) = dev_project(svc, &project)?;
    let prompt = briefing(svc, task)?;
    let name = if agent_name.trim().is_empty() { novo_core::text::slug(&t.head.title, 24) } else { agent_name.trim().to_string() };
    let out = anthive_tool(&aid, &dir, "agent_create", json!({"name": name, "prompt": prompt})).await?;
    let _w = svc.write.lock().await;
    let mut t = svc.vault.load_task(task)?.context("tarefa sumiu")?;
    t.head.delegated_to = Some(name.clone());
    t.head.updated = novo_core::time::now();
    svc.vault.save_task(&t)?;
    drop(_w);
    svc.emit("task", task);
    Ok(json!({"agent": name, "result": out}))
}

/// O briefing que vai para o agente do Anthive; a UI mostra antes da confirmação (UC-03 antigo, RF-DEV-04).
pub fn briefing(svc: &Service, task: &str) -> Result<String> {
    let t = svc.vault.load_task(task)?.with_context(|| format!("tarefa {task} não existe"))?;
    let mut ctx = String::new();
    for x in svc.graph().related_to(task) {
        if let Ok(Some(b)) = novo_memory::load_box(&svc.vault, &x) {
            ctx.push_str(&format!("\n- {x}: {}", b.l1(300)));
        }
    }
    let mut s = format!("Tarefa vinda do Non Panã: {}\n", t.head.title);
    if !t.description.trim().is_empty() {
        s.push_str(&format!("\n{}\n", t.description.trim()));
    }
    if let Some(d) = &t.head.due {
        s.push_str(&format!("\nPrazo: {d}\n"));
    }
    if !ctx.is_empty() {
        s.push_str(&format!("\nContexto das threads relacionadas:\n{}\n", untrusted("anthive-novo", &ctx)));
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    #[test]
    fn slug_like_anthive() {
        assert_eq!(super::anthive_slug("Pesquisa de Carreira AI", 32), "pesquisa-de-carreira-ai");
        assert_eq!(super::anthive_slug("Ação!! já", 32), "acao-ja");
        assert_eq!(super::anthive_slug("um nome bem comprido que passa do limite", 16), "um-nome-bem");
        assert_eq!(super::anthive_slug("???", 32), "nota");
        assert_eq!(super::radix36(35), "z");
        assert_eq!(super::radix36(36), "10");
    }

    #[test]
    fn sh_quote_keeps_everything_literal() {
        let dir = "/tmp/a b'$(touch NOVO_PWNED)\"`id`";
        let out = std::process::Command::new("sh").arg("-c").arg(format!("printf %s {}", super::sh_quote(dir))).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), dir);
        assert!(!std::path::Path::new("NOVO_PWNED").exists());
    }
}
