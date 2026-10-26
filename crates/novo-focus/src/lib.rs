//! Sessão Foco (spec 4.6, RF-FOC) e números da Observabilidade (spec 4.7, RF-OBS).
//! Tudo aqui é calculado por código e é determinístico (RNF-09); o agente só acrescenta resumo e temas.

use anyhow::Result;
use chrono::{Duration, NaiveDate, NaiveDateTime};
use novo_core::fsx::atomic_write;
use novo_core::model::Task;
use novo_core::text::clip;
use novo_core::thread::EventBody;
use novo_core::time::{self, Ts, day_window};
use novo_core::{Config, ThreadState, Vault, frontmatter};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Counts {
    pub threads_created: usize,
    pub threads_with_material: usize,
    pub user_posts: usize,
    pub agent_replies: usize,
    pub reads: usize,
    pub tasks_created: usize,
    pub tasks_completed: usize,
    pub tasks_open: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct IgnoredThread {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub age_hours: i64,
    #[serde(default)]
    pub site: Option<String>,
    /// Threads privadas não vão a motores remotos nem saem pelo MCP.
    #[serde(default)]
    pub private: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct IgnoredTask {
    pub id: String,
    pub title: String,
    /// overdue | stale
    pub reason: String,
    #[serde(default)]
    pub due: Option<String>,
    pub days: i64,
    #[serde(default)]
    pub project: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct MostRead {
    pub sites: Vec<(String, usize)>,
    pub entities: Vec<(String, usize)>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FocusReport {
    pub kind: String,
    pub schema: u32,
    pub date: NaiveDate,
    pub window_start: Ts,
    pub window_end: Ts,
    pub generated_at: Ts,
    pub counts: Counts,
    pub ignored_threads: Vec<IgnoredThread>,
    pub ignored_tasks: Vec<IgnoredTask>,
    /// % de FOMO nos 7 dias que terminam no dia (RF-OBS-08; fórmula da spec 4.7, hipótese).
    pub fomo_7d: Option<f64>,
    pub most_read: MostRead,
    /// Preenchidos pelo agente da Sessão Foco (RF-FOC-04); nunca alteram os números.
    #[serde(default)]
    pub themes: Vec<String>,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub agent: Option<String>,
}

/// Uma thread está ignorada no instante `at`? (RF-OBS-02)
pub fn thread_ignored_at(st: &ThreadState, at: &Ts, cfg: &Config) -> bool {
    if cfg.ignored.only_with_material && !st.has_material() {
        return false;
    }
    if st.created > *at {
        return false;
    }
    if (*at - st.created) < Duration::hours(cfg.ignored.thread_hours) {
        return false;
    }
    match st.read_at {
        None => true,
        Some(r) => r > *at,
    }
}

fn due_date(t: &Task) -> Option<NaiveDate> {
    let d = t.head.due.as_deref()?;
    NaiveDate::parse_from_str(&d[..d.len().min(10)], "%Y-%m-%d").ok()
}

/// Uma tarefa está ignorada no instante `at`? (RF-OBS-03) Devolve o motivo e os dias.
pub fn task_ignored_at(t: &Task, at: &Ts, cfg: &Config) -> Option<(String, i64)> {
    let open_at = t.head.created <= *at && (t.head.status != "done" || t.head.completed_at.map(|c| c > *at).unwrap_or(false));
    if !open_at {
        return None;
    }
    let today = time::day_of(at);
    if let Some(due) = due_date(t) {
        if due < today {
            return Some(("overdue".into(), (today - due).num_days()));
        }
        return None;
    }
    let stale = (*at - t.head.updated).num_days();
    if t.head.updated <= *at && stale >= cfg.ignored.task_days {
        return Some(("stale".into(), stale));
    }
    None
}

pub fn ignored_threads(threads: &[ThreadState], at: &Ts, cfg: &Config) -> Vec<IgnoredThread> {
    let mut v: Vec<IgnoredThread> = threads
        .iter()
        .filter(|st| thread_ignored_at(st, at, cfg))
        .map(|st| IgnoredThread {
            id: st.id.clone(),
            title: st.title(),
            kind: format!("{:?}", st.kind).to_lowercase(),
            age_hours: (*at - st.created).num_hours(),
            site: st.material.iter().find_map(|m| m.site.clone()),
            private: st.private,
        })
        .collect();
    v.sort_by(|a, b| b.age_hours.cmp(&a.age_hours).then(a.id.cmp(&b.id)));
    v
}

pub fn ignored_tasks(tasks: &[Task], at: &Ts, cfg: &Config) -> Vec<IgnoredTask> {
    let mut v: Vec<IgnoredTask> = tasks
        .iter()
        .filter_map(|t| {
            task_ignored_at(t, at, cfg).map(|(reason, days)| IgnoredTask {
                id: t.head.id.clone(),
                title: t.head.title.clone(),
                reason,
                due: t.head.due.clone(),
                days,
                project: t.head.project.clone(),
            })
        })
        .collect();
    v.sort_by(|a, b| b.days.cmp(&a.days).then(a.id.cmp(&b.id)));
    v
}

/// Calcula o relatório de um dia. Não chama LLM.
pub fn compute(vault: &Vault, day: NaiveDate) -> Result<FocusReport> {
    let cfg = vault.config();
    let (start, end) = day_window(day);
    let threads = vault.all_threads();
    let tasks = vault.tasks();
    let mut counts = Counts::default();
    for st in &threads {
        if st.created >= start && st.created < end {
            counts.threads_created += 1;
            if st.has_material() {
                counts.threads_with_material += 1;
            }
        }
        for ev in vault.load_events(&st.id)? {
            if ev.ts < start || ev.ts >= end {
                continue;
            }
            match ev.body {
                EventBody::Post { .. } => counts.user_posts += 1,
                EventBody::AgentReply { .. } => counts.agent_replies += 1,
                EventBody::Read => counts.reads += 1,
                _ => {}
            }
        }
    }
    for t in &tasks {
        if t.head.created >= start && t.head.created < end {
            counts.tasks_created += 1;
        }
        if let Some(c) = t.head.completed_at {
            if c >= start && c < end {
                counts.tasks_completed += 1;
            }
        }
        let open_at_end = t.head.created < end && (t.head.status != "done" || t.head.completed_at.map(|c| c >= end).unwrap_or(false));
        if open_at_end {
            counts.tasks_open += 1;
        }
    }
    // "No fim do dia" = último instante da janela.
    let at = end - Duration::seconds(1);
    let it = ignored_threads(&threads, &at, &cfg);
    let ik = ignored_tasks(&tasks, &at, &cfg);
    let fomo = fomo_7d(&threads, &tasks, &at, &cfg);
    let most_read = most_read(vault, &threads, &at);
    Ok(FocusReport {
        kind: "focus".into(),
        schema: 1,
        date: day,
        window_start: start,
        window_end: end,
        generated_at: time::now(),
        counts,
        ignored_threads: it,
        ignored_tasks: ik,
        fomo_7d: fomo,
        most_read,
        themes: vec![],
        summary: String::new(),
        agent: None,
    })
}

/// FOMO₇d = 100 × (T_ign + K_ign) / (T_mat + K_ativas), na janela de 7 dias que termina em `at`.
pub fn fomo_7d(threads: &[ThreadState], tasks: &[Task], at: &Ts, cfg: &Config) -> Option<f64> {
    let since = *at - Duration::days(7);
    let t_mat: Vec<&ThreadState> = threads.iter().filter(|st| st.created > since && st.created <= *at && st.has_material()).collect();
    let t_ign = t_mat.iter().filter(|st| thread_ignored_at(st, at, cfg)).count();
    let k_active: Vec<&Task> = tasks
        .iter()
        .filter(|t| t.head.created <= *at && (t.head.status != "done" || t.head.completed_at.map(|c| c > since).unwrap_or(false)))
        .collect();
    let k_ign = k_active.iter().filter(|t| task_ignored_at(t, at, cfg).is_some()).count();
    let den = t_mat.len() + k_active.len();
    if den == 0 {
        return None;
    }
    Some(((t_ign + k_ign) as f64 * 1000.0 / den as f64).round() / 10.0)
}

/// "O que eu tenho mais lido" nos 7 dias: sites e entidades das threads marcadas "Li" (RF-OBS-07).
pub fn most_read(vault: &Vault, threads: &[ThreadState], at: &Ts) -> MostRead {
    let since = *at - Duration::days(7);
    let mut sites: BTreeMap<String, usize> = BTreeMap::new();
    let mut ents: BTreeMap<String, usize> = BTreeMap::new();
    for st in threads {
        let Some(r) = st.read_at else { continue };
        if r <= since || r > *at {
            continue;
        }
        for m in &st.material {
            if let Some(s) = &m.site {
                *sites.entry(s.clone()).or_default() += 1;
            }
        }
        if let Ok(Some(b)) = novo_memory::load_box(vault, &st.id) {
            for e in b.head.entities {
                *ents.entry(e).or_default() += 1;
            }
        }
    }
    let top = |m: BTreeMap<String, usize>| {
        let mut v: Vec<(String, usize)> = m.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v.truncate(10);
        v
    };
    MostRead { sites: top(sites), entities: top(ents) }
}

pub fn render(r: &FocusReport) -> Result<String> {
    let mut body = String::new();
    body.push_str(&format!("# Foco de {}\n\n", r.date.format("%d/%m/%Y")));
    if !r.summary.is_empty() {
        body.push_str(&format!("{}\n\n", r.summary.trim()));
    }
    if !r.themes.is_empty() {
        body.push_str(&format!("**Temas:** {}\n\n", r.themes.join(", ")));
    }
    body.push_str(&format!(
        "Threads novas: {} · respostas suas: {} · respostas do agente: {} · \"Li\": {} · tarefas concluídas: {} · abertas: {}\n\n",
        r.counts.threads_created, r.counts.user_posts, r.counts.agent_replies, r.counts.reads, r.counts.tasks_completed, r.counts.tasks_open
    ));
    body.push_str(&format!("## O que só foi ignorado ({} threads, {} tarefas)\n\n", r.ignored_threads.len(), r.ignored_tasks.len()));
    for t in &r.ignored_threads {
        body.push_str(&format!("- [[{}]] {} — {} h sem \"Li\"\n", t.id, clip(&t.title, 100), t.age_hours));
    }
    for k in &r.ignored_tasks {
        let why = if k.reason == "overdue" { format!("vencida há {} dia(s)", k.days) } else { format!("parada há {} dia(s)", k.days) };
        body.push_str(&format!("- [[{}]] {} — {}\n", k.id, k.title, why));
    }
    frontmatter::render(r, &body)
}

pub fn save(vault: &Vault, r: &FocusReport) -> Result<()> {
    atomic_write(&vault.focus_path(r.date), render(r)?.as_bytes())
}

pub fn load(vault: &Vault, day: NaiveDate) -> Result<Option<FocusReport>> {
    match fs::read_to_string(vault.focus_path(day)) {
        Ok(s) => Ok(Some(frontmatter::parse::<FocusReport>(&s)?.0)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn list_days(vault: &Vault) -> Vec<NaiveDate> {
    let mut v: Vec<NaiveDate> = fs::read_dir(vault.root.join("focus"))
        .map(|rd| rd.flatten().filter_map(|e| NaiveDate::parse_from_str(e.file_name().to_str()?.strip_suffix(".md")?, "%Y-%m-%d").ok()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// Dias que precisam de relatório: de ontem para trás até `catch_up` dias, sem arquivo (RF-FOC-05).
pub fn missing_days(vault: &Vault, today: NaiveDate, catch_up: i64) -> Vec<NaiveDate> {
    let mut out = vec![];
    for back in (1..=catch_up).rev() {
        let d = today - Duration::days(back);
        if !vault.focus_path(d).exists() {
            out.push(d);
        }
    }
    out
}

/// Próximo disparo: 00:00 GMT-3 = 03:00 UTC, independente do fuso do Mac (RNF-08).
pub fn next_run_after(now: &Ts) -> Ts {
    let today = time::day_of(now);
    let (start_tomorrow, _) = day_window(today + Duration::days(1));
    start_tomorrow
}

pub fn parse_day(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok().or_else(|| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok().map(|d| d.date()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::DateTime;
    use novo_core::model::TaskHead;
    use novo_core::thread::{AgentMode, Header, Material, ThreadKind};

    fn ts(s: &str) -> Ts {
        DateTime::parse_from_rfc3339(s).unwrap()
    }

    fn thread(v: &Vault, created: &str, material: bool, kind: ThreadKind) -> ThreadState {
        let id = novo_core::new_id("t");
        let h = Header {
            kind,
            agent: AgentMode::Off,
            profile: None,
            private: false,
            source: "app".into(),
            material: if material { vec![Material { url: "https://a.com/x".into(), site: Some("a.com".into()), ..Default::default() }] } else { vec![] },
        };
        v.create_thread(&id, h, "post", ts(created)).unwrap()
    }

    fn task(v: &Vault, title: &str, due: Option<&str>, created: &str, updated: &str) -> Task {
        let t = Task {
            head: TaskHead {
                kind: "task".into(),
                schema: 1,
                id: novo_core::new_id("k"),
                title: title.into(),
                project: None,
                parent: None,
                due: due.map(String::from),
                priority: 4,
                labels: vec![],
                status: "open".into(),
                completed_at: None,
                created: ts(created),
                updated: ts(updated),
                origin: "manual".into(),
                order: 0,
                delegated_to: None,
            },
            description: String::new(),
        };
        v.save_task(&t).unwrap();
        t
    }

    #[test]
    fn ignored_rules_and_report_are_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        let old = thread(&v, "2026-10-24T10:00:00-03:00", true, ThreadKind::Reading);
        let _fresh = thread(&v, "2026-10-25T20:00:00-03:00", true, ThreadKind::Reading); // menos de 24 h no fim do dia 25
        let _thought = thread(&v, "2026-10-23T10:00:00-03:00", false, ThreadKind::Thinking); // sem material: não conta
        let read = thread(&v, "2026-10-23T09:00:00-03:00", true, ThreadKind::Reading);
        v.append_event(&read.id, EventBody::Read, ts("2026-10-24T12:00:00-03:00")).unwrap();
        task(&v, "vencida", Some("2026-10-24"), "2026-10-20T10:00:00-03:00", "2026-10-20T10:00:00-03:00");
        task(&v, "parada", None, "2026-10-10T10:00:00-03:00", "2026-10-15T10:00:00-03:00");
        task(&v, "em dia", Some("2026-10-30"), "2026-10-20T10:00:00-03:00", "2026-10-20T10:00:00-03:00");
        let day = NaiveDate::from_ymd_opt(2026, 10, 25).unwrap();
        let r1 = compute(&v, day).unwrap();
        assert_eq!(r1.ignored_threads.iter().map(|t| t.id.clone()).collect::<Vec<_>>(), vec![old.id.clone()]);
        assert_eq!(r1.ignored_tasks.iter().map(|t| t.title.clone()).collect::<Vec<_>>(), vec!["parada", "vencida"]);
        assert_eq!(r1.counts.threads_created, 1);
        assert_eq!(r1.counts.tasks_open, 3);
        // RNF-09: recalcular dá os mesmos números
        let r2 = compute(&v, day).unwrap();
        assert_eq!(r1.ignored_threads, r2.ignored_threads);
        assert_eq!(r1.counts, r2.counts);
        assert_eq!(r1.fomo_7d, r2.fomo_7d);
        // gravar e ler de volta
        save(&v, &r1).unwrap();
        let back = load(&v, day).unwrap().unwrap();
        assert_eq!(back.ignored_tasks, r1.ignored_tasks);
        assert_eq!(list_days(&v), vec![day]);
        let s = fs::read_to_string(v.focus_path(day)).unwrap();
        assert!(s.contains("O que só foi ignorado"));
    }

    #[test]
    fn fomo_formula_matches_spec() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        // 2 threads com material (1 ignorada) + 2 tarefas ativas (1 ignorada) → 2/4 = 50%
        let _a = thread(&v, "2026-10-22T10:00:00-03:00", true, ThreadKind::Reading);
        let b = thread(&v, "2026-10-22T11:00:00-03:00", true, ThreadKind::Reading);
        v.append_event(&b.id, EventBody::Read, ts("2026-10-22T12:00:00-03:00")).unwrap();
        task(&v, "vencida", Some("2026-10-23"), "2026-10-21T10:00:00-03:00", "2026-10-21T10:00:00-03:00");
        task(&v, "ok", Some("2026-10-30"), "2026-10-21T10:00:00-03:00", "2026-10-21T10:00:00-03:00");
        let r = compute(&v, NaiveDate::from_ymd_opt(2026, 10, 25).unwrap()).unwrap();
        assert_eq!(r.fomo_7d, Some(50.0));
        assert_eq!(r.most_read.sites, vec![("a.com".to_string(), 1)]);
    }

    #[test]
    fn schedule_is_utc_three_and_catch_up_lists_missing() {
        let now = ts("2026-10-26T22:15:00-03:00");
        assert_eq!(next_run_after(&now).with_timezone(&chrono::Utc).to_rfc3339(), "2026-10-27T03:00:00+00:00");
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 10, 26).unwrap();
        assert_eq!(missing_days(&v, today, 3).len(), 3);
        let r = compute(&v, today - Duration::days(1)).unwrap();
        save(&v, &r).unwrap();
        assert_eq!(missing_days(&v, today, 3), vec![today - Duration::days(3), today - Duration::days(2)]);
    }
}
