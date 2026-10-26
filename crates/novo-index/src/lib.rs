//! Índice derivado (spec 8.1): SQLite com FTS5, entidades e arestas. Pode ser apagado e reconstruído (RNF-15).
//! Busca (RF-BUS-01, 03; RF-MEM-09): texto (bm25), entidades das caixas e vizinhos no grafo, fundidos por RRF.

use anyhow::{Context, Result};
use novo_core::text::clip;
use novo_core::{ThreadState, Vault};
use novo_graph::Graph;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

const RRF_K: f64 = 60.0;

pub struct Index {
    conn: Connection,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Doc {
    pub id: String,
    /// thread | superthread | task | note | project
    pub kind: String,
    pub title: String,
    pub body: String,
    /// thinking | reading | watching (threads); open | done (tarefas)
    pub subtype: String,
    pub created: i64,
    pub updated: i64,
    pub read: bool,
    pub private: bool,
    pub site: String,
    pub entities: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Filters {
    #[serde(default)]
    pub kinds: Vec<String>,
    #[serde(default)]
    pub subtype: Option<String>,
    /// Só itens relacionados a este projeto (direta ou indiretamente pela superthread).
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub since: Option<i64>,
    #[serde(default)]
    pub until: Option<i64>,
    /// "read" | "pending"
    #[serde(default)]
    pub read: Option<String>,
    #[serde(default)]
    pub include_private: bool,
    #[serde(default)]
    pub site: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Hit {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub snippet: String,
    pub score: f64,
    /// fts | entity | graph (fontes que acharam o item)
    pub via: Vec<String>,
}

impl Index {
    pub fn open(path: &Path) -> Result<Index> {
        if let Some(d) = path.parent() {
            fs::create_dir_all(d)?;
        }
        let conn = Connection::open(path).with_context(|| format!("abrir índice {}", path.display()))?;
        let idx = Index { conn };
        idx.init()?;
        Ok(idx)
    }

    pub fn in_memory() -> Result<Index> {
        let idx = Index { conn: Connection::open_in_memory()? };
        idx.init()?;
        Ok(idx)
    }

    fn init(&self) -> Result<()> {
        self.conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS docs(
                id TEXT PRIMARY KEY, kind TEXT NOT NULL, title TEXT NOT NULL, body TEXT NOT NULL,
                subtype TEXT NOT NULL DEFAULT '', created INTEGER NOT NULL, updated INTEGER NOT NULL,
                read INTEGER NOT NULL DEFAULT 0, private INTEGER NOT NULL DEFAULT 0, site TEXT NOT NULL DEFAULT '');
             CREATE VIRTUAL TABLE IF NOT EXISTS docs_fts USING fts5(id UNINDEXED, title, body, tokenize='unicode61 remove_diacritics 2');
             CREATE TABLE IF NOT EXISTS entities(entity TEXT NOT NULL, doc TEXT NOT NULL, PRIMARY KEY(entity, doc));
             CREATE TABLE IF NOT EXISTS edges(type TEXT NOT NULL, src TEXT NOT NULL, dst TEXT NOT NULL, PRIMARY KEY(type, src, dst));
             CREATE INDEX IF NOT EXISTS edges_dst ON edges(dst);
             CREATE INDEX IF NOT EXISTS docs_kind ON docs(kind, created);",
        )?;
        Ok(())
    }

    pub fn upsert(&self, d: &Doc) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        write_doc(&tx, d)?;
        tx.commit()?;
        Ok(())
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM docs WHERE id = ?1", params![id])?;
        self.conn.execute("DELETE FROM docs_fts WHERE id = ?1", params![id])?;
        self.conn.execute("DELETE FROM entities WHERE doc = ?1", params![id])?;
        Ok(())
    }

    pub fn count(&self) -> Result<i64> {
        Ok(self.conn.query_row("SELECT count(*) FROM docs", [], |r| r.get(0))?)
    }

    pub fn get(&self, id: &str) -> Result<Option<Doc>> {
        Ok(self
            .conn
            .query_row("SELECT id, kind, title, body, subtype, created, updated, read, private, site FROM docs WHERE id = ?1", params![id], |r| {
                Ok(Doc {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    title: r.get(2)?,
                    body: r.get(3)?,
                    subtype: r.get(4)?,
                    created: r.get(5)?,
                    updated: r.get(6)?,
                    read: r.get::<_, i64>(7)? != 0,
                    private: r.get::<_, i64>(8)? != 0,
                    site: r.get(9)?,
                    entities: vec![],
                })
            })
            .optional()?)
    }

    /// Copia as arestas atuais do grafo para a tabela `edges`.
    pub fn sync_graph(&self, g: &Graph) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM edges", [])?;
        for (t, f, to, _) in g.edges() {
            tx.execute("INSERT OR IGNORE INTO edges(type, src, dst) VALUES(?1, ?2, ?3)", params![format!("{t:?}"), f, to])?;
        }
        tx.commit()?;
        Ok(())
    }

    fn neighbors(&self, id: &str) -> Result<Vec<String>> {
        let mut st = self.conn.prepare_cached(
            "SELECT dst FROM edges WHERE src = ?1 UNION SELECT src FROM edges WHERE dst = ?1
             UNION SELECT e2.src FROM edges e1 JOIN edges e2 ON e1.dst = e2.dst AND e1.type = 'BlockOf' AND e2.type = 'BlockOf' WHERE e1.src = ?1 AND e2.src != ?1",
        )?;
        let rows = st.query_map(params![id], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Itens ligados a um projeto, direta ou indiretamente (bloco de uma superthread ligada).
    fn in_project(&self, project: &str) -> Result<std::collections::HashSet<String>> {
        let mut out = std::collections::HashSet::new();
        let mut st = self.conn.prepare_cached("SELECT src FROM edges WHERE dst = ?1 AND type IN ('RelatesProject','RelatesTask')")?;
        let direct: Vec<String> = st.query_map(params![project], |r| r.get(0))?.filter_map(|r| r.ok()).collect();
        let mut stack = direct.clone();
        while let Some(x) = stack.pop() {
            if !out.insert(x.clone()) {
                continue;
            }
            let mut kids = self.conn.prepare_cached("SELECT src FROM edges WHERE dst = ?1 AND type = 'BlockOf'")?;
            stack.extend(kids.query_map(params![x], |r| r.get::<_, String>(0))?.filter_map(|r| r.ok()));
        }
        // tarefas do projeto
        let mut tk = self.conn.prepare_cached("SELECT id FROM docs WHERE kind = 'task' AND body LIKE ?1")?;
        out.extend(tk.query_map(params![format!("%project:{project}%")], |r| r.get::<_, String>(0))?.filter_map(|r| r.ok()));
        Ok(out)
    }

    fn passes(&self, d: &Doc, f: &Filters, project_set: &Option<std::collections::HashSet<String>>) -> bool {
        if !f.include_private && d.private {
            return false;
        }
        if !f.kinds.is_empty() && !f.kinds.iter().any(|k| k == &d.kind) {
            return false;
        }
        if let Some(s) = &f.subtype {
            if &d.subtype != s {
                return false;
            }
        }
        if let Some(s) = f.since {
            if d.created < s {
                return false;
            }
        }
        if let Some(u) = f.until {
            if d.created > u {
                return false;
            }
        }
        if let Some(r) = &f.read {
            if d.kind != "thread" || (r == "read") != d.read {
                return false;
            }
        }
        if let Some(site) = &f.site {
            if !d.site.contains(site.as_str()) {
                return false;
            }
        }
        if let Some(set) = project_set {
            if !set.contains(&d.id) {
                return false;
            }
        }
        true
    }

    /// Busca híbrida. Consulta vazia lista os itens mais recentes que passam nos filtros.
    pub fn search(&self, query: &str, f: &Filters, limit: usize) -> Result<Vec<Hit>> {
        let project_set = match &f.project {
            Some(p) => Some(self.in_project(p)?),
            None => None,
        };
        let q = query.trim();
        if q.is_empty() {
            let mut st = self.conn.prepare("SELECT id FROM docs ORDER BY created DESC LIMIT 2000")?;
            let ids: Vec<String> = st.query_map([], |r| r.get(0))?.filter_map(|r| r.ok()).collect();
            let mut out = vec![];
            for id in ids {
                if let Some(d) = self.get(&id)? {
                    if self.passes(&d, f, &project_set) {
                        out.push(Hit { id: d.id, kind: d.kind, title: d.title, snippet: clip(&d.body, 160), score: 0.0, via: vec![] });
                        if out.len() >= limit {
                            break;
                        }
                    }
                }
            }
            return Ok(out);
        }
        let mut scores: HashMap<String, (f64, Vec<String>)> = HashMap::new();
        let mut add = |id: &str, rank: usize, via: &str| {
            let e = scores.entry(id.to_string()).or_insert((0.0, vec![]));
            e.0 += 1.0 / (RRF_K + rank as f64 + 1.0);
            if !e.1.iter().any(|v| v == via) {
                e.1.push(via.to_string());
            }
        };
        // 1. texto
        let fts_query = fts_expr(q);
        let mut fts_ids = vec![];
        if !fts_query.is_empty() {
            let mut st = self.conn.prepare("SELECT id FROM docs_fts WHERE docs_fts MATCH ?1 ORDER BY bm25(docs_fts, 0.0, 4.0, 1.0) LIMIT 200")?;
            fts_ids = st.query_map(params![fts_query], |r| r.get::<_, String>(0))?.filter_map(|r| r.ok()).collect();
        }
        for (i, id) in fts_ids.iter().enumerate() {
            add(id, i, "fts");
        }
        // 2. entidades das caixas
        let terms: Vec<String> = q.split_whitespace().map(|t| t.to_lowercase()).collect();
        let mut ent_ids: Vec<String> = vec![];
        {
            let mut st = self.conn.prepare_cached("SELECT DISTINCT doc FROM entities WHERE entity = ?1 OR entity LIKE ?2")?;
            for t in &terms {
                let rows = st.query_map(params![t, format!("%{t}%")], |r| r.get::<_, String>(0))?;
                for id in rows.flatten() {
                    if !ent_ids.contains(&id) {
                        ent_ids.push(id);
                    }
                }
            }
        }
        for (i, id) in ent_ids.iter().enumerate() {
            add(id, i, "entity");
        }
        // 3. vizinhos no grafo dos 10 primeiros resultados de texto
        let mut graph_rank = 0;
        for id in fts_ids.iter().take(10) {
            for n in self.neighbors(id)? {
                add(&n, graph_rank, "graph");
                graph_rank += 1;
            }
        }
        let mut ranked: Vec<(String, f64, Vec<String>)> = scores.into_iter().map(|(k, (s, v))| (k, s, v)).collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
        let mut out = vec![];
        for (id, score, via) in ranked {
            let Some(d) = self.get(&id)? else { continue };
            if !self.passes(&d, f, &project_set) {
                continue;
            }
            out.push(Hit { snippet: snippet(&d.body, &terms), id: d.id, kind: d.kind, title: d.title, score, via });
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    /// Reconstrói tudo a partir do cofre (RNF-15).
    pub fn rebuild(&self, vault: &Vault, graph: &Graph) -> Result<usize> {
        self.conn.execute_batch("DELETE FROM docs; DELETE FROM docs_fts; DELETE FROM entities; DELETE FROM edges;")?;
        let tx = self.conn.unchecked_transaction()?;
        let mut n = 0;
        for st in vault.all_threads() {
            write_doc(&tx, &thread_doc(vault, &st))?;
            n += 1;
        }
        for t in vault.tasks() {
            write_doc(&tx, &task_doc(&t))?;
            n += 1;
        }
        for p in vault.projects() {
            write_doc(&tx, &Doc {
                id: p.head.id.clone(),
                kind: "project".into(),
                title: p.head.name.clone(),
                body: p.body.clone(),
                subtype: p.head.ptype.clone(),
                created: p.head.created.timestamp(),
                updated: p.head.created.timestamp(),
                ..Default::default()
            })?;
            n += 1;
        }
        for note in vault.notes() {
            write_doc(&tx, &Doc {
                id: note.head.id.clone(),
                kind: "note".into(),
                title: note.head.title.clone(),
                body: note.body.clone(),
                created: note.head.created.timestamp(),
                updated: note.head.updated.timestamp(),
                ..Default::default()
            })?;
            n += 1;
        }
        for st in graph.superthreads() {
            write_doc(&tx, &superthread_doc(vault, graph, &st))?;
            n += 1;
        }
        tx.commit()?;
        self.sync_graph(graph)?;
        Ok(n)
    }
}

fn write_doc(c: &Connection, d: &Doc) -> Result<()> {
    c.execute("DELETE FROM docs_fts WHERE id = ?1", params![d.id])?;
    c.execute("DELETE FROM entities WHERE doc = ?1", params![d.id])?;
    c.execute(
        "INSERT INTO docs(id, kind, title, body, subtype, created, updated, read, private, site) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
         ON CONFLICT(id) DO UPDATE SET kind=excluded.kind, title=excluded.title, body=excluded.body, subtype=excluded.subtype,
           created=excluded.created, updated=excluded.updated, read=excluded.read, private=excluded.private, site=excluded.site",
        params![d.id, d.kind, d.title, d.body, d.subtype, d.created, d.updated, d.read as i64, d.private as i64, d.site],
    )?;
    c.execute("INSERT INTO docs_fts(id, title, body) VALUES(?1, ?2, ?3)", params![d.id, d.title, d.body])?;
    for e in &d.entities {
        c.execute("INSERT OR IGNORE INTO entities(entity, doc) VALUES(?1, ?2)", params![e.to_lowercase(), d.id])?;
    }
    Ok(())
}

/// Expressão FTS5 segura: cada termo vira um prefixo entre aspas, ligados por OR.
fn fts_expr(q: &str) -> String {
    q.split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.chars().count() >= 2)
        .map(|t| format!("\"{}\"*", t.replace('"', "")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn snippet(body: &str, terms: &[String]) -> String {
    let lower = body.to_lowercase();
    let at = terms.iter().filter_map(|t| lower.find(t.as_str())).min().unwrap_or(0);
    let start = lower[..at].char_indices().rev().nth(60).map(|(i, _)| i).unwrap_or(0);
    let start = (0..=start).rev().find(|i| body.is_char_boundary(*i)).unwrap_or(0);
    let s = clip(&body[start..], 200);
    if start > 0 { format!("…{s}") } else { s }
}

pub fn thread_doc(vault: &Vault, st: &ThreadState) -> Doc {
    let mut body = String::new();
    for p in &st.posts {
        body.push_str(&p.body);
        body.push('\n');
    }
    let dir = vault.thread_dir(&st.id);
    for m in &st.material {
        if let Some(t) = &m.title {
            body.push_str(t);
            body.push('\n');
        }
        if let Some(snap) = &m.snapshot {
            if let Ok(s) = fs::read_to_string(dir.join(snap)) {
                body.push_str(&clip(&s, 200_000));
                body.push('\n');
            }
        }
    }
    let mut entities = vec![];
    if let Ok(Some(b)) = novo_memory::load_box(vault, &st.id) {
        body.push_str(&b.full());
        entities = b.head.entities.clone();
    }
    Doc {
        id: st.id.clone(),
        kind: "thread".into(),
        title: st.title(),
        body,
        subtype: format!("{:?}", st.kind).to_lowercase(),
        created: st.created.timestamp(),
        updated: st.last_activity.timestamp(),
        read: st.read_at.is_some(),
        private: st.private,
        site: st.material.iter().filter_map(|m| m.site.clone()).collect::<Vec<_>>().join(" "),
        entities,
    }
}

pub fn task_doc(t: &novo_core::model::Task) -> Doc {
    let project = t.head.project.as_ref().map(|p| format!("\nproject:{p}")).unwrap_or_default();
    Doc {
        id: t.head.id.clone(),
        kind: "task".into(),
        title: t.head.title.clone(),
        body: format!("{}\n{}{}", t.description, t.head.labels.join(" "), project),
        subtype: t.head.status.clone(),
        created: t.head.created.timestamp(),
        updated: t.head.updated.timestamp(),
        ..Default::default()
    }
}

pub fn superthread_doc(vault: &Vault, graph: &Graph, id: &str) -> Doc {
    let meta = vault.load_superthread(id).ok().flatten();
    let title = meta.as_ref().map(|m| m.title.clone()).unwrap_or_else(|| id.to_string());
    let mut body = String::new();
    let mut entities = vec![];
    if let Ok(Some(b)) = novo_memory::load_box(vault, id) {
        body.push_str(&b.full());
        entities = b.head.entities.clone();
    }
    body.push_str(&format!("\nblocos: {}", graph.children(id).join(" ")));
    Doc {
        id: id.to_string(),
        kind: "superthread".into(),
        title,
        body,
        created: meta.as_ref().map(|m| m.created.timestamp()).unwrap_or(0),
        updated: meta.as_ref().map(|m| m.created.timestamp()).unwrap_or(0),
        entities,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use novo_core::thread::{AgentMode, Header, ThreadKind};
    use novo_core::time;

    fn doc(id: &str, title: &str, body: &str, ents: &[&str]) -> Doc {
        Doc { id: id.into(), kind: "thread".into(), title: title.into(), body: body.into(), created: 1, updated: 1, entities: ents.iter().map(|s| s.to_string()).collect(), ..Default::default() }
    }

    #[test]
    fn fts5_with_accents_and_rrf() {
        let idx = Index::in_memory().unwrap();
        idx.upsert(&doc("t-1", "Onboarding em 3 passos", "Mostre valor antes do cadastro", &["onboarding"])).unwrap();
        idx.upsert(&doc("t-2", "Preço por uso", "Precificação de SaaS", &["pricing"])).unwrap();
        idx.upsert(&doc("t-3", "Receita", "nada a ver", &[])).unwrap();
        let hits = idx.search("precificacao", &Filters::default(), 10).unwrap();
        assert_eq!(hits[0].id, "t-2", "remove_diacritics: {hits:?}");
        let hits = idx.search("pricing", &Filters::default(), 10).unwrap();
        assert_eq!(hits[0].id, "t-2");
        assert!(hits[0].via.contains(&"entity".to_string()));
        // vizinho no grafo entra pela fusão
        let mut g = Graph::default();
        let ch = g.relate("t-1", "t-3", || "st-1".into()).unwrap();
        for e in &ch.events {
            g.apply(e);
        }
        idx.sync_graph(&g).unwrap();
        let hits = idx.search("onboarding", &Filters::default(), 10).unwrap();
        let ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids[0], "t-1");
        assert!(ids.contains(&"t-3"), "vizinho t-3 deveria vir via grafo: {hits:?}");
    }

    #[test]
    fn filters_and_private() {
        let idx = Index::in_memory().unwrap();
        let mut a = doc("t-a", "privada", "segredo", &[]);
        a.private = true;
        idx.upsert(&a).unwrap();
        idx.upsert(&doc("t-b", "publica", "segredo", &[])).unwrap();
        let hits = idx.search("segredo", &Filters::default(), 10).unwrap();
        assert_eq!(hits.len(), 1);
        let hits = idx.search("segredo", &Filters { include_private: true, ..Default::default() }, 10).unwrap();
        assert_eq!(hits.len(), 2);
        let hits = idx.search("", &Filters { read: Some("pending".into()), ..Default::default() }, 10).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn rebuild_from_vault() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        for i in 0..5 {
            let h = Header { kind: ThreadKind::Thinking, agent: AgentMode::Off, profile: None, private: false, source: "app".into(), material: vec![] };
            v.create_thread(&novo_core::new_id("t"), h, &format!("pensamento número {i} sobre ativação"), time::now()).unwrap();
        }
        let idx = Index::open(&v.index_path()).unwrap();
        assert_eq!(idx.rebuild(&v, &Graph::default()).unwrap(), 5);
        assert_eq!(idx.search("ativacao", &Filters::default(), 10).unwrap().len(), 5);
        // apagar o índice e reconstruir não perde nada
        drop(idx);
        fs::remove_file(v.index_path()).unwrap();
        let idx = Index::open(&v.index_path()).unwrap();
        assert_eq!(idx.count().unwrap(), 0);
        idx.rebuild(&v, &Graph::default()).unwrap();
        assert_eq!(idx.count().unwrap(), 5);
    }
}
