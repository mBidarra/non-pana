//! O cofre: caminhos e operações de arquivo. Regras de negócio ficam no `novo-service`.

use crate::config::Config;
use crate::frontmatter;
use crate::fsx::{append_jsonl, atomic_write, read_json, read_jsonl, write_json};
use crate::ids::millis_of;
use crate::model::{AccessEvent, Note, NoteHead, Project, ProjectHead, SuperthreadMeta, Task, TaskHead};
use crate::thread::{Event, EventBody, Header, ThreadState};
use crate::time::{self, Ts};
use anyhow::{Context, Result, bail};
use chrono::{Datelike, NaiveDate};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Vault {
    pub root: PathBuf,
}

impl Vault {
    /// `NOVO_HOME` ou `~/.anthive/novo`.
    pub fn default_root() -> PathBuf {
        if let Ok(h) = std::env::var("NOVO_HOME") {
            return PathBuf::from(h);
        }
        dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".anthive").join("novo")
    }

    pub fn open(root: impl Into<PathBuf>) -> Result<Vault> {
        let v = Vault { root: root.into() };
        for d in [
            "threads", "boxes/.versions", "superthreads", "chats", "tasks", "projects", "notes", "focus", "trash", "prompts",
            "agent/runs", "inbox", ".index", ".queue", ".runtime", "logs",
        ] {
            fs::create_dir_all(v.root.join(d)).with_context(|| format!("criar {d}"))?;
        }
        Ok(v)
    }

    /// Dia (GMT-3) em que o cofre foi criado; a Sessão Foco não recupera dias anteriores a ele.
    pub fn created_day(&self) -> NaiveDate {
        let p = self.runtime_dir().join("created");
        if let Some(d) = fs::read_to_string(&p).ok().and_then(|s| NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()) {
            return d;
        }
        let first = self.thread_ids().first().and_then(|id| millis_of(id)).map(|ms| time::day_of(&time::from_millis(ms)));
        let d = first.unwrap_or_else(|| time::day_of(&time::now()));
        let _ = fs::write(&p, d.format("%Y-%m-%d").to_string());
        d
    }

    pub fn from_env() -> Result<Vault> {
        Vault::open(Vault::default_root())
    }

    // ---------------------------------------------------------------- caminhos
    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.toml")
    }
    pub fn profiles_path(&self) -> PathBuf {
        self.root.join("profiles.toml")
    }
    pub fn graph_path(&self) -> PathBuf {
        self.root.join("graph.jsonl")
    }
    pub fn access_path(&self) -> PathBuf {
        self.root.join("access.jsonl")
    }
    pub fn index_path(&self) -> PathBuf {
        self.root.join(".index").join("novo.db")
    }
    pub fn queue_dir(&self) -> PathBuf {
        self.root.join(".queue")
    }
    pub fn runtime_dir(&self) -> PathBuf {
        self.root.join(".runtime")
    }
    /// Socket do daemon. O limite do macOS é ~104 bytes; com um cofre em caminho longo, o socket vai para
    /// `/tmp/novo-<uid>/<hash>.sock` (pasta 0700), e cliente e daemon calculam o mesmo caminho.
    pub fn socket_path(&self) -> PathBuf {
        let normal = self.runtime_dir().join("novod.sock");
        if normal.as_os_str().len() < 100 {
            return normal;
        }
        use sha2::{Digest, Sha256};
        let h = hex::encode(Sha256::digest(self.root.to_string_lossy().as_bytes()));
        let uid = std::env::var("USER").unwrap_or_else(|_| "user".into());
        let dir = PathBuf::from(format!("/tmp/novo-{uid}"));
        let _ = fs::create_dir_all(&dir);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
        }
        dir.join(format!("{}.sock", &h[..16]))
    }

    pub fn inbox_dir(&self) -> PathBuf {
        self.root.join("inbox")
    }
    pub fn prompts_dir(&self) -> PathBuf {
        self.root.join("prompts")
    }
    pub fn trash_dir(&self) -> PathBuf {
        self.root.join("trash")
    }
    pub fn focus_path(&self, day: NaiveDate) -> PathBuf {
        self.root.join("focus").join(format!("{}.md", day.format("%Y-%m-%d")))
    }
    pub fn runs_path(&self, day: NaiveDate) -> PathBuf {
        self.root.join("agent").join("runs").join(format!("{}.jsonl", day.format("%Y-%m-%d")))
    }
    pub fn box_path(&self, id: &str) -> PathBuf {
        self.root.join("boxes").join(format!("{id}.md"))
    }
    pub fn box_versions_dir(&self, id: &str) -> PathBuf {
        self.root.join("boxes").join(".versions").join(id)
    }
    pub fn superthread_path(&self, id: &str) -> PathBuf {
        self.root.join("superthreads").join(format!("{id}.json"))
    }
    pub fn chat_path(&self, id: &str) -> PathBuf {
        self.root.join("chats").join(format!("{id}.jsonl"))
    }
    pub fn task_path(&self, id: &str) -> PathBuf {
        self.root.join("tasks").join(format!("{id}.md"))
    }
    pub fn project_path(&self, id: &str) -> PathBuf {
        self.root.join("projects").join(format!("{id}.md"))
    }
    pub fn note_path(&self, id: &str) -> PathBuf {
        self.root.join("notes").join(format!("{id}.md"))
    }

    /// `threads/AAAA/MM/<id>.jsonl`, com o mês tirado do tempo codificado no id (GMT-3).
    pub fn thread_path(&self, id: &str) -> PathBuf {
        if let Some(ms) = millis_of(id) {
            let d = time::from_millis(ms);
            let p = self.root.join("threads").join(format!("{:04}", d.year())).join(format!("{:02}", d.month())).join(format!("{id}.jsonl"));
            if p.exists() {
                return p;
            }
            if let Some(found) = self.find_thread_file(id) {
                return found;
            }
            return p;
        }
        self.find_thread_file(id).unwrap_or_else(|| self.root.join("threads").join(format!("{id}.jsonl")))
    }

    /// Pasta de anexos da thread, ao lado do JSONL.
    pub fn thread_dir(&self, id: &str) -> PathBuf {
        let p = self.thread_path(id);
        p.with_extension("")
    }

    fn find_thread_file(&self, id: &str) -> Option<PathBuf> {
        let name = format!("{id}.jsonl");
        walk(&self.root.join("threads"), &|p| p.file_name().and_then(|n| n.to_str()) == Some(name.as_str())).into_iter().next()
    }

    // ---------------------------------------------------------------- config
    pub fn config(&self) -> Config {
        fs::read_to_string(self.config_path()).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save_config(&self, c: &Config) -> Result<()> {
        atomic_write(&self.config_path(), toml::to_string_pretty(c)?.as_bytes())
    }

    // ---------------------------------------------------------------- threads
    pub fn create_thread(&self, id: &str, header: Header, root_body: &str, ts: Ts) -> Result<ThreadState> {
        let path = self.thread_path(id);
        if path.exists() {
            bail!("thread {id} já existe");
        }
        let created = Event { schema: crate::SCHEMA, id: id.to_string(), ts, body: EventBody::Created(header) };
        append_jsonl(&path, &created)?;
        let post = Event {
            schema: crate::SCHEMA,
            id: "p2".into(),
            ts,
            body: EventBody::Post { author: "voce".into(), body: root_body.to_string(), reply_to: None },
        };
        append_jsonl(&path, &post)?;
        self.load_thread(id)?.context("thread recém-criada ilegível")
    }

    pub fn load_events(&self, id: &str) -> Result<Vec<Event>> {
        Ok(read_jsonl(&self.thread_path(id))?.0)
    }

    pub fn load_thread(&self, id: &str) -> Result<Option<ThreadState>> {
        let events = self.load_events(id)?;
        Ok(ThreadState::fold(&events))
    }

    /// Acrescenta um evento; o id segue a sequência da thread.
    pub fn append_event(&self, id: &str, body: EventBody, ts: Ts) -> Result<(Event, ThreadState)> {
        let st = self.load_thread(id)?.with_context(|| format!("thread {id} não existe"))?;
        let is_post = matches!(body, EventBody::Post { .. } | EventBody::AgentReply { .. });
        let ev = Event { schema: crate::SCHEMA, id: st.next_id(is_post), ts, body };
        append_jsonl(&self.thread_path(id), &ev)?;
        let st = self.load_thread(id)?.context("thread ilegível após acréscimo")?;
        Ok((ev, st))
    }

    pub fn thread_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = walk(&self.root.join("threads"), &|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
            .into_iter()
            .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(String::from))
            .collect();
        ids.sort();
        ids
    }

    pub fn all_threads(&self) -> Vec<ThreadState> {
        self.thread_ids().iter().filter_map(|id| self.load_thread(id).ok().flatten()).collect()
    }

    // ---------------------------------------------------------------- lixeira
    /// Move a thread (JSONL + anexos + caixa) para `trash/<dia>/<id>/`. Devolve a pasta da lixeira.
    pub fn trash_thread(&self, id: &str, day: NaiveDate) -> Result<PathBuf> {
        let src = self.thread_path(id);
        if !src.exists() {
            bail!("thread {id} não existe");
        }
        let dst = self.trash_dir().join(day.format("%Y-%m-%d").to_string()).join(id);
        fs::create_dir_all(&dst)?;
        let rel = src.strip_prefix(&self.root).unwrap_or(&src).to_string_lossy().to_string();
        fs::write(dst.join("origin.txt"), rel)?;
        fs::rename(&src, dst.join(format!("{id}.jsonl")))?;
        let att = self.thread_dir(id);
        if att.exists() {
            fs::rename(&att, dst.join("attachments"))?;
        }
        let b = self.box_path(id);
        if b.exists() {
            fs::rename(&b, dst.join("box.md"))?;
        }
        Ok(dst)
    }

    /// Restaura uma thread da lixeira. Devolve `true` se encontrou.
    pub fn restore_thread(&self, id: &str) -> Result<bool> {
        let Some(dir) = self.trashed_dir(id) else { return Ok(false) };
        let origin = fs::read_to_string(dir.join("origin.txt")).unwrap_or_default();
        let origin = origin.trim();
        let safe = !origin.is_empty() && Path::new(origin).components().all(|c| matches!(c, std::path::Component::Normal(_)));
        let dst = if safe { self.root.join(origin) } else { self.thread_path(id) };
        // confere tudo antes de mover qualquer coisa: uma falha no meio deixaria a thread pela metade
        if dst.exists() {
            anyhow::bail!("já existe uma thread em {}", dst.display());
        }
        if !dir.join(format!("{id}.jsonl")).is_file() {
            anyhow::bail!("a lixeira de {id} está incompleta");
        }
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(dir.join(format!("{id}.jsonl")), &dst)?;
        if dir.join("attachments").exists() {
            let att = dst.with_extension("");
            if att.exists() {
                // sobra de um job que rodou depois da exclusão: junta, com a versão da lixeira por cima
                merge_into(&dir.join("attachments"), &att)?;
            } else {
                fs::rename(dir.join("attachments"), att)?;
            }
        }
        if dir.join("box.md").exists() {
            fs::rename(dir.join("box.md"), self.box_path(id))?;
        }
        fs::remove_dir_all(&dir)?;
        Ok(true)
    }

    pub fn trashed_dir(&self, id: &str) -> Option<PathBuf> {
        let days = fs::read_dir(self.trash_dir()).ok()?;
        for d in days.flatten() {
            let cand = d.path().join(id);
            if cand.is_dir() {
                return Some(cand);
            }
        }
        None
    }

    /// Itens na lixeira: (dia, id).
    pub fn trash_list(&self) -> Vec<(NaiveDate, String)> {
        let mut out = vec![];
        if let Ok(days) = fs::read_dir(self.trash_dir()) {
            for d in days.flatten() {
                let Some(day) = d.file_name().to_str().and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()) else { continue };
                if let Ok(items) = fs::read_dir(d.path()) {
                    for it in items.flatten() {
                        if let Some(id) = it.file_name().to_str() {
                            out.push((day, id.to_string()));
                        }
                    }
                }
            }
        }
        out.sort();
        out
    }

    /// Apaga da lixeira o que passou de `days` dias. Devolve os ids apagados.
    pub fn purge_trash(&self, today: NaiveDate, days: i64) -> Result<Vec<String>> {
        let mut gone = vec![];
        if let Ok(entries) = fs::read_dir(self.trash_dir()) {
            for d in entries.flatten() {
                let Some(day) = d.file_name().to_str().and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()) else { continue };
                if (today - day).num_days() >= days {
                    if let Ok(items) = fs::read_dir(d.path()) {
                        gone.extend(items.flatten().filter_map(|i| i.file_name().to_str().map(String::from)));
                    }
                    fs::remove_dir_all(d.path())?;
                }
            }
        }
        Ok(gone)
    }

    // ---------------------------------------------------------------- tarefas, projetos, anotações
    pub fn save_task(&self, t: &Task) -> Result<()> {
        atomic_write(&self.task_path(&t.head.id), frontmatter::render(&t.head, &t.description)?.as_bytes())
    }
    pub fn load_task(&self, id: &str) -> Result<Option<Task>> {
        load_md::<TaskHead>(&self.task_path(id)).map(|o| o.map(|(head, description)| Task { head, description }))
    }
    pub fn tasks(&self) -> Vec<Task> {
        list_md::<TaskHead>(&self.root.join("tasks")).into_iter().map(|(head, description)| Task { head, description }).collect()
    }
    pub fn delete_task(&self, id: &str) -> Result<()> {
        let p = self.task_path(id);
        if p.exists() {
            let dst = self.trash_dir().join(time::day_of(&time::now()).format("%Y-%m-%d").to_string()).join(id);
            fs::create_dir_all(&dst)?;
            fs::rename(p, dst.join(format!("{id}.md")))?;
        }
        Ok(())
    }

    pub fn save_project(&self, p: &Project) -> Result<()> {
        atomic_write(&self.project_path(&p.head.id), frontmatter::render(&p.head, &p.body)?.as_bytes())
    }
    pub fn load_project(&self, id: &str) -> Result<Option<Project>> {
        load_md::<ProjectHead>(&self.project_path(id)).map(|o| o.map(|(head, body)| Project { head, body }))
    }
    pub fn projects(&self) -> Vec<Project> {
        list_md::<ProjectHead>(&self.root.join("projects")).into_iter().map(|(head, body)| Project { head, body }).collect()
    }

    pub fn save_note(&self, n: &Note) -> Result<()> {
        atomic_write(&self.note_path(&n.head.id), frontmatter::render(&n.head, &n.body)?.as_bytes())
    }
    pub fn load_note(&self, id: &str) -> Result<Option<Note>> {
        load_md::<NoteHead>(&self.note_path(id)).map(|o| o.map(|(head, body)| Note { head, body }))
    }
    pub fn notes(&self) -> Vec<Note> {
        list_md::<NoteHead>(&self.root.join("notes")).into_iter().map(|(head, body)| Note { head, body }).collect()
    }

    // ---------------------------------------------------------------- superthreads
    pub fn save_superthread(&self, m: &SuperthreadMeta) -> Result<()> {
        write_json(&self.superthread_path(&m.id), m)
    }
    pub fn load_superthread(&self, id: &str) -> Result<Option<SuperthreadMeta>> {
        let p = self.superthread_path(id);
        if !p.exists() {
            return Ok(None);
        }
        Ok(Some(read_json(&p)?))
    }

    // ---------------------------------------------------------------- acessos
    pub fn log_access(&self, thread: &str, action: &str) -> Result<()> {
        append_jsonl(&self.access_path(), &AccessEvent { ts: time::now(), thread: thread.into(), action: action.into() })
    }
    pub fn access_log(&self) -> Vec<AccessEvent> {
        read_jsonl(&self.access_path()).map(|r| r.0).unwrap_or_default()
    }
}

fn load_md<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<(T, String)>> {
    match fs::read_to_string(path) {
        Ok(s) => {
            let (head, body): (T, String) = frontmatter::parse(&s).with_context(|| format!("ler {}", path.display()))?;
            Ok(Some((head, body.trim_end().to_string())))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn list_md<T: serde::de::DeserializeOwned>(dir: &Path) -> Vec<(T, String)> {
    let mut out = vec![];
    if let Ok(entries) = fs::read_dir(dir) {
        let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().and_then(|e| e.to_str()) == Some("md")).collect();
        paths.sort();
        for p in paths {
            if let Ok(Some(x)) = load_md::<T>(&p) {
                out.push(x);
            }
        }
    }
    out
}

/// Arquivos sob `dir` (recursivo) que satisfazem `keep`, ignorando pastas ocultas.
pub fn walk(dir: &Path, keep: &dyn Fn(&Path) -> bool) -> Vec<PathBuf> {
    let mut out = vec![];
    let Ok(entries) = fs::read_dir(dir) else { return out };
    for e in entries.flatten() {
        let p = e.path();
        let hidden = p.file_name().and_then(|n| n.to_str()).map(|n| n.starts_with('.')).unwrap_or(false);
        if hidden {
            continue;
        }
        if p.is_dir() {
            out.extend(walk(&p, keep));
        } else if keep(&p) {
            out.push(p);
        }
    }
    out
}

/// Move o conteúdo de `src` para dentro de `dst`, substituindo o que tiver o mesmo nome.
fn merge_into(src: &Path, dst: &Path) -> Result<()> {
    for e in fs::read_dir(src)?.flatten() {
        let to = dst.join(e.file_name());
        if e.path().is_dir() && to.is_dir() {
            merge_into(&e.path(), &to)?;
            continue;
        }
        if to.is_dir() {
            fs::remove_dir_all(&to)?;
        } else if to.exists() {
            fs::remove_file(&to)?;
        }
        fs::rename(e.path(), &to)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread::{AgentMode, ThreadKind};

    fn header() -> Header {
        Header { kind: ThreadKind::Thinking, agent: AgentMode::Off, profile: None, private: false, source: "app".into(), material: vec![] }
    }

    #[test]
    fn thread_create_append_trash_restore() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        let id = crate::new_id("t");
        let st = v.create_thread(&id, header(), "olá", time::now()).unwrap();
        assert_eq!(st.posts.len(), 1);
        assert!(v.thread_path(&id).to_string_lossy().contains("/threads/20"));
        let (ev, st) = v.append_event(&id, EventBody::Read, time::now()).unwrap();
        assert_eq!(ev.id, "e3");
        assert!(st.read_at.is_some());
        assert_eq!(v.thread_ids(), vec![id.clone()]);
        v.trash_thread(&id, time::day_of(&time::now())).unwrap();
        assert!(v.thread_ids().is_empty());
        assert_eq!(v.trash_list().len(), 1);
        assert!(v.restore_thread(&id).unwrap());
        assert_eq!(v.load_thread(&id).unwrap().unwrap().posts.len(), 1);
        assert!(v.trash_list().is_empty());
    }

    #[test]
    fn purge_removes_old_trash_only() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        let a = crate::new_id("t");
        let b = crate::new_id("t");
        v.create_thread(&a, header(), "a", time::now()).unwrap();
        v.create_thread(&b, header(), "b", time::now()).unwrap();
        let today = time::day_of(&time::now());
        v.trash_thread(&a, today - chrono::Duration::days(31)).unwrap();
        v.trash_thread(&b, today).unwrap();
        assert_eq!(v.purge_trash(today, 30).unwrap(), vec![a]);
        assert_eq!(v.trash_list().len(), 1);
    }

    #[test]
    fn task_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        let now = time::now();
        let t = Task {
            head: TaskHead {
                kind: "task".into(),
                schema: 1,
                id: crate::new_id("k"),
                title: "Revisar".into(),
                project: None,
                parent: None,
                due: Some("2026-10-27".into()),
                priority: 1,
                labels: vec!["marca".into()],
                status: "open".into(),
                completed_at: None,
                created: now,
                updated: now,
                origin: "manual".into(),
                order: 0,
                delegated_to: None,
            },
            description: "detalhes".into(),
        };
        v.save_task(&t).unwrap();
        assert_eq!(v.tasks(), vec![t.clone()]);
        assert_eq!(v.load_task(&t.head.id).unwrap().unwrap().description.trim(), "detalhes");
    }
}
