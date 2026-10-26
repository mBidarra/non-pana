//! O serviço do Novo: regras de negócio, fila de jobs e API. O `novod` só sobe os transportes.
//! Um único escritor: toda mutação passa por `Service::write` (spec 8, ADR "Processos").

pub mod chat;
pub mod dev;
pub mod focus_ops;
pub mod jobs;
pub mod quickadd;
pub mod rpc;
pub mod server;
pub mod superthreads;
pub mod tasks;
pub mod threads;
pub mod views;

use anyhow::Result;
use novo_core::Vault;
use novo_graph::Graph;
use novo_index::Index;
use serde_json::{Value, json};
use std::sync::{Mutex, RwLock};
use tokio::sync::{Notify, broadcast};

pub struct Service {
    pub vault: Vault,
    pub write: tokio::sync::Mutex<()>,
    pub graph: RwLock<Graph>,
    pub index: Mutex<Index>,
    pub events: broadcast::Sender<Value>,
    pub queue_wake: Notify,
}

impl Service {
    pub fn open(vault: Vault) -> Result<Service> {
        let graph = Graph::load(&vault.graph_path())?;
        let index = Index::open(&vault.index_path())?;
        let (tx, _) = broadcast::channel(512);
        let svc = Service {
            vault,
            write: tokio::sync::Mutex::new(()),
            graph: RwLock::new(graph),
            index: Mutex::new(index),
            events: tx,
            queue_wake: Notify::new(),
        };
        let empty = svc.index.lock().expect("index").count()? == 0;
        if empty {
            svc.reindex()?;
        }
        Ok(svc)
    }

    pub fn emit(&self, kind: &str, id: &str) {
        let _ = self.events.send(json!({"type": kind, "id": id, "ts": novo_core::time::now()}));
    }

    pub fn reindex(&self) -> Result<usize> {
        let g = self.graph.read().expect("graph").clone();
        let n = self.index.lock().expect("index").rebuild(&self.vault, &g)?;
        Ok(n)
    }

    /// Reindexa um item (thread, superthread, tarefa, projeto, anotação) e copia o grafo.
    /// Ordem das travas em todo o serviço: nunca segurar `graph` ao pegar `index` (copia o grafo antes).
    pub fn reindex_one(&self, id: &str) {
        let g = self.graph();
        let idx = self.index.lock().expect("index");
        let res = if id.starts_with("t-") {
            match self.vault.load_thread(id) {
                Ok(Some(st)) => idx.upsert(&novo_index::thread_doc(&self.vault, &st)),
                _ => idx.remove(id),
            }
        } else if id.starts_with("st-") {
            if g.children(id).is_empty() { idx.remove(id) } else { idx.upsert(&novo_index::superthread_doc(&self.vault, &g, id)) }
        } else if id.starts_with("k-") {
            match self.vault.load_task(id) {
                Ok(Some(t)) => idx.upsert(&novo_index::task_doc(&t)),
                _ => idx.remove(id),
            }
        } else if id.starts_with("n-") {
            match self.vault.load_note(id) {
                Ok(Some(n)) => idx.upsert(&novo_index::Doc {
                    id: n.head.id.clone(),
                    kind: "note".into(),
                    title: n.head.title.clone(),
                    body: n.body.clone(),
                    created: n.head.created.timestamp(),
                    updated: n.head.updated.timestamp(),
                    ..Default::default()
                }),
                _ => idx.remove(id),
            }
        } else if id.starts_with("p-") {
            match self.vault.load_project(id) {
                Ok(Some(p)) => idx.upsert(&novo_index::Doc {
                    id: p.head.id.clone(),
                    kind: "project".into(),
                    title: p.head.name.clone(),
                    body: p.body.clone(),
                    subtype: p.head.ptype.clone(),
                    created: p.head.created.timestamp(),
                    updated: p.head.created.timestamp(),
                    ..Default::default()
                }),
                _ => idx.remove(id),
            }
        } else {
            Ok(())
        };
        if let Err(e) = res {
            tracing::warn!("reindexar {id}: {e:#}");
        }
        let _ = idx.sync_graph(&g);
    }

    pub fn graph(&self) -> Graph {
        self.graph.read().expect("graph").clone()
    }

    /// Grava uma mudança do grafo (log + estado em memória + índice).
    pub fn commit_graph(&self, change: &novo_graph::Change) -> Result<()> {
        let g = {
            let mut g = self.graph.write().expect("graph");
            g.commit(&self.vault.graph_path(), change)?;
            g.clone()
        };
        self.index.lock().expect("index").sync_graph(&g)?;
        Ok(())
    }
}
