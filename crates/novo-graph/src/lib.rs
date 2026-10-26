//! Grafo explícito (spec 3.2, RF-SUP, RF-MEM-03): só arestas criadas por ações do usuário.
//!
//! - `block_of`: thread ou superthread → superthread (composição em árvore, um pai só).
//! - `relates_project` / `relates_task`: vínculos N:N com projetos e tarefas.
//!
//! O arquivo `graph.jsonl` é um log de operações; o estado atual é a dobra do log.

use anyhow::{Result, bail};
use novo_core::fsx::{append_jsonl, read_jsonl};
use novo_core::time::{self, Ts};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum EdgeType {
    BlockOf,
    RelatesProject,
    RelatesTask,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Add,
    Remove,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct EdgeEvent {
    pub ts: Ts,
    pub op: Op,
    #[serde(rename = "type")]
    pub etype: EdgeType,
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pos: Option<i64>,
}

impl EdgeEvent {
    pub fn add(etype: EdgeType, from: &str, to: &str, pos: Option<i64>) -> Self {
        EdgeEvent { ts: time::now(), op: Op::Add, etype, from: from.into(), to: to.into(), pos }
    }
    pub fn remove(etype: EdgeType, from: &str, to: &str, pos: Option<i64>) -> Self {
        EdgeEvent { ts: time::now(), op: Op::Remove, etype, from: from.into(), to: to.into(), pos }
    }
    pub fn inverse(&self) -> Self {
        EdgeEvent { ts: time::now(), op: if self.op == Op::Add { Op::Remove } else { Op::Add }, ..self.clone() }
    }
}

/// O que uma operação de composição produziu, para o serviço aplicar e gravar.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Change {
    pub events: Vec<EdgeEvent>,
    /// Superthread criada pela operação.
    pub created: Option<String>,
    /// Superthreads que se desfizeram (ficaram com um bloco só).
    pub dissolved: Vec<String>,
}

pub fn is_superthread(id: &str) -> bool {
    id.starts_with("st-")
}
pub fn is_thread(id: &str) -> bool {
    id.starts_with("t-")
}
pub fn is_project(id: &str) -> bool {
    id.starts_with("p-")
}
pub fn is_task(id: &str) -> bool {
    id.starts_with("k-")
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Graph {
    /// (tipo, de, para) → posição.
    edges: BTreeMap<(EdgeType, String, String), Option<i64>>,
}

impl Graph {
    pub fn load(path: &Path) -> Result<Graph> {
        let (events, _bad): (Vec<EdgeEvent>, usize) = read_jsonl(path)?;
        let mut g = Graph::default();
        for e in &events {
            g.apply(e);
        }
        Ok(g)
    }

    pub fn apply(&mut self, e: &EdgeEvent) {
        let key = (e.etype, e.from.clone(), e.to.clone());
        match e.op {
            Op::Add => {
                self.edges.insert(key, e.pos);
            }
            Op::Remove => {
                self.edges.remove(&key);
            }
        }
    }

    /// Aplica e grava os eventos de uma mudança.
    pub fn commit(&mut self, path: &Path, change: &Change) -> Result<()> {
        for e in &change.events {
            append_jsonl(path, e)?;
            self.apply(e);
        }
        Ok(())
    }

    pub fn edges(&self) -> impl Iterator<Item = (EdgeType, &str, &str, Option<i64>)> {
        self.edges.iter().map(|((t, f, to), p)| (*t, f.as_str(), to.as_str(), *p))
    }

    pub fn parent(&self, node: &str) -> Option<String> {
        self.edges.keys().find(|(t, f, _)| *t == EdgeType::BlockOf && f == node).map(|(_, _, to)| to.clone())
    }

    fn pos_of(&self, child: &str, parent: &str) -> Option<i64> {
        self.edges.get(&(EdgeType::BlockOf, child.to_string(), parent.to_string())).copied().flatten()
    }

    /// Blocos de uma superthread, na ordem de `pos` (empate: id).
    pub fn children(&self, st: &str) -> Vec<String> {
        let mut kids: Vec<(i64, String)> = self
            .edges
            .iter()
            .filter(|((t, _, to), _)| *t == EdgeType::BlockOf && to == st)
            .map(|((_, f, _), p)| (p.unwrap_or(i64::MAX), f.clone()))
            .collect();
        kids.sort();
        kids.into_iter().map(|(_, id)| id).collect()
    }

    /// Todas as threads dentro de uma superthread, em profundidade, na ordem dos blocos.
    pub fn leaf_threads(&self, st: &str) -> Vec<String> {
        let mut out = vec![];
        for c in self.children(st) {
            if is_superthread(&c) {
                out.extend(self.leaf_threads(&c));
            } else {
                out.push(c);
            }
        }
        out
    }

    pub fn top(&self, node: &str) -> String {
        let mut cur = node.to_string();
        let mut seen = HashSet::new();
        while let Some(p) = self.parent(&cur) {
            if !seen.insert(cur.clone()) {
                break;
            }
            cur = p;
        }
        cur
    }

    /// Projetos e tarefas relacionados ao nó.
    pub fn relations(&self, node: &str) -> Vec<(EdgeType, String)> {
        self.edges
            .keys()
            .filter(|(t, f, _)| *t != EdgeType::BlockOf && f == node)
            .map(|(t, _, to)| (*t, to.clone()))
            .collect()
    }

    /// Threads e superthreads relacionadas a um projeto ou tarefa (lado inverso).
    pub fn related_to(&self, target: &str) -> Vec<String> {
        self.edges.keys().filter(|(t, _, to)| *t != EdgeType::BlockOf && to == target).map(|(_, f, _)| f.clone()).collect()
    }

    /// Superthreads que têm ao menos um bloco.
    pub fn superthreads(&self) -> BTreeSet<String> {
        self.edges.keys().filter(|(t, _, _)| *t == EdgeType::BlockOf).map(|(_, _, to)| to.clone()).collect()
    }

    // ------------------------------------------------------------ composição

    /// Relaciona `a` com `b`. Para threads e superthreads, monta peças pelo topo (RF-SUP-01 a 04);
    /// para projetos e tarefas, cria um vínculo (RF-MEN-07). `new_st` dá o id de uma superthread nova.
    pub fn relate(&self, a: &str, b: &str, new_st: impl FnOnce() -> String) -> Result<Change> {
        if a == b {
            bail!("uma peça não se relaciona consigo mesma");
        }
        if is_project(b) || is_task(b) {
            if !(is_thread(a) || is_superthread(a)) {
                bail!("só threads e superthreads se relacionam a projetos e tarefas");
            }
            let et = if is_project(b) { EdgeType::RelatesProject } else { EdgeType::RelatesTask };
            if self.edges.contains_key(&(et, a.to_string(), b.to_string())) {
                bail!("já relacionado");
            }
            return Ok(Change { events: vec![EdgeEvent::add(et, a, b, None)], ..Default::default() });
        }
        if is_project(a) || is_task(a) {
            return self.relate(b, a, new_st);
        }
        if !(is_thread(a) || is_superthread(a)) || !(is_thread(b) || is_superthread(b)) {
            bail!("tipo de peça desconhecido");
        }
        let ta = self.top(a);
        let tb = self.top(b);
        if ta == tb {
            bail!("as duas peças já estão no mesmo conjunto");
        }
        let mut ch = Change::default();
        match (is_superthread(&ta), is_superthread(&tb)) {
            (false, true) => {
                let pos = self.next_pos(&tb);
                ch.events.push(EdgeEvent::add(EdgeType::BlockOf, &ta, &tb, Some(pos)));
            }
            (true, false) => {
                let pos = self.next_pos(&ta);
                ch.events.push(EdgeEvent::add(EdgeType::BlockOf, &tb, &ta, Some(pos)));
            }
            _ => {
                let st = new_st();
                ch.events.push(EdgeEvent::add(EdgeType::BlockOf, &ta, &st, Some(0)));
                ch.events.push(EdgeEvent::add(EdgeType::BlockOf, &tb, &st, Some(1)));
                ch.created = Some(st);
            }
        }
        Ok(ch)
    }

    fn next_pos(&self, st: &str) -> i64 {
        self.edges
            .iter()
            .filter(|((t, _, to), _)| *t == EdgeType::BlockOf && to == st)
            .filter_map(|(_, p)| *p)
            .max()
            .map(|m| m + 1)
            .unwrap_or(0)
    }

    /// Desfaz um vínculo com projeto ou tarefa.
    pub fn unrelate(&self, a: &str, b: &str) -> Result<Change> {
        let et = if is_project(b) { EdgeType::RelatesProject } else if is_task(b) { EdgeType::RelatesTask } else { bail!("só vínculos com projetos e tarefas se desfazem assim; use desmontar") };
        if !self.edges.contains_key(&(et, a.to_string(), b.to_string())) {
            bail!("não há esse vínculo");
        }
        Ok(Change { events: vec![EdgeEvent::remove(et, a, b, None)], ..Default::default() })
    }

    /// Tira `block` da superthread em que está (RF-SUP-08). A superthread que ficar com um bloco se desfaz.
    pub fn disassemble(&self, block: &str) -> Result<Change> {
        let Some(parent) = self.parent(block) else { bail!("essa peça não está em nenhuma superthread") };
        let mut sim = self.clone();
        let mut ch = Change::default();
        let ev = EdgeEvent::remove(EdgeType::BlockOf, block, &parent, sim.pos_of(block, &parent));
        sim.apply(&ev);
        ch.events.push(ev);
        sim.dissolve_if_single(&parent, &mut ch);
        Ok(ch)
    }

    /// Tira um nó do grafo por inteiro (exclusão de thread): todos os vínculos e a composição.
    pub fn detach_node(&self, node: &str) -> Change {
        let mut sim = self.clone();
        let mut ch = Change::default();
        let rels: Vec<(EdgeType, String, String, Option<i64>)> = sim
            .edges
            .iter()
            .filter(|((t, f, to), _)| (f == node) || (*t != EdgeType::BlockOf && to == node))
            .map(|((t, f, to), p)| (*t, f.clone(), to.clone(), *p))
            .collect();
        let parent = sim.parent(node);
        for (t, f, to, p) in rels {
            let ev = EdgeEvent::remove(t, &f, &to, p);
            sim.apply(&ev);
            ch.events.push(ev);
        }
        // uma superthread excluída deixa os blocos soltos
        if is_superthread(node) {
            for c in sim.children(node) {
                let ev = EdgeEvent::remove(EdgeType::BlockOf, &c, node, sim.pos_of(&c, node));
                sim.apply(&ev);
                ch.events.push(ev);
            }
        }
        if let Some(p) = parent {
            sim.dissolve_if_single(&p, &mut ch);
        }
        ch
    }

    /// Se `st` ficou com um só bloco, ele se desfaz: o bloco restante ocupa o lugar de `st` no pai (se houver)
    /// e herda os vínculos de `st`, para nada se perder.
    fn dissolve_if_single(&mut self, st: &str, ch: &mut Change) {
        let kids = self.children(st);
        if kids.len() != 1 {
            return;
        }
        let only = kids[0].clone();
        let mut evs = vec![EdgeEvent::remove(EdgeType::BlockOf, &only, st, self.pos_of(&only, st))];
        if let Some(gp) = self.parent(st) {
            let pos = self.pos_of(st, &gp);
            evs.push(EdgeEvent::remove(EdgeType::BlockOf, st, &gp, pos));
            evs.push(EdgeEvent::add(EdgeType::BlockOf, &only, &gp, pos));
        }
        for (t, to) in self.relations(st) {
            evs.push(EdgeEvent::remove(t, st, &to, None));
            if !self.edges.contains_key(&(t, only.clone(), to.clone())) {
                evs.push(EdgeEvent::add(t, &only, &to, None));
            }
        }
        for e in evs {
            self.apply(&e);
            ch.events.push(e);
        }
        ch.dissolved.push(st.to_string());
    }

    /// Reordena os blocos de uma superthread (RF-SUP-06).
    pub fn reorder(&self, st: &str, order: &[String]) -> Result<Change> {
        let mut current = self.children(st);
        let mut wanted = order.to_vec();
        current.sort();
        wanted.sort();
        if current != wanted {
            bail!("a nova ordem precisa ter exatamente os blocos atuais");
        }
        let events = order.iter().enumerate().map(|(i, c)| EdgeEvent::add(EdgeType::BlockOf, c, st, Some(i as i64))).collect();
        Ok(Change { events, ..Default::default() })
    }

    // ------------------------------------------------------------ vizinhança

    /// Vizinhos de um nó para montar o contexto do agente (RF-MEM-04), mais próximos primeiro.
    /// Um salto inclui o pai, os irmãos de bloco, os blocos (se for superthread) e os projetos e tarefas relacionados.
    pub fn neighbors(&self, node: &str, hops: usize, max: usize) -> Vec<(String, usize)> {
        let adj = self.adjacency();
        let mut out = vec![];
        let mut seen: HashSet<String> = HashSet::from([node.to_string()]);
        let mut q = VecDeque::from([(node.to_string(), 0usize)]);
        while let Some((cur, d)) = q.pop_front() {
            if d == hops {
                continue;
            }
            let mut next: Vec<&String> = adj.get(&cur).map(|s| s.iter().collect()).unwrap_or_default();
            next.sort();
            for n in next {
                if seen.insert(n.clone()) {
                    out.push((n.clone(), d + 1));
                    if out.len() >= max {
                        return out;
                    }
                    q.push_back((n.clone(), d + 1));
                }
            }
        }
        out
    }

    fn adjacency(&self) -> HashMap<String, BTreeSet<String>> {
        let mut adj: HashMap<String, BTreeSet<String>> = HashMap::new();
        let mut link = |a: &str, b: &str| {
            adj.entry(a.to_string()).or_default().insert(b.to_string());
            adj.entry(b.to_string()).or_default().insert(a.to_string());
        };
        for (t, f, to) in self.edges.keys() {
            link(f, to);
            let _ = t;
        }
        for st in self.superthreads() {
            let kids = self.children(&st);
            for i in 0..kids.len() {
                for j in (i + 1)..kids.len() {
                    link(&kids[i], &kids[j]);
                }
            }
        }
        adj
    }

    /// Checagem de invariantes (usada nos testes e no `novo doctor`).
    pub fn check(&self) -> Result<()> {
        let mut parents: HashMap<&str, usize> = HashMap::new();
        for (t, f, _) in self.edges.keys() {
            if *t == EdgeType::BlockOf {
                *parents.entry(f.as_str()).or_default() += 1;
            }
        }
        if let Some((n, _)) = parents.iter().find(|(_, c)| **c > 1) {
            bail!("{n} tem mais de um pai");
        }
        for st in self.superthreads() {
            if self.children(&st).len() < 2 {
                bail!("{st} tem menos de dois blocos");
            }
            let mut seen = HashSet::new();
            let mut cur = st.clone();
            while let Some(p) = self.parent(&cur) {
                if !seen.insert(cur.clone()) {
                    bail!("ciclo em {st}");
                }
                cur = p;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn ids() -> impl FnMut() -> String {
        let n = Cell::new(0);
        move || {
            n.set(n.get() + 1);
            format!("st-{}", n.get())
        }
    }

    fn apply(g: &mut Graph, ch: &Change) {
        for e in &ch.events {
            g.apply(e);
        }
    }

    #[test]
    fn lego_rules_from_the_spec() {
        let mut g = Graph::default();
        let mut next = ids();
        // 1. thread + thread
        let ch = g.relate("t-a", "t-b", &mut next).unwrap();
        assert_eq!(ch.created.as_deref(), Some("st-1"));
        apply(&mut g, &ch);
        assert_eq!(g.children("st-1"), vec!["t-a", "t-b"]);
        // 2. thread + superthread (via um bloco dela): entra como bloco
        let ch = g.relate("t-c", "t-a", &mut next).unwrap();
        assert!(ch.created.is_none());
        apply(&mut g, &ch);
        assert_eq!(g.children("st-1"), vec!["t-a", "t-b", "t-c"]);
        // mesma peça: recusa
        assert!(g.relate("t-a", "t-c", &mut next).is_err());
        // 3. superthread + superthread
        { let ch = g.relate("t-d", "t-e", &mut next).unwrap(); apply(&mut g, &ch); }
        let ch = g.relate("st-1", "t-d", &mut next).unwrap();
        assert_eq!(ch.created.as_deref(), Some("st-3"));
        apply(&mut g, &ch);
        assert_eq!(g.children("st-3"), vec!["st-1", "st-2"]);
        assert_eq!(g.top("t-b"), "st-3");
        assert_eq!(g.leaf_threads("st-3"), vec!["t-a", "t-b", "t-c", "t-d", "t-e"]);
        // 4. vínculo com projeto e tarefa
        { let ch = g.relate("st-3", "p-anthive", &mut next).unwrap(); apply(&mut g, &ch); }
        { let ch = g.relate("st-3", "k-revisar", &mut next).unwrap(); apply(&mut g, &ch); }
        assert_eq!(g.related_to("p-anthive"), vec!["st-3"]);
        g.check().unwrap();
    }

    #[test]
    fn disassemble_dissolves_single_block_and_keeps_links() {
        let mut g = Graph::default();
        let mut next = ids();
        { let ch = g.relate("t-a", "t-b", &mut next).unwrap(); apply(&mut g, &ch); }
        { let ch = g.relate("st-1", "p-x", &mut next).unwrap(); apply(&mut g, &ch); }
        let ch = g.disassemble("t-a").unwrap();
        assert_eq!(ch.dissolved, vec!["st-1"]);
        apply(&mut g, &ch);
        assert!(g.parent("t-b").is_none());
        assert_eq!(g.relations("t-b"), vec![(EdgeType::RelatesProject, "p-x".to_string())]);
        g.check().unwrap();
    }

    #[test]
    fn neighbors_include_siblings_parent_and_relations() {
        let mut g = Graph::default();
        let mut next = ids();
        { let ch = g.relate("t-a", "t-b", &mut next).unwrap(); apply(&mut g, &ch); }
        { let ch = g.relate("t-a", "k-1", &mut next).unwrap(); apply(&mut g, &ch); }
        { let ch = g.relate("t-z", "k-1", &mut next).unwrap(); apply(&mut g, &ch); }
        let one: Vec<String> = g.neighbors("t-a", 1, 12).into_iter().map(|(n, _)| n).collect();
        assert_eq!(one, vec!["k-1", "st-1", "t-b"]);
        let two: Vec<String> = g.neighbors("t-a", 2, 12).into_iter().map(|(n, _)| n).collect();
        assert!(two.contains(&"t-z".to_string()));
        assert_eq!(g.neighbors("t-a", 2, 2).len(), 2);
    }

    #[test]
    fn delete_and_undo_restore_everything() {
        let mut g = Graph::default();
        let mut next = ids();
        { let ch = g.relate("t-a", "t-b", &mut next).unwrap(); apply(&mut g, &ch); }
        { let ch = g.relate("st-1", "t-c", &mut next).unwrap(); apply(&mut g, &ch); }
        { let ch = g.relate("t-a", "p-1", &mut next).unwrap(); apply(&mut g, &ch); }
        let before = g.clone();
        let ch = g.detach_node("t-a");
        apply(&mut g, &ch);
        g.check().unwrap();
        for e in ch.events.iter().rev() {
            g.apply(&e.inverse());
        }
        assert_eq!(g, before);
    }

    #[test]
    fn graph_file_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("graph.jsonl");
        let mut g = Graph::default();
        let mut next = ids();
        let ch = g.relate("t-a", "t-b", &mut next).unwrap();
        g.commit(&path, &ch).unwrap();
        let ch = g.disassemble("t-a").unwrap();
        g.commit(&path, &ch).unwrap();
        assert_eq!(Graph::load(&path).unwrap(), g);
    }
}
