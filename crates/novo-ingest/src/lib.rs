//! Extração do material de uma thread (RF-PUB-04, 08; RNF-02): download, conteúdo legível em markdown,
//! metadados, PDF e fallback com Chrome headless. Sem estado: a fila e as tentativas ficam no `novo-service`.

use anyhow::{Context, Result, anyhow, bail};
use dom_smoothie::{Config as RConfig, Readability, TextMode};
use novo_core::config::Ingest;
use novo_core::text::{canonical_url, clip};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Extracted {
    pub url: String,
    pub canonical_url: Option<String>,
    pub title: Option<String>,
    pub site: Option<String>,
    pub author: Option<String>,
    pub published: Option<String>,
    pub image: Option<String>,
    pub lang: Option<String>,
    pub excerpt: Option<String>,
    /// Conteúdo legível em markdown (vai para `snapshot.md`).
    pub markdown: String,
    /// HTML limpo do artigo (vai para `snapshot.html`), quando houver.
    pub html: Option<String>,
    /// html | pdf | text | chrome
    pub via: String,
}

pub fn client(cfg: &Ingest) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(cfg.user_agent.clone())
        .timeout(Duration::from_secs(cfg.timeout_secs))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()?)
}

/// Baixa e extrai. Páginas que voltam quase vazias tentam o Chrome headless quando permitido.
pub async fn fetch(client: &reqwest::Client, url: &str, cfg: &Ingest) -> Result<Extracted> {
    let resp = client.get(url).send().await.with_context(|| format!("baixar {url}"))?;
    let status = resp.status();
    let final_url = resp.url().to_string();
    if !status.is_success() {
        if cfg.chrome_fallback && (status.as_u16() == 403 || status.as_u16() == 401) {
            if let Ok(html) = chrome_dump(url, cfg).await {
                let mut e = extract_html(&html, &final_url)?;
                e.via = "chrome".into();
                return Ok(e);
            }
        }
        bail!("HTTP {} ao baixar {url}", status.as_u16());
    }
    if let Some(len) = resp.content_length() {
        if len > cfg.max_bytes {
            bail!("material grande demais ({len} bytes; limite {})", cfg.max_bytes);
        }
    }
    let ctype = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_lowercase();
    let bytes = resp.bytes().await?;
    if bytes.len() as u64 > cfg.max_bytes {
        bail!("material grande demais ({} bytes)", bytes.len());
    }
    let mut out = if ctype.contains("pdf") || bytes.starts_with(b"%PDF") {
        extract_pdf(&bytes, &final_url)?
    } else if ctype.contains("html") || ctype.is_empty() || looks_html(&bytes) {
        let html = String::from_utf8_lossy(&bytes).to_string();
        let e = extract_html(&html, &final_url)?;
        if e.markdown.trim().chars().count() < 200 && cfg.chrome_fallback {
            // provável página montada por JavaScript
            match chrome_dump(url, cfg).await {
                Ok(html2) => {
                    let mut e2 = extract_html(&html2, &final_url)?;
                    if e2.markdown.len() > e.markdown.len() {
                        e2.via = "chrome".into();
                        e2
                    } else {
                        e
                    }
                }
                Err(_) => e,
            }
        } else {
            e
        }
    } else if ctype.starts_with("text/") {
        let text = String::from_utf8_lossy(&bytes).to_string();
        Extracted { url: final_url.clone(), markdown: text.clone(), excerpt: Some(clip(text.trim(), 280)), via: "text".into(), ..Default::default() }
    } else {
        bail!("tipo de conteúdo não suportado: {ctype}");
    };
    if is_youtube(url) {
        with_transcript(client, url, cfg, &mut out).await;
    }
    out.url = url.to_string();
    out.canonical_url = canonical_url(&final_url).or_else(|| canonical_url(url));
    if out.site.is_none() {
        out.site = host(&final_url);
    }
    if out.lang.is_none() {
        out.lang = detect_lang(&out.markdown);
    }
    Ok(out)
}

/// Links de vídeo do YouTube: a página só traz a descrição; o que interessa é o que foi falado.
pub fn is_youtube(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url) else { return false };
    let host = u.host_str().unwrap_or("").trim_start_matches("www.").trim_start_matches("m.").trim_start_matches("music.");
    match host {
        "youtu.be" => u.path().len() > 1,
        "youtube.com" => u.path() == "/watch" || ["/shorts/", "/live/", "/embed/"].iter().any(|p| u.path().starts_with(p)),
        _ => false,
    }
}

#[derive(Deserialize, Debug, PartialEq)]
pub struct Transcript {
    pub language: Option<String>,
    #[serde(default)]
    pub is_generated: bool,
    #[serde(default)]
    pub words: usize,
    pub text: String,
}

/// Pede a transcrição ao serviço local (`services/yt-transcript`, POST /transcript).
pub async fn youtube_transcript(client: &reqwest::Client, url: &str, cfg: &Ingest) -> Result<Transcript> {
    if cfg.transcript_url.trim().is_empty() {
        bail!("serviço de transcrição desligado (ingest.transcript_url vazio)");
    }
    let body = serde_json::to_vec(&serde_json::json!({ "url": url }))?;
    let resp = client
        .post(cfg.transcript_url.trim())
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .timeout(Duration::from_secs(cfg.transcript_timeout_secs))
        .body(body)
        .send()
        .await
        .with_context(|| format!("serviço de transcrição fora do ar ({})", cfg.transcript_url))?;
    let status = resp.status();
    let bytes = resp.bytes().await?;
    if !status.is_success() {
        let msg = serde_json::from_slice::<serde_json::Value>(&bytes).ok().and_then(|v| v["error"].as_str().map(String::from)).unwrap_or_default();
        bail!("transcrição: HTTP {} {msg}", status.as_u16());
    }
    let t: Transcript = serde_json::from_slice(&bytes).context("resposta do serviço de transcrição ilegível")?;
    if t.text.trim().is_empty() {
        bail!("transcrição vazia");
    }
    Ok(t)
}

/// O serviço de transcrição responde? (`novo doctor`): troca `/transcript` por `/health`.
pub async fn transcript_health(cfg: &Ingest) -> Result<()> {
    let url = cfg.transcript_url.trim();
    if url.is_empty() {
        bail!("desligado (ingest.transcript_url vazio)");
    }
    let health = format!("{}/health", url.trim_end_matches('/').trim_end_matches("/transcript"));
    let resp = client(cfg)?.get(&health).timeout(Duration::from_secs(3)).send().await.with_context(|| format!("fora do ar em {url}"))?;
    if !resp.status().is_success() {
        bail!("HTTP {} em {health}", resp.status().as_u16());
    }
    Ok(())
}

/// Junta descrição e transcrição no material. Sem transcrição, o material diz por quê, para o agente não fingir que viu o vídeo.
async fn with_transcript(client: &reqwest::Client, url: &str, cfg: &Ingest, out: &mut Extracted) {
    // o texto "legível" da página do YouTube é o JavaScript dela; a descrição de verdade está no og:description
    let desc = out.excerpt.clone().unwrap_or_default().trim().to_string();
    match youtube_transcript(client, url, cfg).await {
        Ok(t) => {
            let origin = if t.is_generated { "automática" } else { "feita por pessoa" };
            let lang = t.language.clone().unwrap_or_default();
            out.markdown = format!("## Transcrição ({lang}, legenda {origin})\n\n{}\n\n## Descrição do vídeo\n\n{desc}\n", t.text.trim());
            out.lang = Some(lang.chars().take(2).collect()).filter(|l: &String| !l.is_empty()).or(out.lang.take());
            out.via = "youtube".into();
            out.html = None;
        }
        Err(e) => {
            out.markdown = format!("## Descrição do vídeo\n\n{desc}\n\n(Transcrição indisponível: {e:#}. Só a descrição acima foi lida.)\n");
        }
    }
}

fn looks_html(b: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&b[..b.len().min(512)]).to_lowercase();
    head.contains("<html") || head.contains("<!doctype html") || head.contains("<head")
}

pub fn host(url: &str) -> Option<String> {
    url::Url::parse(url).ok()?.host_str().map(|h| h.trim_start_matches("www.").to_string())
}

pub fn detect_lang(text: &str) -> Option<String> {
    let sample: String = text.chars().take(4000).collect();
    whatlang::detect(&sample).filter(|i| i.is_reliable()).map(|i| i.lang().code().to_string())
}

/// Readability em modo markdown; se falhar, converte o `<body>` inteiro.
pub fn extract_html(html: &str, url: &str) -> Result<Extracted> {
    let og = og_meta(html);
    let cfg = RConfig { text_mode: TextMode::Markdown, ..Default::default() };
    let base = url::Url::parse(url).ok().map(|_| url);
    let parsed = Readability::new(html, base, Some(cfg)).ok().and_then(|mut r| r.parse().ok());
    let mut e = Extracted { url: url.to_string(), via: "html".into(), ..Default::default() };
    match parsed {
        Some(a) => {
            e.title = og.og_title.clone().or_else(|| non_empty(a.title.to_string()).map(|t| clean_title(&t))).or(og.title.clone());
            e.author = a.byline.clone();
            e.excerpt = a.excerpt.clone().or(og.description.clone());
            e.site = a.site_name.clone().or(og.site.clone());
            e.published = a.published_time.clone();
            e.image = a.image.clone().or(og.image.clone());
            e.lang = a.lang.clone().map(|l| l.chars().take(2).collect());
            e.markdown = a.text_content.to_string();
            e.html = Some(a.content.to_string());
        }
        None => {
            e.title = og.title.clone();
            e.excerpt = og.description.clone();
            e.site = og.site.clone();
            e.image = og.image.clone();
            e.markdown = htmd::convert(html).unwrap_or_default();
        }
    }
    if e.excerpt.is_none() && !e.markdown.trim().is_empty() {
        e.excerpt = Some(clip(e.markdown.trim(), 280));
    }
    Ok(e)
}

/// "Título | Site" → "Título", quando a primeira parte é um título de verdade (3+ palavras).
fn clean_title(t: &str) -> String {
    for sep in [" | ", " — ", " – ", " - "] {
        if let Some((first, _)) = t.split_once(sep) {
            if first.split_whitespace().count() >= 3 {
                return first.trim().to_string();
            }
        }
    }
    t.trim().to_string()
}

fn non_empty(s: String) -> Option<String> {
    let t = s.trim().to_string();
    if t.is_empty() { None } else { Some(t) }
}

#[derive(Default)]
struct Og {
    og_title: Option<String>,
    title: Option<String>,
    description: Option<String>,
    site: Option<String>,
    image: Option<String>,
}

fn og_meta(html: &str) -> Og {
    let doc = scraper::Html::parse_document(html);
    let get = |sel: &str| -> Option<String> {
        let s = scraper::Selector::parse(sel).ok()?;
        doc.select(&s).next()?.value().attr("content").map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
    };
    let og_title = get("meta[property='og:title']");
    let title = og_title.clone().or_else(|| {
        let s = scraper::Selector::parse("title").ok()?;
        doc.select(&s).next().map(|t| t.text().collect::<String>().trim().to_string()).filter(|t| !t.is_empty())
    });
    Og {
        og_title,
        title: title.map(|t| clean_title(&t)),
        description: get("meta[property='og:description']").or_else(|| get("meta[name='description']")),
        site: get("meta[property='og:site_name']"),
        image: get("meta[property='og:image']"),
    }
}

pub fn extract_pdf(bytes: &[u8], url: &str) -> Result<Extracted> {
    let text = pdf_extract::extract_text_from_mem(bytes).map_err(|e| anyhow!("PDF ilegível: {e}"))?;
    let title = text.lines().map(str::trim).find(|l| l.chars().count() > 3).map(|l| clip(l, 140));
    Ok(Extracted {
        url: url.to_string(),
        title,
        excerpt: Some(clip(text.trim(), 280)),
        lang: detect_lang(&text),
        markdown: text,
        via: "pdf".into(),
        ..Default::default()
    })
}

/// Extrai um arquivo local (PDF, HTML, MD, TXT ou `.webloc`). Para `.webloc`, devolve a URL a baixar.
pub enum Local {
    Content(Extracted),
    Url(String),
}

pub fn extract_local(path: &Path) -> Result<Local> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let bytes = std::fs::read(path).with_context(|| format!("ler {}", path.display()))?;
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("arquivo").to_string();
    let url = format!("file://{}", path.display());
    match ext.as_str() {
        "pdf" => Ok(Local::Content(Extracted { title: Some(name), ..extract_pdf(&bytes, &url)? })),
        "html" | "htm" => Ok(Local::Content(extract_html(&String::from_utf8_lossy(&bytes), &url)?)),
        "webloc" => {
            let s = String::from_utf8_lossy(&bytes);
            let u = s.split("<string>").nth(1).and_then(|x| x.split("</string>").next()).map(|x| x.trim().to_string());
            u.map(Local::Url).ok_or_else(|| anyhow!(".webloc sem URL"))
        }
        "md" | "markdown" | "txt" | "text" => {
            let text = String::from_utf8_lossy(&bytes).to_string();
            Ok(Local::Content(Extracted {
                url,
                title: Some(name),
                excerpt: Some(clip(text.trim(), 280)),
                lang: detect_lang(&text),
                markdown: text,
                via: "text".into(),
                ..Default::default()
            }))
        }
        _ => bail!("formato não suportado: .{ext}"),
    }
}

pub fn chrome_binary() -> Option<PathBuf> {
    [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/Applications/Arc.app/Contents/MacOS/Arc",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|p| p.exists())
}

/// Renderiza a página no Chrome headless e devolve o DOM final (RF-PUB-08).
/// Usa um perfil próprio do Novo (`NOVO_CHROME_PROFILE` ou `.runtime/chrome-profile`), onde o usuário pode logar uma vez.
pub async fn chrome_dump(url: &str, cfg: &Ingest) -> Result<String> {
    let bin = chrome_binary().ok_or_else(|| anyhow!("Chrome não encontrado"))?;
    let profile = std::env::var("NOVO_CHROME_PROFILE").map(PathBuf::from).unwrap_or_else(|_| novo_core::Vault::default_root().join(".runtime").join("chrome-profile"));
    let out = tokio::time::timeout(
        Duration::from_secs(cfg.timeout_secs + 10),
        tokio::process::Command::new(bin)
            .arg("--headless=new")
            .arg("--disable-gpu")
            .arg("--no-first-run")
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg("--virtual-time-budget=8000")
            .arg("--dump-dom")
            .arg(url)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| anyhow!("Chrome passou do tempo"))??;
    if !out.status.success() || out.stdout.is_empty() {
        bail!("Chrome não renderizou a página");
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<!doctype html><html lang="pt"><head><title>Onboarding em 3 passos | Blog</title>
<meta property="og:site_name" content="Exemplo"><meta property="og:image" content="https://exemplo.com/capa.png">
<meta name="description" content="Como mostrar valor antes do cadastro."></head><body>
<nav>menu menu menu</nav><article><h1>Onboarding em 3 passos</h1>
<p>Mostre valor antes de pedir o cadastro. Esta é a ideia central do artigo, e ela se repete em vários exemplos práticos de produtos.</p>
<p>Primeiro, deixe a pessoa experimentar o produto sem login. Depois, peça só o essencial. Por fim, convide para a equipe.</p>
<p>Empresas que adotaram esse caminho viram mais ativação nas primeiras semanas, segundo os dados apresentados no texto.</p>
</article><footer>rodapé</footer></body></html>"#;

    #[test]
    fn readable_markdown_and_metadata() {
        let e = extract_html(PAGE, "https://exemplo.com/onboarding").unwrap();
        assert_eq!(e.title.as_deref(), Some("Onboarding em 3 passos"));
        assert_eq!(e.site.as_deref(), Some("Exemplo"));
        assert_eq!(e.image.as_deref(), Some("https://exemplo.com/capa.png"));
        assert!(e.markdown.contains("Mostre valor antes de pedir o cadastro"), "{}", e.markdown);
        assert!(!e.markdown.contains("menu menu"), "o menu não é conteúdo: {}", e.markdown);
        assert_eq!(detect_lang(&e.markdown).as_deref(), Some("por"));
    }

    #[test]
    fn local_files() {
        let dir = std::env::temp_dir().join(format!("novo-ingest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let w = dir.join("link.webloc");
        std::fs::write(&w, r#"<?xml version="1.0"?><plist><dict><key>URL</key><string>https://exemplo.com/a</string></dict></plist>"#).unwrap();
        assert!(matches!(extract_local(&w).unwrap(), Local::Url(u) if u == "https://exemplo.com/a"));
        let m = dir.join("nota.md");
        std::fs::write(&m, "# Nota\n\ntexto da nota").unwrap();
        match extract_local(&m).unwrap() {
            Local::Content(e) => assert!(e.markdown.contains("texto da nota")),
            _ => panic!(),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn youtube_links() {
        for u in ["https://www.youtube.com/watch?v=A0kTUP7fWtk", "https://youtu.be/A0kTUP7fWtk", "https://m.youtube.com/watch?v=x", "https://www.youtube.com/shorts/abc"] {
            assert!(is_youtube(u), "{u}");
        }
        for u in ["https://www.youtube.com/@canal", "https://youtu.be/", "https://exemplo.com/watch?v=x", "nada"] {
            assert!(!is_youtube(u), "{u}");
        }
    }

    /// Servidor de uma resposta só: devolve `body` com `ctype` para qualquer pedido.
    async fn serve(status: &'static str, ctype: &'static str, body: String) -> std::net::SocketAddr {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else { break };
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf).await;
                let resp = format!("HTTP/1.1 {status}\r\ncontent-type: {ctype}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
                let _ = s.write_all(resp.as_bytes()).await;
            }
        });
        addr
    }

    #[tokio::test]
    async fn youtube_material_is_the_transcript() {
        let svc = serve("200 OK", "application/json", r#"{"language":"en-US","is_generated":false,"words":4,"text":"we have choices here"}"#.into()).await;
        let cfg = Ingest { chrome_fallback: false, transcript_url: format!("http://{svc}/transcript"), ..Default::default() };
        let c = client(&cfg).unwrap();
        let mut e = extract_html(PAGE, "https://www.youtube.com/watch?v=A0kTUP7fWtk").unwrap();
        with_transcript(&c, "https://www.youtube.com/watch?v=A0kTUP7fWtk", &cfg, &mut e).await;
        assert_eq!(e.via, "youtube");
        assert!(e.markdown.starts_with("## Transcrição (en-US, legenda feita por pessoa)\n\nwe have choices here"), "{}", e.markdown);
        assert!(e.markdown.contains("## Descrição do vídeo\n\nComo mostrar valor antes do cadastro."), "a descrição é a do og: {}", e.markdown);
        assert!(!e.markdown.contains("menu menu"), "nada do resto da página: {}", e.markdown);
        assert_eq!(e.lang.as_deref(), Some("en"));
    }

    #[tokio::test]
    async fn youtube_without_service_says_why() {
        let svc = serve("404 Not Found", "application/json", r#"{"error":"transcrição indisponível: sem legendas"}"#.into()).await;
        let cfg = Ingest { chrome_fallback: false, transcript_url: format!("http://{svc}/transcript"), ..Default::default() };
        let c = client(&cfg).unwrap();
        let mut e = extract_html(PAGE, "https://youtu.be/A0kTUP7fWtk").unwrap();
        with_transcript(&c, "https://youtu.be/A0kTUP7fWtk", &cfg, &mut e).await;
        assert_eq!(e.via, "html");
        assert!(e.markdown.contains("Transcrição indisponível: transcrição: HTTP 404 transcrição indisponível: sem legendas"), "{}", e.markdown);
        let off = Ingest { transcript_url: String::new(), ..cfg };
        assert!(youtube_transcript(&c, "https://youtu.be/A0kTUP7fWtk", &off).await.is_err());
    }

    #[tokio::test]
    async fn fetch_from_local_http_server() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else { break };
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf).await;
                let resp = format!("HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\n\r\n{}", PAGE.len(), PAGE);
                let _ = s.write_all(resp.as_bytes()).await;
            }
        });
        let cfg = Ingest { chrome_fallback: false, ..Default::default() };
        let c = client(&cfg).unwrap();
        let e = fetch(&c, &format!("http://{addr}/onboarding?utm_source=x"), &cfg).await.unwrap();
        assert_eq!(e.title.as_deref(), Some("Onboarding em 3 passos"));
        assert_eq!(e.canonical_url.unwrap(), format!("http://{addr}/onboarding"));
    }
}
