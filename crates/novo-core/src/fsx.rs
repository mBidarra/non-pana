//! Escrita segura: arquivo inteiro por tmp + rename + fsync; JSONL por acréscimo + fsync.

use anyhow::{Context, Result};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static TMP: AtomicU64 = AtomicU64::new(0);

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("caminho sem diretório")?;
    fs::create_dir_all(dir)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("f");
    let tmp = dir.join(format!(".{name}.tmp-{}-{}", std::process::id(), TMP.fetch_add(1, Ordering::Relaxed)));
    {
        let mut f = File::create(&tmp).with_context(|| format!("criar {}", tmp.display()))?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path).with_context(|| format!("renomear para {}", path.display()))?;
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut s = serde_json::to_string_pretty(value)?;
    s.push('\n');
    atomic_write(path, s.as_bytes())
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let s = fs::read_to_string(path).with_context(|| format!("ler {}", path.display()))?;
    Ok(serde_json::from_str(&s).with_context(|| format!("json inválido em {}", path.display()))?)
}

/// Acrescenta uma linha JSON e força para o disco.
pub fn append_jsonl<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut line = serde_json::to_string(value)?;
    line.push('\n');
    let mut f = OpenOptions::new().create(true).append(true).open(path).with_context(|| format!("abrir {}", path.display()))?;
    f.write_all(line.as_bytes())?;
    f.sync_data()?;
    Ok(())
}

/// Lê um JSONL. Linhas inválidas (ex.: uma última linha cortada por crash) são ignoradas e contadas.
pub fn read_jsonl<T: DeserializeOwned>(path: &Path) -> Result<(Vec<T>, usize)> {
    let f = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((vec![], 0)),
        Err(e) => return Err(e).with_context(|| format!("abrir {}", path.display())),
    };
    let mut out = vec![];
    let mut bad = 0;
    for line in BufReader::new(f).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str(&line) {
            Ok(v) => out.push(v),
            Err(_) => bad += 1,
        }
    }
    Ok((out, bad))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonl_round_trip_and_tolerates_torn_line() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a/b.jsonl");
        append_jsonl(&p, &serde_json::json!({"n": 1})).unwrap();
        append_jsonl(&p, &serde_json::json!({"n": 2})).unwrap();
        let mut f = OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(b"{\"n\": 3").unwrap();
        let (rows, bad): (Vec<serde_json::Value>, usize) = read_jsonl(&p).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(bad, 1);
    }

    #[test]
    fn atomic_write_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.md");
        atomic_write(&p, b"um").unwrap();
        atomic_write(&p, b"dois").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "dois");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
