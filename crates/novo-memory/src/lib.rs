//! Caixas de memória (spec 5.4, RF-MEM-01 a 07) e montagem do contexto de uma resposta (RF-MEM-04).

use anyhow::{Context, Result};
use novo_core::config::ContextLimits;
use novo_core::fsx::atomic_write;
use novo_core::text::{approx_tokens, clip, untrusted};
use novo_core::time::{self, Ts};
use novo_core::{ThreadState, Vault, frontmatter};
use novo_graph::{Graph, is_project, is_superthread, is_task, is_thread};
use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BoxHead {
    pub kind: String,
    pub schema: u32,
    pub about: String,
    pub version: u32,
    #[serde(default)]
    pub supersedes: Option<u32>,
    pub updated: Ts,
    /// agent | voce | fallback
    pub edited_by: String,
    #[serde(default)]
    pub entities: Vec<String>,
}

/// Uma caixa: L0 (`abstract_`), L1 (`summary`, `stance`, `questions`), L2 (`key_points`, `quotes`, `entities`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MemoryBox {
    pub head: BoxHead,
    pub abstract_: String,
    pub summary: String,
    pub stance: String,
    pub questions: Vec<String>,
    pub key_points: Vec<String>,
    pub quotes: Vec<String>,
}

/// O que o agente devolve ao reescrever uma caixa (validado por JSON Schema — RF-MEM-02).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct BoxDraft {
    #[serde(rename = "abstract")]
    pub abstract_: String,
    pub summary: String,
    #[serde(default)]
    pub stance: String,
    #[serde(default)]
    pub questions: Vec<String>,
    #[serde(default)]
    pub key_points: Vec<String>,
    #[serde(default)]
    pub quotes: Vec<String>,
    #[serde(default)]
    pub entities: Vec<String>,
}

pub const BOX_SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["abstract","summary","stance","questions","key_points","quotes","entities"],"properties":{"abstract":{"type":"string","description":"uma linha, até 200 caracteres"},"summary":{"type":"string"},"stance":{"type":"string","description":"só o que o usuário escreveu; vazio se ele não opinou"},"questions":{"type":"array","items":{"type":"string"}},"key_points":{"type":"array","items":{"type":"string"}},"quotes":{"type":"array","items":{"type":"string"}},"entities":{"type":"array","items":{"type":"string"}}}}"#;

impl MemoryBox {
    pub fn render(&self) -> Result<String> {
        let mut body = String::new();
        body.push_str(&format!("**Em uma linha:** {}\n\n", self.abstract_.trim()));
        body.push_str(&format!("**Resumo:** {}\n\n", self.summary.trim()));
        body.push_str(&format!("**Sua posição:** {}\n\n", if self.stance.trim().is_empty() { "—" } else { self.stance.trim() }));
        body.push_str("**Perguntas abertas**\n");
        push_list(&mut body, &self.questions);
        body.push_str("\n**Pontos-chave**\n");
        push_list(&mut body, &self.key_points);
        body.push_str("\n**Trechos**\n");
        push_list(&mut body, &self.quotes.iter().map(|q| format!("\"{}\"", q.trim().trim_matches('"'))).collect::<Vec<_>>());
        frontmatter::render(&self.head, &body)
    }

    /// Parser tolerante: aceita a caixa editada à mão no Obsidian.
    pub fn parse(text: &str) -> Result<MemoryBox> {
        let (head, body): (BoxHead, String) = frontmatter::parse(text)?;
        let mut b = MemoryBox { head, abstract_: String::new(), summary: String::new(), stance: String::new(), questions: vec![], key_points: vec![], quotes: vec![] };
        let mut section = "";
        for line in body.lines() {
            let l = line.trim();
            if let Some(rest) = l.strip_prefix("**Em uma linha:**") {
                b.abstract_ = rest.trim().to_string();
                section = "";
            } else if let Some(rest) = l.strip_prefix("**Resumo:**") {
                b.summary = rest.trim().to_string();
                section = "summary";
            } else if let Some(rest) = l.strip_prefix("**Sua posição:**") {
                let r = rest.trim();
                b.stance = if r == "—" { String::new() } else { r.to_string() };
                section = "";
            } else if l.starts_with("**Perguntas abertas**") {
                section = "q";
            } else if l.starts_with("**Pontos-chave**") {
                section = "k";
            } else if l.starts_with("**Trechos**") {
                section = "t";
            } else if let Some(item) = l.strip_prefix("- ") {
                let item = item.trim().to_string();
                if item == "—" {
                    continue;
                }
                match section {
                    "q" => b.questions.push(item),
                    "k" => b.key_points.push(item),
                    "t" => b.quotes.push(item.trim_matches('"').to_string()),
                    _ => {}
                }
            } else if section == "summary" && !l.is_empty() {
                b.summary.push(' ');
                b.summary.push_str(l);
            }
        }
        Ok(b)
    }

    /// Texto L1 para os vizinhos, cortado no teto de tokens (RF-MEM-04).
    pub fn l1(&self, max_tokens: usize) -> String {
        let mut s = format!("Em uma linha: {}\nResumo: {}", self.abstract_, self.summary);
        if !self.stance.is_empty() {
            s.push_str(&format!("\nPosição do usuário: {}", self.stance));
        }
        if !self.questions.is_empty() {
            s.push_str(&format!("\nPerguntas abertas: {}", self.questions.join("; ")));
        }
        let max_chars = max_tokens * 4;
        if approx_tokens(&s) > max_tokens { clip(&s, max_chars) } else { s }
    }

    /// Texto completo (L0 a L2) para a thread atual.
    pub fn full(&self) -> String {
        let mut s = self.l1(usize::MAX / 8);
        if !self.key_points.is_empty() {
            s.push_str(&format!("\nPontos-chave: {}", self.key_points.join("; ")));
        }
        if !self.quotes.is_empty() {
            s.push_str(&format!("\nTrechos: {}", self.quotes.iter().map(|q| format!("\"{q}\"")).collect::<Vec<_>>().join(" ")));
        }
        if !self.head.entities.is_empty() {
            s.push_str(&format!("\nEntidades: {}", self.head.entities.join(", ")));
        }
        s
    }
}

fn push_list(out: &mut String, items: &[String]) {
    if items.is_empty() {
        out.push_str("- —\n");
    }
    for i in items {
        out.push_str(&format!("- {}\n", i.trim()));
    }
}

pub fn load_box(vault: &Vault, id: &str) -> Result<Option<MemoryBox>> {
    match fs::read_to_string(vault.box_path(id)) {
        Ok(s) => Ok(Some(MemoryBox::parse(&s).with_context(|| format!("caixa {id}"))?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Grava uma nova versão da caixa; a anterior vai para `boxes/.versions/<id>/v<N>.md` (RF-MEM-02).
pub fn save_box(vault: &Vault, about: &str, draft: BoxDraft, edited_by: &str) -> Result<MemoryBox> {
    let prev = load_box(vault, about)?;
    let version = prev.as_ref().map(|p| p.head.version + 1).unwrap_or(1);
    if let Some(p) = &prev {
        let dir = vault.box_versions_dir(about);
        fs::create_dir_all(&dir)?;
        atomic_write(&dir.join(format!("v{}.md", p.head.version)), p.render()?.as_bytes())?;
    }
    let b = MemoryBox {
        head: BoxHead {
            kind: "box".into(),
            schema: 1,
            about: about.into(),
            version,
            supersedes: prev.map(|p| p.head.version),
            updated: time::now(),
            edited_by: edited_by.into(),
            entities: draft.entities.into_iter().map(|e| e.trim().to_lowercase()).filter(|e| !e.is_empty()).collect(),
        },
        abstract_: clip(draft.abstract_.trim(), 200),
        summary: draft.summary.trim().to_string(),
        stance: draft.stance.trim().to_string(),
        questions: draft.questions,
        key_points: draft.key_points,
        quotes: draft.quotes,
    };
    atomic_write(&vault.box_path(about), b.render()?.as_bytes())?;
    Ok(b)
}

pub fn box_versions(vault: &Vault, id: &str) -> Vec<u32> {
    let mut v: Vec<u32> = fs::read_dir(vault.box_versions_dir(id))
        .map(|rd| rd.flatten().filter_map(|e| e.file_name().to_str()?.strip_prefix('v')?.strip_suffix(".md")?.parse().ok()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// Caixa sem LLM: usada quando o agente está desligado, para toda thread ter caixa (RF-MEM-01).
pub fn fallback_draft(st: &ThreadState) -> BoxDraft {
    let root = st.root().map(|p| p.body.clone()).unwrap_or_default();
    let mut summary = clip(root.trim(), 600);
    if let Some(m) = st.material.iter().find(|m| m.excerpt.is_some()) {
        summary.push_str(&format!(" Material: {}", clip(m.excerpt.as_deref().unwrap_or(""), 300)));
    }
    BoxDraft {
        abstract_: st.title(),
        summary,
        stance: String::new(),
        questions: vec![],
        key_points: vec![],
        quotes: vec![],
        entities: st.material.iter().filter_map(|m| m.site.clone()).collect(),
    }
}

/// Síntese sem LLM de uma superthread: as linhas L0 dos blocos (RF-MEM-05, modo sem agente).
pub fn fallback_superthread(title: &str, children: &[(String, Option<MemoryBox>)]) -> BoxDraft {
    let lines: Vec<String> = children.iter().map(|(id, b)| format!("{}: {}", id, b.as_ref().map(|b| b.abstract_.clone()).unwrap_or_default())).collect();
    BoxDraft {
        abstract_: title.to_string(),
        summary: lines.join(" | "),
        key_points: children.iter().filter_map(|(_, b)| b.as_ref().map(|b| b.abstract_.clone())).collect(),
        entities: children.iter().flat_map(|(_, b)| b.as_ref().map(|b| b.head.entities.clone()).unwrap_or_default()).collect(),
        ..Default::default()
    }
}

// ------------------------------------------------------------------ contexto

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Neighbor {
    pub id: String,
    pub depth: usize,
    pub kind: String,
    pub text: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AgentContext {
    pub thread: String,
    pub material: String,
    pub own_box: String,
    pub neighbors: Vec<Neighbor>,
}

impl AgentContext {
    /// Ids das caixas usadas (vão para o diário — UC-12).
    pub fn used(&self) -> Vec<String> {
        self.neighbors.iter().map(|n| n.id.clone()).collect()
    }

    pub fn render(&self) -> String {
        let mut s = String::new();
        if !self.neighbors.is_empty() {
            s.push_str("## Caixas de memória relacionadas (só as que o usuário ligou no grafo)\n");
            for n in &self.neighbors {
                let text = untrusted(&format!("memoria:{}", n.id), &n.text);
                s.push_str(&format!("\n### {} ({}, {} salto{})\n{}\n", n.id, n.kind, n.depth, if n.depth > 1 { "s" } else { "" }, text));
            }
            s.push('\n');
        }
        if !self.own_box.is_empty() {
            // a caixa é escrita pelo agente a partir do material e guarda trechos literais dele: também é dado
            s.push_str(&format!("## Caixa desta thread\n{}\n\n", untrusted("memoria:esta-thread", &self.own_box)));
        }
        if !self.material.is_empty() {
            s.push_str(&format!("## Material\n{}\n\n", self.material));
        }
        s.push_str(&format!("## Thread\n{}\n", self.thread));
        s
    }
}

/// Texto da thread, do fim para o começo até o teto, devolvido em ordem cronológica.
pub fn thread_text(st: &ThreadState, max_chars: usize) -> String {
    let mut parts: Vec<String> = vec![];
    let mut used = 0;
    for p in st.posts.iter().rev() {
        let who = if p.is_agent { format!("agente ({})", p.author.trim_start_matches("profile:")) } else { "usuário".to_string() };
        let reply = p.reply_to.as_ref().map(|r| format!(" em resposta a {r}")).unwrap_or_default();
        // o que o agente escreveu pode citar o material literalmente: entra como dado, não como instrução
        let body = if p.is_agent { untrusted(&format!("agente:{}", p.id), p.body.trim()) } else { p.body.trim().to_string() };
        let line = format!("[{}] {}{} — {}:\n{}", p.id, who, reply, p.ts.format("%Y-%m-%d %H:%M"), body);
        used += line.chars().count();
        if used > max_chars && !parts.is_empty() {
            parts.push("[…posts anteriores cortados…]".into());
            break;
        }
        parts.push(line);
    }
    parts.reverse();
    parts.join("\n\n")
}

/// Material extraído (snapshot em markdown), com moldura de dado não confiável (RF-AGT-07).
pub fn material_text(vault: &Vault, st: &ThreadState, max_chars: usize) -> String {
    let dir = vault.thread_dir(&st.id);
    let mut out = vec![];
    for m in &st.material {
        let mut text = String::new();
        if let Some(snap) = &m.snapshot {
            text = fs::read_to_string(dir.join(snap)).unwrap_or_default();
        }
        if text.is_empty() {
            text = m.excerpt.clone().unwrap_or_default();
        }
        let head = format!("Título: {}\nURL: {}\nSite: {}", m.title.clone().unwrap_or_default(), m.url, m.site.clone().unwrap_or_default());
        let body = if text.is_empty() { format!("(conteúdo indisponível: {})", m.error.clone().unwrap_or_else(|| m.status.clone())) } else { clip(&text, max_chars) };
        out.push(untrusted(&m.url, &format!("{head}\n\n{body}")));
    }
    out.join("\n\n")
}

/// Monta o contexto de uma resposta: thread, material, caixa própria e caixas L1 dos vizinhos no grafo.
pub fn assemble(vault: &Vault, graph: &Graph, st: &ThreadState, limits: &ContextLimits, hops: Option<usize>) -> Result<AgentContext> {
    let hops = hops.unwrap_or(limits.neighbor_hops).clamp(1, limits.max_hops.max(1));
    let own = load_box(vault, &st.id)?.map(|b| b.full()).unwrap_or_default();
    let mut neighbors = vec![];
    for (id, depth) in graph.neighbors(&st.id, hops, limits.max_boxes) {
        let (kind, text) = describe(vault, graph, &id, limits.box_tokens)?;
        if text.is_empty() {
            continue;
        }
        neighbors.push(Neighbor { id, depth, kind, text });
    }
    Ok(AgentContext { thread: thread_text(st, limits.thread_chars), material: material_text(vault, st, limits.material_chars), own_box: own, neighbors })
}

fn describe(vault: &Vault, graph: &Graph, id: &str, max_tokens: usize) -> Result<(String, String)> {
    if is_thread(id) || is_superthread(id) {
        let kind = if is_thread(id) { "thread" } else { "superthread" };
        // o que é privado fica na própria thread: nunca entra no contexto de outra (RF-PUB-07)
        let private = |t: &str| vault.load_thread(t).ok().flatten().map(|s| s.private).unwrap_or(false);
        let leaves = if is_thread(id) { vec![id.to_string()] } else { graph.leaf_threads(id) };
        if leaves.iter().any(|t| private(t)) {
            // nem o título: ele pode ter sido escrito a partir da thread privada
            if !is_thread(id) {
                return Ok((kind.into(), format!("Superthread com {} blocos, alguns privados", graph.children(id).len())));
            }
            return Ok((kind.into(), String::new()));
        }
        if let Some(b) = load_box(vault, id)? {
            return Ok((kind.into(), b.l1(max_tokens)));
        }
        if is_thread(id) {
            if let Some(st) = vault.load_thread(id)? {
                return Ok((kind.into(), format!("Em uma linha: {}", st.title())));
            }
        } else if let Some(m) = vault.load_superthread(id)? {
            return Ok((kind.into(), format!("Superthread \"{}\" com {} blocos", m.title, graph.children(id).len())));
        }
        return Ok((kind.into(), String::new()));
    }
    if is_project(id) {
        if let Some(p) = vault.load_project(id)? {
            return Ok(("projeto".into(), clip(&format!("Projeto {}: {}", p.head.name, p.body.trim()), max_tokens * 4)));
        }
        return Ok(("projeto".into(), format!("Projeto {id}")));
    }
    if is_task(id) {
        if let Some(t) = vault.load_task(id)? {
            return Ok((
                "tarefa".into(),
                clip(&format!("Tarefa \"{}\" ({}{}): {}", t.head.title, t.head.status, t.head.due.as_ref().map(|d| format!(", prazo {d}")).unwrap_or_default(), t.description.trim()), max_tokens * 4),
            ));
        }
    }
    Ok(("outro".into(), String::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use novo_core::thread::{AgentMode, Header, ThreadKind};

    fn mk(v: &Vault, body: &str) -> ThreadState {
        let id = novo_core::new_id("t");
        let h = Header { kind: ThreadKind::Reading, agent: AgentMode::Off, profile: None, private: false, source: "app".into(), material: vec![] };
        v.create_thread(&id, h, body, time::now()).unwrap()
    }

    #[test]
    fn box_round_trip_and_versions() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        let d = BoxDraft {
            abstract_: "O artigo defende onboarding em 3 passos".into(),
            summary: "Mostrar valor antes do cadastro.".into(),
            stance: "Cético sobre o passo 2.".into(),
            questions: vec!["Funciona sem login?".into()],
            key_points: vec!["Valor antes do cadastro".into()],
            quotes: vec!["Mostre valor antes".into()],
            entities: vec!["Onboarding".into()],
        };
        let b1 = save_box(&v, "t-x", d.clone(), "agent").unwrap();
        assert_eq!(b1.head.version, 1);
        let b2 = save_box(&v, "t-x", BoxDraft { summary: "Novo resumo".into(), ..d }, "voce").unwrap();
        assert_eq!(b2.head.version, 2);
        assert_eq!(b2.head.supersedes, Some(1));
        let back = load_box(&v, "t-x").unwrap().unwrap();
        assert_eq!(back, b2);
        assert_eq!(box_versions(&v, "t-x"), vec![1]);
        assert_eq!(back.head.entities, vec!["onboarding"]);
    }

    #[test]
    fn context_uses_only_graph_neighbors() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        let a = mk(&v, "Thread A sobre onboarding");
        let b = mk(&v, "Thread B sobre pricing");
        let c = mk(&v, "Thread C, sem relação, também sobre onboarding");
        for st in [&a, &b, &c] {
            save_box(&v, &st.id, fallback_draft(st), "fallback").unwrap();
        }
        let mut g = Graph::default();
        let ch = g.relate(&a.id, &b.id, || "st-1".to_string()).unwrap();
        for e in &ch.events {
            g.apply(e);
        }
        let ctx = assemble(&v, &g, &a, &ContextLimits::default(), None).unwrap();
        let used = ctx.used();
        assert!(used.contains(&b.id));
        assert!(!used.contains(&c.id), "thread sem aresta não entra (RF-MEM-03)");
        assert!(ctx.render().contains("Thread A sobre onboarding"));
    }

    #[test]
    fn neighbor_text_respects_token_cap() {
        let b = MemoryBox {
            head: BoxHead { kind: "box".into(), schema: 1, about: "t".into(), version: 1, supersedes: None, updated: time::now(), edited_by: "agent".into(), entities: vec![] },
            abstract_: "x".into(),
            summary: "palavra ".repeat(1000),
            stance: String::new(),
            questions: vec![],
            key_points: vec![],
            quotes: vec![],
        };
        assert!(approx_tokens(&b.l1(400)) <= 401);
    }
}
