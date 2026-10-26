//! Tasks (spec 4.5, RF-TSK), projetos e anotações.

use crate::views::{self, ThreadCard};
use crate::Service;
use anyhow::{Context, Result, bail};
use chrono::{Duration, NaiveDate};
use novo_core::model::{Note, NoteHead, Project, ProjectHead, Task, TaskHead};
use novo_core::text::{clip, slug};
use novo_core::time;
use novo_graph::is_superthread;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Deserialize, Default, Debug)]
pub struct TaskInput {
    #[serde(default)]
    pub title: Option<String>,
    /// Texto em linguagem natural (RF-TSK-05).
    #[serde(default)]
    pub quick: Option<String>,
    #[serde(default)]
    pub project: Option<Option<String>>,
    #[serde(default)]
    pub parent: Option<Option<String>>,
    #[serde(default)]
    pub due: Option<Option<String>>,
    #[serde(default)]
    pub priority: Option<u8>,
    #[serde(default)]
    pub labels: Option<Vec<String>>,
    #[serde(default)]
    pub description: Option<String>,
    /// Thread ou superthread a relacionar já na criação (RF-MEN-06 "criar tarefa nova já relacionada").
    #[serde(default)]
    pub relate_to: Option<String>,
    #[serde(default)]
    pub origin: Option<String>,
}

#[derive(Serialize, Debug, Clone)]
pub struct TaskRow {
    #[serde(flatten)]
    pub task: Task,
    pub subtasks_open: usize,
    pub subtasks_total: usize,
    pub threads: usize,
    pub overdue: bool,
    pub ignored: Option<String>,
    pub project_name: Option<String>,
}

fn row(svc: &Service, all: &[Task], t: &Task) -> TaskRow {
    let cfg = svc.vault.config();
    let today = time::day_of(&time::now());
    let subs: Vec<&Task> = all.iter().filter(|x| x.head.parent.as_deref() == Some(&t.head.id)).collect();
    let due = t.head.due.as_deref().and_then(|d| NaiveDate::parse_from_str(&d[..d.len().min(10)], "%Y-%m-%d").ok());
    TaskRow {
        subtasks_open: subs.iter().filter(|s| s.head.status != "done").count(),
        subtasks_total: subs.len(),
        threads: svc.graph().related_to(&t.head.id).len(),
        overdue: t.head.status != "done" && due.map(|d| d < today).unwrap_or(false),
        ignored: novo_focus::task_ignored_at(t, &time::now(), &cfg).map(|(r, _)| r),
        project_name: t.head.project.as_ref().map(|p| views::title_of(svc, p)),
        task: t.clone(),
    }
}

fn due_date(t: &Task) -> Option<NaiveDate> {
    let d = t.head.due.as_deref()?;
    NaiveDate::parse_from_str(&d[..d.len().min(10)], "%Y-%m-%d").ok()
}

/// Visões Hoje, Próximos 7 dias, Entrada, por projeto, todas e concluídas (RF-TSK-04).
pub fn list(svc: &Service, view: &str, project: Option<&str>) -> Result<Vec<TaskRow>> {
    let all = svc.vault.tasks();
    let today = time::day_of(&time::now());
    let open = |t: &&Task| t.head.status != "done";
    let mut v: Vec<&Task> = match view {
        "today" => all.iter().filter(open).filter(|t| due_date(t).map(|d| d <= today).unwrap_or(false)).collect(),
        "next7" => all.iter().filter(open).filter(|t| due_date(t).map(|d| d > today && d <= today + Duration::days(7)).unwrap_or(false)).collect(),
        "inbox" => all.iter().filter(open).filter(|t| t.head.project.is_none() && t.head.parent.is_none()).collect(),
        "project" => {
            let p = project.context("informe o projeto")?;
            all.iter().filter(|t| t.head.project.as_deref() == Some(p)).collect()
        }
        "done" => all.iter().filter(|t| t.head.status == "done").collect(),
        _ => all.iter().filter(open).collect(),
    };
    if view == "done" {
        v.sort_by(|a, b| b.head.completed_at.cmp(&a.head.completed_at));
        v.truncate(200);
    } else {
        v.sort_by(|a, b| {
            let da = due_date(a).unwrap_or(NaiveDate::MAX);
            let db = due_date(b).unwrap_or(NaiveDate::MAX);
            (a.head.status == "done").cmp(&(b.head.status == "done")).then(da.cmp(&db)).then(a.head.priority.cmp(&b.head.priority)).then(a.head.order.cmp(&b.head.order)).then(a.head.created.cmp(&b.head.created))
        });
    }
    Ok(v.into_iter().map(|t| row(svc, &all, t)).collect())
}

pub fn get(svc: &Service, id: &str) -> Result<Value> {
    let t = svc.vault.load_task(id)?.with_context(|| format!("tarefa {id} não existe"))?;
    let all = svc.vault.tasks();
    let g = svc.graph();
    let threads: Vec<Value> = g
        .related_to(id)
        .iter()
        .map(|x| {
            if is_superthread(x) {
                json!({"kind": "superthread", "id": x, "title": views::title_of(svc, x)})
            } else {
                match svc.vault.load_thread(x).ok().flatten() {
                    Some(st) => json!({"kind": "thread", "card": views::card(svc, &g, &st)}),
                    None => json!({"kind": "missing", "id": x}),
                }
            }
        })
        .collect();
    let subtasks: Vec<TaskRow> = all.iter().filter(|x| x.head.parent.as_deref() == Some(id)).map(|x| row(svc, &all, x)).collect();
    let notes: Vec<Note> = svc.vault.notes().into_iter().filter(|n| n.head.about.as_deref() == Some(id)).collect();
    Ok(json!({"task": row(svc, &all, &t), "subtasks": subtasks, "threads": threads, "notes": notes}))
}

pub async fn create(svc: &Service, input: TaskInput) -> Result<Value> {
    let now = time::now();
    let projects: Vec<(String, String)> = svc.vault.projects().into_iter().map(|p| (p.head.id, p.head.name)).collect();
    let parsed = input.quick.as_deref().map(|q| crate::quickadd::parse(q, &projects, time::day_of(&now)));
    let title = input.title.clone().or_else(|| parsed.as_ref().map(|p| p.title.clone())).unwrap_or_default().trim().to_string();
    if title.is_empty() {
        bail!("a tarefa precisa de um título");
    }
    let mut project = input.project.clone().flatten().or_else(|| parsed.as_ref().and_then(|p| p.project.clone()));
    let parent = input.parent.clone().flatten();
    if let Some(par) = &parent {
        let pt = svc.vault.load_task(par)?.context("tarefa-mãe não existe")?;
        if project.is_none() {
            project = pt.head.project.clone();
        }
    }
    if let Some(p) = &project {
        svc.vault.load_project(p)?.with_context(|| format!("projeto {p} não existe"))?;
    }
    let mut labels = input.labels.clone().unwrap_or_default();
    if let Some(p) = &parsed {
        labels.extend(p.labels.clone());
    }
    let t = Task {
        head: TaskHead {
            kind: "task".into(),
            schema: 1,
            id: novo_core::new_id("k"),
            title,
            project,
            parent,
            due: input.due.clone().flatten().or_else(|| parsed.as_ref().and_then(|p| p.due.clone())),
            priority: input.priority.or_else(|| parsed.as_ref().and_then(|p| p.priority)).unwrap_or(4).clamp(1, 4),
            labels,
            status: "open".into(),
            completed_at: None,
            created: now,
            updated: now,
            origin: input.origin.clone().unwrap_or_else(|| if input.relate_to.is_some() { "thread".into() } else { "manual".into() }),
            order: 0,
            delegated_to: None,
        },
        description: input.description.clone().unwrap_or_default(),
    };
    {
        let _w = svc.write.lock().await;
        svc.vault.save_task(&t)?;
    }
    svc.reindex_one(&t.head.id);
    if let Some(src) = &input.relate_to {
        crate::superthreads::relate(svc, src, &t.head.id).await?;
    }
    svc.emit("task", &t.head.id);
    get(svc, &t.head.id)
}

pub async fn update(svc: &Service, id: &str, input: TaskInput) -> Result<Value> {
    let _w = svc.write.lock().await;
    let mut t = svc.vault.load_task(id)?.with_context(|| format!("tarefa {id} não existe"))?;
    if let Some(x) = input.title {
        if !x.trim().is_empty() {
            t.head.title = x.trim().into();
        }
    }
    if let Some(x) = input.project {
        if let Some(p) = &x {
            svc.vault.load_project(p)?.with_context(|| format!("projeto {p} não existe"))?;
        }
        t.head.project = x;
    }
    if let Some(x) = input.parent {
        if x.as_deref() == Some(id) {
            bail!("uma tarefa não pode ser subtarefa de si mesma");
        }
        t.head.parent = x;
    }
    if let Some(x) = input.due {
        t.head.due = x;
    }
    if let Some(x) = input.priority {
        t.head.priority = x.clamp(1, 4);
    }
    if let Some(x) = input.labels {
        t.head.labels = x;
    }
    if let Some(x) = input.description {
        t.description = x;
    }
    t.head.updated = time::now();
    svc.vault.save_task(&t)?;
    drop(_w);
    svc.reindex_one(id);
    svc.emit("task", id);
    get(svc, id)
}

pub async fn complete(svc: &Service, id: &str, done: bool) -> Result<Value> {
    let _w = svc.write.lock().await;
    let mut t = svc.vault.load_task(id)?.with_context(|| format!("tarefa {id} não existe"))?;
    t.head.status = if done { "done".into() } else { "open".into() };
    t.head.completed_at = if done { Some(time::now()) } else { None };
    t.head.updated = time::now();
    svc.vault.save_task(&t)?;
    drop(_w);
    svc.reindex_one(id);
    svc.emit("task", id);
    get(svc, id)
}

pub async fn delete(svc: &Service, id: &str) -> Result<Value> {
    let _w = svc.write.lock().await;
    let ch = svc.graph().detach_node(id);
    svc.commit_graph(&ch)?;
    svc.vault.delete_task(id)?;
    drop(_w);
    svc.reindex_one(id);
    svc.emit("task", id);
    Ok(json!({"deleted": id}))
}

// ------------------------------------------------------------------ projetos

pub fn projects(svc: &Service) -> Vec<Value> {
    let tasks = svc.vault.tasks();
    let g = svc.graph();
    let mut v: Vec<Value> = svc
        .vault
        .projects()
        .into_iter()
        .filter(|p| !p.head.archived)
        .map(|p| {
            let open = tasks.iter().filter(|t| t.head.project.as_deref() == Some(&p.head.id) && t.head.status != "done").count();
            json!({"project": p.head, "open_tasks": open, "threads": g.related_to(&p.head.id).len()})
        })
        .collect();
    v.sort_by(|a, b| a["project"]["name"].as_str().unwrap_or("").to_lowercase().cmp(&b["project"]["name"].as_str().unwrap_or("").to_lowercase()));
    v
}

pub async fn create_project(svc: &Service, name: &str, color: Option<String>, ptype: Option<String>, dir: Option<String>, anthive_id: Option<String>) -> Result<ProjectHead> {
    let name = name.trim();
    if name.is_empty() {
        bail!("o projeto precisa de nome");
    }
    let id = match &anthive_id {
        Some(a) => format!("p-anthive-{}", slug(a, 40)),
        None => novo_core::new_id("p"),
    };
    let _w = svc.write.lock().await;
    if let Some(p) = svc.vault.load_project(&id)? {
        return Ok(p.head);
    }
    let head = ProjectHead {
        kind: "project".into(),
        schema: 1,
        id,
        name: name.into(),
        ptype: ptype.unwrap_or_else(|| "tasks".into()),
        dir,
        anthive_id,
        color,
        created: time::now(),
        archived: false,
    };
    svc.vault.save_project(&Project { head: head.clone(), body: String::new() })?;
    drop(_w);
    svc.reindex_one(&head.id);
    svc.emit("project", &head.id);
    Ok(head)
}

pub async fn update_project(svc: &Service, id: &str, name: Option<String>, color: Option<String>, body: Option<String>, archived: Option<bool>) -> Result<Value> {
    let _w = svc.write.lock().await;
    let mut p = svc.vault.load_project(id)?.with_context(|| format!("projeto {id} não existe"))?;
    if let Some(n) = name {
        p.head.name = n;
    }
    if color.is_some() {
        p.head.color = color;
    }
    if let Some(b) = body {
        p.body = b;
    }
    if let Some(a) = archived {
        p.head.archived = a;
    }
    svc.vault.save_project(&p)?;
    drop(_w);
    svc.reindex_one(id);
    svc.emit("project", id);
    project_get(svc, id)
}

pub fn project_get(svc: &Service, id: &str) -> Result<Value> {
    let p = svc.vault.load_project(id)?.with_context(|| format!("projeto {id} não existe"))?;
    let g = svc.graph();
    let related: Vec<Value> = g
        .related_to(id)
        .iter()
        .map(|x| {
            if is_superthread(x) {
                json!({"kind": "superthread", "id": x, "title": views::title_of(svc, x), "threads": g.leaf_threads(x).len()})
            } else {
                match svc.vault.load_thread(x).ok().flatten() {
                    Some(st) => json!({"kind": "thread", "card": views::card(svc, &g, &st)}),
                    None => json!({"kind": "missing", "id": x}),
                }
            }
        })
        .collect();
    let notes: Vec<Note> = svc.vault.notes().into_iter().filter(|n| n.head.about.as_deref() == Some(id)).collect();
    Ok(json!({"project": p.head, "body": p.body, "tasks": list(svc, "project", Some(id))?, "related": related, "notes": notes}))
}

// ------------------------------------------------------------------ anotações (RF-TSK-06)

pub async fn note_save(svc: &Service, id: Option<String>, title: String, body: String, about: Option<String>) -> Result<Note> {
    let now = time::now();
    let _w = svc.write.lock().await;
    let note = match id.as_deref().map(|i| svc.vault.load_note(i)).transpose()?.flatten() {
        Some(mut n) => {
            n.head.title = title;
            n.body = body;
            if about.is_some() {
                n.head.about = about;
            }
            n.head.updated = now;
            n
        }
        None => Note { head: NoteHead { kind: "note".into(), schema: 1, id: novo_core::new_id("n"), title, about, created: now, updated: now }, body },
    };
    svc.vault.save_note(&note)?;
    drop(_w);
    svc.reindex_one(&note.head.id);
    svc.emit("note", &note.head.id);
    Ok(note)
}

pub fn notes(svc: &Service, about: Option<&str>) -> Vec<Note> {
    let mut v: Vec<Note> = svc.vault.notes().into_iter().filter(|n| about.map(|a| n.head.about.as_deref() == Some(a)).unwrap_or(true)).collect();
    v.sort_by(|a, b| b.head.updated.cmp(&a.head.updated));
    v
}

pub async fn note_delete(svc: &Service, id: &str) -> Result<Value> {
    let _w = svc.write.lock().await;
    let p = svc.vault.note_path(id);
    if p.exists() {
        let dst = svc.vault.trash_dir().join(time::day_of(&time::now()).format("%Y-%m-%d").to_string()).join(id);
        std::fs::create_dir_all(&dst)?;
        std::fs::rename(p, dst.join(format!("{id}.md")))?;
    }
    drop(_w);
    svc.reindex_one(id);
    Ok(json!({"deleted": id}))
}

/// Cartões das threads de uma tarefa ou projeto, para o MCP.
pub fn related_cards(svc: &Service, id: &str) -> Vec<ThreadCard> {
    let g = svc.graph();
    let mut ids = vec![];
    for x in g.related_to(id) {
        if is_superthread(&x) {
            ids.extend(g.leaf_threads(&x));
        } else {
            ids.push(x);
        }
    }
    ids.iter().filter_map(|t| svc.vault.load_thread(t).ok().flatten()).filter(|s| !s.private).map(|s| views::card(svc, &g, &s)).collect()
}

pub fn short(t: &Task) -> String {
    clip(&t.head.title, 80)
}
