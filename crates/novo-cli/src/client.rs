//! Cliente do daemon pelo socket Unix. Sobe o `novod` se ele não estiver rodando (RF-MCP-02).

use anyhow::{Context, Result, anyhow, bail};
use novo_core::Vault;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

pub fn sock(vault: &Vault) -> PathBuf {
    vault.socket_path()
}

pub fn novod_bin() -> PathBuf {
    if let Ok(p) = std::env::var("NOVOD_BIN") {
        return p.into();
    }
    if let Ok(exe) = std::env::current_exe() {
        let sib = exe.with_file_name("novod");
        if sib.exists() {
            return sib;
        }
    }
    "novod".into()
}

/// Inicia o daemon em segundo plano, desligado do terminal.
pub fn spawn_daemon(vault: &Vault) -> Result<()> {
    std::process::Command::new(novod_bin())
        .env("NOVO_HOME", &vault.root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("não consegui iniciar o novod")?;
    Ok(())
}

pub async fn connect(vault: &Vault, start: bool) -> Result<UnixStream> {
    if let Ok(s) = UnixStream::connect(sock(vault)).await {
        return Ok(s);
    }
    if !start {
        bail!("o novod não está rodando");
    }
    spawn_daemon(vault)?;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Ok(s) = UnixStream::connect(sock(vault)).await {
            return Ok(s);
        }
    }
    Err(anyhow!("o novod não respondeu em 5 s"))
}

pub struct Client {
    lines: tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    w: tokio::net::unix::OwnedWriteHalf,
    next: u64,
}

impl Client {
    pub async fn open(vault: &Vault, start: bool) -> Result<Client> {
        let s = connect(vault, start).await?;
        let (r, w) = s.into_split();
        Ok(Client { lines: BufReader::new(r).lines(), w, next: 1 })
    }

    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next;
        self.next += 1;
        let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.w.write_all(format!("{req}\n").as_bytes()).await?;
        let line = self.lines.next_line().await?.context("o novod fechou a conexão")?;
        let v: Value = serde_json::from_str(&line)?;
        if let Some(e) = v.get("error") {
            bail!("{}", e["message"].as_str().unwrap_or("erro"));
        }
        Ok(v["result"].clone())
    }
}
