//! Transportes e rotinas do daemon: socket Unix, HTTP local com token, eventos (SSE), agendador da
//! Sessão Foco, watcher do cofre, pasta `inbox/`, lixeira e notificações.

use crate::{Service, focus_ops, jobs, rpc, threads};
use anyhow::{Context, Result, bail};
use axum::Router;
use axum::body::Body;
use axum::extract::{Path as AxPath, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures_util::StreamExt;
use novo_core::time;
use rust_embed::RustEmbed;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

#[derive(RustEmbed)]
#[folder = "../../web/"]
struct Web;

#[derive(Clone)]
struct AppState {
    svc: Arc<Service>,
    token: String,
    port: u16,
}

pub fn socket_path(svc: &Service) -> PathBuf {
    svc.vault.socket_path()
}

fn random_hex() -> String {
    let mut out = String::new();
    for i in 0..4u64 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(i ^ std::process::id() as u64);
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

/// Token do HTTP local (RNF-11): gerado uma vez, arquivo com permissão 0600.
pub fn token(svc: &Service) -> Result<String> {
    let p = svc.vault.runtime_dir().join("token");
    if let Ok(t) = std::fs::read_to_string(&p) {
        if t.trim().len() >= 32 {
            return Ok(t.trim().to_string());
        }
    }
    let t = random_hex();
    std::fs::write(&p, &t)?;
    set_mode(&p, 0o600)?;
    Ok(t)
}

fn set_mode(p: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

pub async fn already_running(svc: &Service) -> bool {
    UnixStream::connect(socket_path(svc)).await.is_ok()
}

/// Sobe tudo e fica rodando até SIGINT/SIGTERM.
pub async fn serve(svc: Arc<Service>) -> Result<()> {
    if already_running(&svc).await {
        bail!("o novod já está rodando neste cofre ({})", svc.vault.root.display());
    }
    let tok = token(&svc)?;
    let sock = socket_path(&svc);
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).with_context(|| format!("abrir {}", sock.display()))?;
    set_mode(&sock, 0o600)?;
    std::fs::write(svc.vault.runtime_dir().join("novod.pid"), std::process::id().to_string())?;

    let cfg = svc.vault.config();
    let mut port = cfg.http_port;
    let tcp = loop {
        match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            Ok(l) => break l,
            Err(_) if port != 0 && port < cfg.http_port + 20 => port += 1,
            Err(e) => return Err(e).context("abrir a porta HTTP local"),
        }
    };
    let port = tcp.local_addr()?.port();
    std::fs::write(svc.vault.runtime_dir().join("http.port"), port.to_string())?;
    tracing::info!("novod: http://127.0.0.1:{port}  socket {}", sock.display());

    tokio::spawn(socket_loop(svc.clone(), listener));
    tokio::spawn(jobs::worker(svc.clone(), "io"));
    tokio::spawn(jobs::worker(svc.clone(), "agent"));
    tokio::spawn(scheduler(svc.clone()));
    tokio::spawn(inbox_loop(svc.clone()));
    let _watcher = watch(svc.clone());

    let state = AppState { svc: svc.clone(), token: tok, port };
    let app = router(state);
    let sock_c = sock.clone();
    let pid_c = svc.vault.runtime_dir().join("novod.pid");
    let shutdown = async move {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("sinal");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
        // conexões de eventos (SSE) nunca fecham sozinhas: 2 s de prazo e encerra
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let _ = std::fs::remove_file(&sock_c);
            let _ = std::fs::remove_file(&pid_c);
            std::process::exit(0);
        });
    };
    axum::serve(tcp, app).with_graceful_shutdown(shutdown).await?;
    let _ = std::fs::remove_file(&sock);
    let _ = std::fs::remove_file(svc.vault.runtime_dir().join("novod.pid"));
    Ok(())
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/rpc", post(http_rpc))
        .route("/capture", post(http_capture))
        .route("/events", get(http_events))
        .route("/attachment/{thread}/{*file}", get(http_attachment))
        .route("/", get(index))
        .route("/{*path}", get(asset))
        .with_state(state)
}

/// Bloqueia DNS rebinding: só aceita Host 127.0.0.1/localhost na porta do daemon.
fn host_ok(h: &HeaderMap, port: u16) -> bool {
    let host = h.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("");
    host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}")
}

fn bearer_ok(h: &HeaderMap, token: &str) -> bool {
    h.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).map(|v| v == format!("Bearer {token}")).unwrap_or(false)
}

fn deny(code: StatusCode, msg: &str) -> Response {
    (code, axum::Json(json!({"error": msg}))).into_response()
}

async fn http_rpc(State(st): State<AppState>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    if !host_ok(&headers, st.port) {
        return deny(StatusCode::FORBIDDEN, "host inválido");
    }
    if !bearer_ok(&headers, &st.token) {
        return deny(StatusCode::UNAUTHORIZED, "token ausente ou inválido");
    }
    let req: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return deny(StatusCode::BAD_REQUEST, "JSON inválido"),
    };
    axum::Json(rpc::handle(&st.svc, req).await).into_response()
}

/// Extensão de navegador e Share Extension (RF-PUB-09): `{url?, text?, title?, selection?}`.
async fn http_capture(State(st): State<AppState>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    if !host_ok(&headers, st.port) || !bearer_ok(&headers, &st.token) {
        return deny(StatusCode::UNAUTHORIZED, "token ausente ou inválido");
    }
    let v: Value = serde_json::from_slice(&body).unwrap_or_default();
    let mut text = v["text"].as_str().unwrap_or("").to_string();
    if let Some(sel) = v["selection"].as_str().filter(|s| !s.trim().is_empty()) {
        text.push_str(&format!("\n\n> {}", sel.trim().replace('\n', "\n> ")));
    }
    let links: Vec<String> = v["url"].as_str().map(|u| vec![u.to_string()]).unwrap_or_default();
    let p = threads::CreateParams { text, links, source: Some(v["source"].as_str().filter(|s| s.starts_with("extension")).unwrap_or("extension").into()), ..Default::default() };
    match threads::create(&st.svc, p).await {
        Ok(t) => axum::Json(json!({"id": t.card.id})).into_response(),
        Err(e) => deny(StatusCode::BAD_REQUEST, &format!("{e:#}")),
    }
}

async fn http_events(State(st): State<AppState>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    if !host_ok(&headers, st.port) || q.get("token").map(|t| t != &st.token).unwrap_or(true) {
        return deny(StatusCode::UNAUTHORIZED, "token ausente ou inválido");
    }
    let rx = st.svc.events.subscribe();
    let stream = tokio_stream::wrappers::BroadcastStream::new(rx).filter_map(|m| async move { m.ok().map(|v| Ok::<Event, std::convert::Infallible>(Event::default().data(v.to_string()))) });
    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

async fn http_attachment(State(st): State<AppState>, headers: HeaderMap, AxPath((thread, file)): AxPath<(String, String)>, Query(q): Query<HashMap<String, String>>) -> Response {
    if !host_ok(&headers, st.port) || q.get("token").map(|t| t != &st.token).unwrap_or(true) {
        return deny(StatusCode::UNAUTHORIZED, "token ausente ou inválido");
    }
    // `file` chega decodificado: `%2F…` viraria caminho absoluto e o `join` trocaria a base
    let normal = std::path::Path::new(&file).components().all(|c| matches!(c, std::path::Component::Normal(_)));
    if !thread.starts_with("t-") || !novo_core::ids::is_valid(&thread) || file.is_empty() || !normal {
        return deny(StatusCode::BAD_REQUEST, "caminho inválido");
    }
    let path = st.svc.vault.thread_dir(&thread).join(&file);
    match tokio::fs::read(&path).await {
        Ok(bytes) => {
            let mime = mime_guess::from_path(&path).first_or_octet_stream();
            let ct = if mime.type_() == "text" { format!("{mime}; charset=utf-8") } else { mime.to_string() };
            // o snapshot é conteúdo de terceiros: sem scripts
            ([(header::CONTENT_TYPE, ct), (header::CONTENT_SECURITY_POLICY, "default-src 'none'; img-src * data:; style-src 'unsafe-inline'".to_string())], bytes).into_response()
        }
        Err(_) => deny(StatusCode::NOT_FOUND, "arquivo não encontrado"),
    }
}

async fn index(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if !host_ok(&headers, st.port) {
        return deny(StatusCode::FORBIDDEN, "host inválido");
    }
    serve_asset("index.html")
}

async fn asset(State(st): State<AppState>, headers: HeaderMap, AxPath(path): AxPath<String>) -> Response {
    if !host_ok(&headers, st.port) {
        return deny(StatusCode::FORBIDDEN, "host inválido");
    }
    serve_asset(&path)
}

fn serve_asset(path: &str) -> Response {
    match Web::get(path) {
        Some(f) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            Response::builder()
                .header(header::CONTENT_TYPE, mime.as_ref())
                .header(header::CACHE_CONTROL, "no-cache")
                .body(Body::from(f.data.into_owned()))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn socket_loop(svc: Arc<Service>, listener: UnixListener) {
    loop {
        let Ok((stream, _)) = listener.accept().await else { continue };
        let svc = svc.clone();
        tokio::spawn(async move {
            let (r, mut w) = stream.into_split();
            let mut lines = BufReader::new(r).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                let resp = match serde_json::from_str::<Value>(&line) {
                    Ok(req) => rpc::handle(&svc, req).await,
                    Err(_) => json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "JSON inválido"}}),
                };
                if w.write_all(format!("{resp}\n").as_bytes()).await.is_err() {
                    break;
                }
            }
        });
    }
}

// ------------------------------------------------------------------ rotinas

/// A cada minuto: Sessão Foco dos dias faltantes (00:00 GMT-3 e recuperação ao acordar — RF-FOC-01, 05),
/// lixeira de 30 dias, avisos de prazo.
async fn scheduler(svc: Arc<Service>) {
    let mut last_purge = None;
    loop {
        match focus_ops::catch_up(&svc).await {
            Ok(days) if !days.is_empty() => {
                tracing::info!("Sessão Foco: {:?}", days);
                let cfg = svc.vault.config();
                if cfg.notifications.focus_morning {
                    if let Some(d) = days.last() {
                        if let Ok(Some(r)) = novo_focus::load(&svc.vault, *d) {
                            notify("Sessão Foco", &format!("{} threads e {} tarefas ignoradas", r.ignored_threads.len(), r.ignored_tasks.len()));
                        }
                    }
                }
            }
            Err(e) => tracing::warn!("Sessão Foco: {e:#}"),
            _ => {}
        }
        let today = time::day_of(&time::now());
        if last_purge != Some(today) {
            let days = svc.vault.config().trash_days;
            match svc.vault.purge_trash(today, days) {
                Ok(gone) if !gone.is_empty() => tracing::info!("lixeira: apagados {gone:?}"),
                _ => {}
            }
            last_purge = Some(today);
        }
        due_notifications(&svc);
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

pub fn notify(title: &str, text: &str) {
    if std::env::var("NOVO_NO_NOTIFY").is_ok() {
        return;
    }
    let esc = |s: &str| s.replace('\\', "").replace('"', "'");
    let _ = std::process::Command::new("osascript").arg("-e").arg(format!("display notification \"{}\" with title \"Anthive Novo\" subtitle \"{}\"", esc(text), esc(title))).spawn();
}

/// RF-TSK-11: notificação no prazo (data com hora: na hora; só data: às 09:00 GMT-3).
fn due_notifications(svc: &Service) {
    if !svc.vault.config().notifications.task_due {
        return;
    }
    let path = svc.vault.runtime_dir().join("notified.json");
    let mut done: HashSet<String> = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let now = time::now();
    let mut changed = false;
    for t in svc.vault.tasks() {
        if t.head.status == "done" || done.contains(&t.head.id) {
            continue;
        }
        let Some(due) = t.head.due.as_deref() else { continue };
        let at = if due.len() > 10 {
            chrono::DateTime::parse_from_rfc3339(due).ok()
        } else {
            chrono::NaiveDate::parse_from_str(due, "%Y-%m-%d").ok().map(|d| time::day_window(d).0 + chrono::Duration::hours(9))
        };
        if let Some(at) = at {
            if at <= now && now - at < chrono::Duration::hours(12) {
                notify("Prazo", &t.head.title);
                done.insert(t.head.id.clone());
                changed = true;
            }
        }
    }
    if changed {
        let _ = std::fs::write(&path, serde_json::to_string(&done).unwrap_or_default());
    }
}

/// Pasta monitorada `inbox/`: arquivo solto vira thread (lendo) com o arquivo como material.
async fn inbox_loop(svc: Arc<Service>) {
    loop {
        if let Ok(entries) = std::fs::read_dir(svc.vault.inbox_dir()) {
            for e in entries.flatten() {
                let p = e.path();
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
                if name.starts_with('.') || !p.is_file() || name.ends_with(".tmp") || name.ends_with(".crdownload") {
                    continue;
                }
                let fresh = e.metadata().and_then(|m| m.modified()).map(|m| m.elapsed().map(|d| d < Duration::from_secs(2)).unwrap_or(true)).unwrap_or(true);
                if fresh {
                    continue;
                }
                let res = if name.ends_with(".webloc") {
                    match novo_ingest::extract_local(&p) {
                        Ok(novo_ingest::Local::Url(u)) => threads::create(&svc, threads::CreateParams { links: vec![u], source: Some("inbox".into()), ..Default::default() }).await,
                        _ => Err(anyhow::anyhow!(".webloc sem URL")),
                    }
                } else {
                    threads::create(&svc, threads::CreateParams { files: vec![p.display().to_string()], source: Some("inbox".into()), kind: Some(novo_core::ThreadKind::Reading), ..Default::default() }).await
                };
                match res {
                    Ok(_) => {
                        let _ = std::fs::remove_file(&p);
                    }
                    Err(err) => {
                        tracing::warn!("inbox {name}: {err:#}");
                        let failed = svc.vault.inbox_dir().join(".falhou");
                        let _ = std::fs::create_dir_all(&failed);
                        let _ = std::fs::rename(&p, failed.join(&name));
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

/// Edições feitas fora do app (Obsidian, $EDITOR) são reindexadas (RNF-16).
fn watch(svc: Arc<Service>) -> Option<notify::RecommendedWatcher> {
    use notify::{RecursiveMode, Watcher};
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<PathBuf>();
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            for p in ev.paths {
                let _ = tx.send(p);
            }
        }
    })
    .ok()?;
    for d in ["boxes", "tasks", "notes", "projects", "threads"] {
        let _ = w.watch(&svc.vault.root.join(d), RecursiveMode::Recursive);
    }
    let _ = w.watch(&svc.vault.root, RecursiveMode::NonRecursive);
    tokio::spawn(async move {
        loop {
            let Some(first) = rx.recv().await else { break };
            let mut batch = HashSet::from([first]);
            tokio::time::sleep(Duration::from_millis(400)).await;
            while let Ok(p) = rx.try_recv() {
                batch.insert(p);
            }
            let mut graph_changed = false;
            for p in batch {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.starts_with('.') {
                    continue;
                }
                if name == "graph.jsonl" {
                    graph_changed = true;
                    continue;
                }
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    if ["t-", "st-", "k-", "n-", "p-"].iter().any(|pre| stem.starts_with(pre)) && (name.ends_with(".md") || name.ends_with(".jsonl")) {
                        svc.reindex_one(stem);
                        svc.emit("external", stem);
                    }
                }
            }
            if graph_changed {
                // sob a trava de escrita: o daemon nunca está no meio de um commit, e as próprias escritas dele
                // já estão na memória (só uma edição externa muda algo)
                let _w = svc.write.lock().await;
                if let Ok(g) = novo_graph::Graph::load(&svc.vault.graph_path()) {
                    if g != svc.graph() {
                        *svc.graph.write().expect("graph") = g.clone();
                        let _ = svc.index.lock().expect("index").sync_graph(&g);
                        svc.emit("graph", "external");
                    }
                }
            }
        }
    });
    Some(w)
}
