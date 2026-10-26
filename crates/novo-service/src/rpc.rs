//! Tabela de métodos JSON-RPC do daemon (contrato em docs/DECISIONS.md §7).

use crate::{Service, chat, dev, focus_ops, jobs, superthreads, tasks, threads};
use anyhow::{Context, Result, anyhow, bail};
use novo_core::thread::AgentMode;
use novo_core::time;
use novo_index::Filters;
use novo_memory::BoxDraft;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::sync::Arc;

/// (método, descrição). Usado pela documentação e pelo `novo doctor`.
pub const METHODS: &[(&str, &str)] = &[
    ("status", "versão, contagens, fila, último Foco"),
    ("reindex", "reconstrói o índice a partir do cofre"),
    ("config.get", "lê config.toml"),
    ("config.set", "grava config.toml"),
    ("thread.create", "publica uma thread"),
    ("thread.get", "thread com respostas, caixa e chat"),
    ("feed", "feed com filtros"),
    ("thread.reply", "responde a um post"),
    ("thread.edit", "edita um post seu"),
    ("thread.read", "marca ou desmarca Li"),
    ("thread.agent", "liga ou desliga o agente"),
    ("thread.retry", "tenta de novo depois de falha"),
    ("thread.pin", "fixa no topo"),
    ("thread.paste", "cola o texto de um material que falhou"),
    ("thread.delete", "manda para a lixeira"),
    ("thread.restore", "desfaz a exclusão"),
    ("trash.list", "itens na lixeira"),
    ("relate", "relaciona duas peças (monta superthread ou cria vínculo)"),
    ("unrelate", "desfaz vínculo com projeto ou tarefa"),
    ("pick", "seletor do Relacionar: projetos, tarefas, threads, superthreads"),
    ("superthread.get", "superthread com blocos"),
    ("superthread.list", "superthreads de topo"),
    ("superthread.rename", "renomeia"),
    ("superthread.disassemble", "tira um bloco"),
    ("superthread.reorder", "reordena blocos"),
    ("superthread.read", "Li em todas as threads"),
    ("superthread.converse", "conversa da superthread"),
    ("box.get", "caixa de memória"),
    ("box.save", "edita a caixa à mão"),
    ("box.versions", "versões anteriores"),
    ("task.create", "cria tarefa (aceita texto em linguagem natural)"),
    ("task.update", "edita tarefa"),
    ("task.complete", "conclui ou reabre"),
    ("task.delete", "exclui tarefa"),
    ("task.get", "tarefa com subtarefas, threads e anotações"),
    ("task.list", "visões hoje, next7, inbox, project, all, done"),
    ("project.list", "projetos"),
    ("project.create", "cria projeto"),
    ("project.update", "edita projeto"),
    ("project.get", "projeto com tarefas, threads e anotações"),
    ("note.save", "cria ou edita anotação"),
    ("note.list", "anotações"),
    ("note.delete", "exclui anotação"),
    ("focus.run", "roda a Sessão Foco de um dia"),
    ("focus.get", "relatório de um dia"),
    ("focus.list", "dias com relatório"),
    ("obs.ignored", "o que só foi ignorado, agora"),
    ("obs.series", "série diária e tracks"),
    ("search", "busca híbrida"),
    ("chat.open", "abre o chat sobre uma thread"),
    ("chat.get", "lê um chat"),
    ("chat.send", "manda mensagem no chat"),
    ("chat.apply", "aplica uma proposta do chat"),
    ("chat.publish", "publica o resumo do chat na thread"),
    ("profile.list", "perfis de agente"),
    ("profile.save", "grava perfis"),
    ("profile.set_key", "guarda uma chave no Keychain"),
    ("profile.test", "testa um perfil com uma chamada curta"),
    ("prompts.export", "copia os prompts padrão para prompts/"),
    ("journal", "diário dos agentes de um dia"),
    ("queue.list", "jobs na fila"),
    ("dev.status", "projetos e agentes do Anthive"),
    ("dev.open", "abre um projeto no Anthive"),
    ("dev.note", "thread vira nota do Anthive"),
    ("dev.briefing", "briefing que iria para o agente"),
    ("dev.delegate", "delega tarefa a um agente do Anthive"),
];

fn get<T: DeserializeOwned>(p: &Value, k: &str) -> Result<T> {
    serde_json::from_value(p.get(k).cloned().unwrap_or(Value::Null)).with_context(|| format!("parâmetro `{k}` ausente ou inválido"))
}

fn opt<T: DeserializeOwned>(p: &Value, k: &str) -> Option<T> {
    p.get(k).cloned().filter(|v| !v.is_null()).and_then(|v| serde_json::from_value(v).ok())
}

fn all<T: DeserializeOwned>(p: &Value) -> Result<T> {
    serde_json::from_value(if p.is_null() { json!({}) } else { p.clone() }).context("parâmetros inválidos")
}

fn day_param(p: &Value) -> Result<chrono::NaiveDate> {
    match opt::<String>(p, "day") {
        Some(s) => novo_focus::parse_day(&s).context("dia inválido (use AAAA-MM-DD)"),
        None => Ok(time::day_of(&time::now()) - chrono::Duration::days(1)),
    }
}

/// Parâmetros que nomeiam um item do cofre: viram caminho de arquivo, então só passam ids válidos.
const ID_KEYS: [&str; 9] = ["id", "a", "b", "task", "project", "about", "target", "item", "block"];

pub async fn call(svc: &Arc<Service>, method: &str, p: Value) -> Result<Value> {
    let s: &Service = svc;
    for key in ID_KEYS {
        if let Some(v) = p.get(key).and_then(|v| v.as_str()) {
            if !v.is_empty() && !novo_core::ids::is_valid(v) {
                bail!("{key} inválido: {v}");
            }
        }
    }
    Ok(match method {
        "status" => {
            let idx = s.index.lock().expect("index").count().unwrap_or(0);
            json!({
                "version": env!("CARGO_PKG_VERSION"),
                "vault": s.vault.root,
                "threads": s.vault.thread_ids().len(),
                "tasks": s.vault.tasks().len(),
                "indexed": idx,
                "queue": jobs::pending(s).len(),
                "last_focus": novo_focus::list_days(&s.vault).last(),
                "now": time::now(),
            })
        }
        "reindex" => json!({"indexed": s.reindex()?}),
        "config.get" => serde_json::to_value(s.vault.config())?,
        "config.set" => {
            let c: novo_core::Config = all(&p)?;
            s.vault.save_config(&c)?;
            serde_json::to_value(c)?
        }
        // ---------------------------------------------------------- threads
        "thread.create" => serde_json::to_value(threads::create(s, all(&p)?).await?)?,
        "thread.get" => serde_json::to_value(threads::get(s, &get::<String>(&p, "id")?)?)?,
        "feed" => serde_json::to_value(threads::feed(s, &all(&p)?)?)?,
        "thread.reply" => serde_json::to_value(threads::reply(s, &get::<String>(&p, "id")?, &get::<String>(&p, "body")?, opt(&p, "reply_to")).await?)?,
        "thread.edit" => serde_json::to_value(threads::edit(s, &get::<String>(&p, "id")?, &get::<String>(&p, "post")?, &get::<String>(&p, "body")?).await?)?,
        "thread.read" => serde_json::to_value(threads::mark_read(s, &get::<String>(&p, "id")?, opt(&p, "read").unwrap_or(true)).await?)?,
        "thread.agent" => serde_json::to_value(threads::set_agent(s, &get::<String>(&p, "id")?, opt::<AgentMode>(&p, "mode").unwrap_or(AgentMode::Opinion), opt(&p, "profile")).await?)?,
        "thread.retry" => serde_json::to_value(threads::retry(s, &get::<String>(&p, "id")?).await?)?,
        "thread.pin" => serde_json::to_value(threads::pin(s, &get::<String>(&p, "id")?, opt(&p, "pinned").unwrap_or(true)).await?)?,
        "thread.paste" => serde_json::to_value(threads::paste_material(s, &get::<String>(&p, "id")?, opt(&p, "index").unwrap_or(0), &get::<String>(&p, "text")?).await?)?,
        "thread.delete" => threads::delete(s, &get::<String>(&p, "id")?).await?,
        "thread.restore" => serde_json::to_value(threads::restore(s, &get::<String>(&p, "id")?).await?)?,
        "trash.list" => json!(threads::trash(s)),
        // ---------------------------------------------------------- relacionar e superthreads
        "relate" => {
            let a: String = get(&p, "a")?;
            match (opt::<String>(&p, "b"), opt::<String>(&p, "new_task")) {
                (_, Some(title)) => tasks::create(s, tasks::TaskInput { quick: Some(title), relate_to: Some(a), ..Default::default() }).await?,
                (Some(b), None) => superthreads::relate(s, &a, &b).await?,
                _ => bail!("informe `b` ou `new_task`"),
            }
        }
        "unrelate" => superthreads::unrelate(s, &get::<String>(&p, "a")?, &get::<String>(&p, "b")?).await?,
        "pick" => {
            let q: String = opt(&p, "q").unwrap_or_default();
            let exclude: Option<String> = opt(&p, "exclude");
            let f = Filters { kinds: vec!["project".into(), "task".into(), "thread".into(), "superthread".into()], include_private: true, ..Default::default() };
            let mut hits = s.index.lock().expect("index").search(&q, &f, 40)?;
            if let Some(x) = exclude {
                hits.retain(|h| h.id != x);
            }
            json!(hits)
        }
        "superthread.get" => superthreads::get(s, &get::<String>(&p, "id")?)?,
        "superthread.list" => json!(superthreads::list(s)),
        "superthread.rename" => superthreads::rename(s, &get::<String>(&p, "id")?, &get::<String>(&p, "title")?).await?,
        "superthread.disassemble" => superthreads::disassemble(s, &get::<String>(&p, "block")?).await?,
        "superthread.reorder" => superthreads::reorder(s, &get::<String>(&p, "id")?, &get::<Vec<String>>(&p, "order")?).await?,
        "superthread.read" => superthreads::read_all(s, &get::<String>(&p, "id")?).await?,
        "superthread.converse" => superthreads::converse(svc.clone(), &get::<String>(&p, "id")?, &get::<String>(&p, "body")?, opt(&p, "agent").unwrap_or(true)).await?,
        // ---------------------------------------------------------- caixas
        "box.get" => serde_json::to_value(novo_memory::load_box(&s.vault, &get::<String>(&p, "id")?)?)?,
        "box.save" => {
            let id: String = get(&p, "id")?;
            let d: BoxDraft = get(&p, "box")?;
            let _w = s.write.lock().await;
            let b = novo_memory::save_box(&s.vault, &id, d, "voce")?;
            drop(_w);
            s.reindex_one(&id);
            s.emit("box", &id);
            serde_json::to_value(b)?
        }
        "box.versions" => json!(novo_memory::box_versions(&s.vault, &get::<String>(&p, "id")?)),
        // ---------------------------------------------------------- tasks
        "task.create" => tasks::create(s, all(&p)?).await?,
        "task.update" => tasks::update(s, &get::<String>(&p, "id")?, all(&p)?).await?,
        "task.complete" => tasks::complete(s, &get::<String>(&p, "id")?, opt(&p, "done").unwrap_or(true)).await?,
        "task.delete" => tasks::delete(s, &get::<String>(&p, "id")?).await?,
        "task.get" => tasks::get(s, &get::<String>(&p, "id")?)?,
        "task.list" => json!(tasks::list(s, &opt::<String>(&p, "view").unwrap_or_else(|| "today".into()), opt::<String>(&p, "project").as_deref())?),
        "project.list" => json!(tasks::projects(s)),
        "project.create" => serde_json::to_value(tasks::create_project(s, &get::<String>(&p, "name")?, opt(&p, "color"), opt(&p, "type"), opt(&p, "dir"), None).await?)?,
        "project.update" => tasks::update_project(s, &get::<String>(&p, "id")?, opt(&p, "name"), opt(&p, "color"), opt(&p, "body"), opt(&p, "archived")).await?,
        "project.get" => tasks::project_get(s, &get::<String>(&p, "id")?)?,
        "note.save" => serde_json::to_value(tasks::note_save(s, opt(&p, "id"), opt(&p, "title").unwrap_or_default(), opt(&p, "body").unwrap_or_default(), opt(&p, "about")).await?)?,
        "note.list" => json!(tasks::notes(s, opt::<String>(&p, "about").as_deref())),
        "note.delete" => tasks::note_delete(s, &get::<String>(&p, "id")?).await?,
        // ---------------------------------------------------------- Foco e Observabilidade
        "focus.run" => serde_json::to_value(focus_ops::run_day(s, day_param(&p)?).await?)?,
        "focus.get" => serde_json::to_value(novo_focus::load(&s.vault, day_param(&p)?)?)?,
        "focus.list" => json!(novo_focus::list_days(&s.vault)),
        "obs.ignored" => focus_ops::ignored_now(s, opt(&p, "include_private").unwrap_or(true)),
        "obs.series" => focus_ops::series(s, opt(&p, "days").unwrap_or(60)),
        // ---------------------------------------------------------- busca
        "search" => {
            let q: String = opt(&p, "q").unwrap_or_default();
            let f: Filters = opt(&p, "filters").unwrap_or_default();
            json!(s.index.lock().expect("index").search(&q, &f, opt(&p, "limit").unwrap_or(30))?)
        }
        // ---------------------------------------------------------- chat
        "chat.open" => chat::open(s, &get::<String>(&p, "target")?).await?,
        "chat.get" => chat::get(s, &get::<String>(&p, "id")?)?,
        "chat.send" => chat::send(s, &get::<String>(&p, "id")?, &get::<String>(&p, "text")?).await?,
        "chat.apply" => chat::apply(s, &get::<String>(&p, "id")?, get(&p, "index")?).await?,
        "chat.publish" => chat::publish(s, &get::<String>(&p, "id")?).await?,
        // ---------------------------------------------------------- perfis
        "profile.list" => {
            let ps = novo_agent::load_profiles(&s.vault);
            json!(ps
                .iter()
                .map(|x| {
                    let key_ok = x.key.as_deref().map(|k| novo_agent::secrets::resolve(k).is_ok()).unwrap_or(true);
                    json!({"profile": x, "key_ok": key_ok})
                })
                .collect::<Vec<_>>())
        }
        "profile.save" => {
            let ps: Vec<novo_agent::Profile> = get(&p, "profiles")?;
            novo_agent::save_profiles(&s.vault, &ps)?;
            json!({"ok": true})
        }
        "profile.set_key" => {
            let account: String = get(&p, "account")?;
            let secret: String = get(&p, "secret")?;
            if secret.trim().is_empty() {
                bail!("chave vazia");
            }
            novo_agent::secrets::store(&account, secret.trim())?;
            json!({"ok": true, "key": format!("keychain:{account}")})
        }
        "profile.test" => {
            let prof = novo_agent::find_profile(&s.vault, opt::<String>(&p, "name").as_deref())?;
            let req = novo_agent::Request::new("test", "Responda só com o JSON pedido.".into(), "Devolva {\"body\": \"ok\", \"quotes\": []}".into(), Some(novo_agent::prompts::REPLY_SCHEMA));
            let r = novo_agent::run(&prof, &req).await?;
            json!({"ok": true, "model": r.model, "cost_usd": r.cost_usd, "duration_ms": r.duration_ms})
        }
        "prompts.export" => {
            novo_agent::prompts::export_defaults(&s.vault)?;
            json!({"dir": s.vault.prompts_dir()})
        }
        "journal" => json!(novo_agent::runs_of(&s.vault, opt::<String>(&p, "day").and_then(|d| novo_focus::parse_day(&d)).unwrap_or_else(|| time::day_of(&time::now())))),
        "queue.list" => json!(jobs::pending(s)),
        // ---------------------------------------------------------- Desenvolvimento
        "dev.status" => dev::status(s).await?,
        "dev.open" => dev::open(s, &get::<String>(&p, "dir")?)?,
        "dev.note" => dev::note(s, &get::<String>(&p, "item")?, &get::<String>(&p, "project")?).await?,
        "dev.briefing" => json!({"briefing": dev::briefing(s, &get::<String>(&p, "task")?)?}),
        "dev.delegate" => dev::delegate(s, &get::<String>(&p, "task")?, &opt::<String>(&p, "agent").unwrap_or_default(), opt(&p, "confirm").unwrap_or(false)).await?,
        other => return Err(anyhow!("método desconhecido: {other}")),
    })
}

/// Envelope JSON-RPC 2.0.
pub async fn handle(svc: &Arc<Service>, req: Value) -> Value {
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("").to_string();
    let params = req.get("params").cloned().unwrap_or(Value::Null);
    match call(svc, &method, params).await {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": format!("{e:#}")}}),
    }
}
