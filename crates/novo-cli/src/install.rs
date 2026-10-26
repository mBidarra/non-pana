//! Instaladores: MCP no Claude Desktop e em projetos do Anthive (RF-MCP-06) e o LaunchAgent do daemon.

use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub fn novo_bin() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| "novo".into())
}

pub fn claude_desktop_config() -> PathBuf {
    dirs_home().join("Library/Application Support/Claude/claude_desktop_config.json")
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| ".".into())
}

/// Grava a entrada `novo` em `mcpServers`, com backup do arquivo anterior. Devolve o caminho do backup.
pub fn install_mcp(config: &Path, vault_root: &Path, extra_env: &[(&str, &str)]) -> Result<Option<PathBuf>> {
    let mut cfg: Value = match std::fs::read_to_string(config) {
        Ok(s) if !s.trim().is_empty() => serde_json::from_str(&s).with_context(|| format!("{} não é JSON válido; não mexi nele", config.display()))?,
        _ => json!({}),
    };
    let backup = if config.exists() {
        let b = config.with_extension(format!("json.bak-{}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
        std::fs::copy(config, &b)?;
        Some(b)
    } else {
        None
    };
    let mut env = json!({"NOVO_HOME": vault_root});
    for (k, v) in extra_env {
        env[*k] = json!(v);
    }
    if !cfg["mcpServers"].is_object() {
        cfg["mcpServers"] = json!({});
    }
    cfg["mcpServers"]["novo"] = json!({"command": novo_bin(), "args": ["mcp"], "env": env});
    if let Some(d) = config.parent() {
        std::fs::create_dir_all(d)?;
    }
    novo_core::fsx::atomic_write(config, format!("{}\n", serde_json::to_string_pretty(&cfg)?).as_bytes())?;
    Ok(backup)
}

pub fn launch_agent_plist(novod: &Path, vault_root: &Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.anthive.novod</string>
  <key>ProgramArguments</key><array><string>{}</string></array>
  <key>EnvironmentVariables</key><dict><key>NOVO_HOME</key><string>{}</string><key>PATH</key><string>/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:{}/.local/bin</string></dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardErrorPath</key><string>{}/logs/novod.stderr.log</string>
</dict>
</plist>
"#,
        novod.display(),
        vault_root.display(),
        dirs_home().display(),
        vault_root.display()
    )
}

pub fn launch_agent_path() -> PathBuf {
    dirs_home().join("Library/LaunchAgents/com.anthive.novod.plist")
}
