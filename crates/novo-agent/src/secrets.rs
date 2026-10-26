//! Segredos no Keychain do macOS (serviço `anthive-novo`), via `security`. Nunca em arquivo (RNF-11).

use anyhow::{Context, Result, bail};
use std::io::Write;
use std::process::{Command, Stdio};

pub const SERVICE: &str = "anthive-novo";

/// Resolve uma referência `keychain:<conta>` ou `env:<VAR>`.
pub fn resolve(reference: &str) -> Result<String> {
    if let Some(var) = reference.strip_prefix("env:") {
        return std::env::var(var).with_context(|| format!("variável {var} não definida"));
    }
    let Some(account) = reference.strip_prefix("keychain:") else { bail!("referência de chave inválida: {reference}") };
    let out = Command::new("security").args(["find-generic-password", "-s", SERVICE, "-a", account, "-w"]).output().context("rodar security")?;
    if !out.status.success() {
        bail!("chave {account} não está no Keychain (serviço {SERVICE})");
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Grava pelo modo interativo do `security` (comando no stdin), para o segredo não aparecer em `ps`.
pub fn store(account: &str, secret: &str) -> Result<()> {
    let bad = |s: &str| s.is_empty() || s.chars().any(|c| matches!(c, '"' | '\\' | '\n' | '\r'));
    if bad(account) || bad(secret) {
        bail!("a chave e o nome não podem ser vazios nem ter aspas, barra invertida ou quebra de linha");
    }
    let mut child = Command::new("security")
        .arg("-i")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("rodar security")?;
    child
        .stdin
        .take()
        .context("stdin do security")?
        .write_all(format!("add-generic-password -U -s {SERVICE} -a \"{account}\" -w \"{secret}\"\n").as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!("não consegui gravar a chave {account} no Keychain: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

pub fn exists(account: &str) -> bool {
    Command::new("security").args(["find-generic-password", "-s", SERVICE, "-a", account]).output().map(|o| o.status.success()).unwrap_or(false)
}

pub fn delete(account: &str) -> Result<()> {
    let _ = Command::new("security").args(["delete-generic-password", "-s", SERVICE, "-a", account]).output();
    Ok(())
}
