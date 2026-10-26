//! Os quatro motores (spec 5.5): Claude Code (`claude -p`), OpenRouter direto, API da Anthropic e Ollama.

use crate::{Engine, Profile, Request, Response, extract_json, secrets};
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;

pub const OPENROUTER_BASE: &str = "https://openrouter.ai/api/v1";
pub const OPENROUTER_GATEWAY: &str = "https://openrouter.ai/api";

pub async fn run(p: &Profile, req: &Request) -> Result<Response> {
    let started = Instant::now();
    let mut r = match p.engine {
        Engine::ClaudeCode => claude_code(p, req).await,
        Engine::OpenrouterHttp => openrouter(p, req).await,
        Engine::AnthropicApi => anthropic(p, req).await,
        Engine::Ollama => ollama(p, req).await,
    }?;
    r.duration_ms = started.elapsed().as_millis() as u64;
    if req.schema.is_some() && r.json.is_none() {
        r.json = extract_json(&r.text);
        if r.json.is_none() {
            bail!("o modelo não devolveu JSON válido: {}", novo_core::text::clip(&r.text, 300));
        }
    }
    Ok(r)
}

fn key(p: &Profile) -> Result<Option<String>> {
    match &p.key {
        Some(k) => Ok(Some(secrets::resolve(k)?)),
        None => Ok(None),
    }
}

/// Monta o comando do Claude Code. Separado para ser testado sem rodar o binário.
pub fn claude_command(p: &Profile, req: &Request, secret: Option<&str>) -> Result<(String, Vec<String>, Vec<(String, String)>)> {
    let bin = std::env::var("NOVO_CLAUDE_BIN").unwrap_or_else(|_| "claude".into());
    let mut args: Vec<String> = vec![
        "-p".into(),
        "--output-format".into(),
        "json".into(),
        "--no-session-persistence".into(),
        "--safe-mode".into(),
        "--tools".into(),
        "".into(),
        "--model".into(),
        p.model.clone(),
        "--system-prompt".into(),
        req.system.clone(),
    ];
    if let Some(s) = &req.schema {
        args.push("--json-schema".into());
        args.push(s.clone());
    }
    if let Some(cfg) = &req.mcp_config {
        args.push("--mcp-config".into());
        args.push(cfg.clone());
        args.push("--strict-mcp-config".into());
        if !req.allowed_tools.is_empty() {
            args.push("--allowedTools".into());
            args.push(req.allowed_tools.join(","));
        }
        args.push("--permission-prompts".into());
        args.push("none".into());
    }
    let mut env = vec![];
    match p.auth.as_str() {
        "subscription" => {
            if let Some(t) = secret {
                env.push(("CLAUDE_CODE_OAUTH_TOKEN".into(), t.to_string()));
            }
        }
        "openrouter" => {
            let t = secret.ok_or_else(|| anyhow!("perfil {} usa o OpenRouter mas não tem chave", p.name))?;
            env.push(("ANTHROPIC_BASE_URL".into(), p.base_url.clone().unwrap_or_else(|| OPENROUTER_GATEWAY.into())));
            env.push(("ANTHROPIC_AUTH_TOKEN".into(), t.to_string()));
            env.push(("ANTHROPIC_API_KEY".into(), String::new()));
        }
        "anthropic_api" => {
            let t = secret.ok_or_else(|| anyhow!("perfil {} usa a API da Anthropic mas não tem chave", p.name))?;
            env.push(("ANTHROPIC_API_KEY".into(), t.to_string()));
        }
        _ => {}
    }
    Ok((bin, args, env))
}

async fn claude_code(p: &Profile, req: &Request) -> Result<Response> {
    let secret = key(p)?;
    let (bin, args, env) = claude_command(p, req, secret.as_deref())?;
    let mut cmd = tokio::process::Command::new(&bin);
    cmd.args(&args).envs(env).current_dir(std::env::temp_dir()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = cmd.spawn().with_context(|| format!("iniciar {bin}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(req.user.as_bytes()).await?;
        drop(stdin);
    }
    let limit = if req.mcp_config.is_some() { 600 } else { 300 };
    let out = tokio::time::timeout(Duration::from_secs(limit), child.wait_with_output()).await.map_err(|_| anyhow!("claude passou de {limit} s"))??;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: Value = serde_json::from_str(stdout.trim()).map_err(|_| {
        anyhow!("saída inesperada do claude (código {:?}): {} {}", out.status.code(), novo_core::text::clip(&stdout, 300), novo_core::text::clip(&String::from_utf8_lossy(&out.stderr), 300))
    })?;
    parse_claude_json(&v, &p.model)
}

pub fn parse_claude_json(v: &Value, fallback_model: &str) -> Result<Response> {
    let text = v["result"].as_str().unwrap_or_default().to_string();
    if v["is_error"].as_bool().unwrap_or(false) {
        bail!("claude respondeu com erro: {}", novo_core::text::clip(&text, 400));
    }
    let usage = &v["usage"];
    let input = ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"].iter().map(|k| usage[*k].as_u64().unwrap_or(0)).sum();
    let model = v["modelUsage"].as_object().and_then(|m| m.keys().next().cloned()).unwrap_or_else(|| fallback_model.to_string());
    let json = if v["structured_output"].is_object() { Some(v["structured_output"].clone()) } else { None };
    Ok(Response { text, json, model, input_tokens: input, output_tokens: usage["output_tokens"].as_u64().unwrap_or(0), cost_usd: v["total_cost_usd"].as_f64().unwrap_or(0.0), duration_ms: 0 })
}

fn http() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().timeout(Duration::from_secs(180)).build()?)
}

fn json_instruction(req: &Request) -> String {
    match &req.schema {
        Some(s) => format!("{}\n\nResponda SOMENTE com um objeto JSON válido que siga este JSON Schema:\n{}", req.system, s),
        None => req.system.clone(),
    }
}

async fn openrouter(p: &Profile, req: &Request) -> Result<Response> {
    let secret = key(p)?.ok_or_else(|| anyhow!("perfil {} sem chave do OpenRouter", p.name))?;
    let base = p.base_url.clone().or_else(|| std::env::var("NOVO_OPENROUTER_BASE").ok()).unwrap_or_else(|| OPENROUTER_BASE.into());
    let mut body = json!({
        "model": p.model,
        "max_tokens": req.max_tokens,
        "usage": {"include": true},
        "messages": [{"role": "system", "content": json_instruction(req)}, {"role": "user", "content": req.user}],
    });
    if let Some(s) = &req.schema {
        let schema: Value = serde_json::from_str(s)?;
        body["response_format"] = json!({"type": "json_schema", "json_schema": {"name": "output", "strict": true, "schema": schema}});
    }
    let client = http()?;
    let send = |b: &Value| {
        client
            .post(format!("{}/chat/completions", base.trim_end_matches('/')))
            .bearer_auth(&secret)
            .header("HTTP-Referer", "https://github.com/andre2654/anthive")
            .header("X-Title", "Anthive Novo")
            .json(b)
            .send()
    };
    let mut resp = send(&body).await.context("chamar o OpenRouter")?;
    if resp.status().as_u16() == 400 && body.get("response_format").is_some() {
        // alguns modelos não aceitam json_schema: tenta de novo só com a instrução no prompt
        body.as_object_mut().map(|o| o.remove("response_format"));
        resp = send(&body).await.context("chamar o OpenRouter")?;
    }
    let status = resp.status();
    let v: Value = resp.json().await.context("resposta do OpenRouter não é JSON")?;
    if !status.is_success() {
        bail!("OpenRouter HTTP {}: {}", status.as_u16(), v["error"]["message"].as_str().unwrap_or("erro"));
    }
    let text = v["choices"][0]["message"]["content"].as_str().unwrap_or_default().to_string();
    Ok(Response {
        json: if req.schema.is_some() { extract_json(&text) } else { None },
        model: v["model"].as_str().unwrap_or(&p.model).to_string(),
        input_tokens: v["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
        output_tokens: v["usage"]["completion_tokens"].as_u64().unwrap_or(0),
        cost_usd: v["usage"]["cost"].as_f64().unwrap_or(0.0),
        text,
        duration_ms: 0,
    })
}

async fn anthropic(p: &Profile, req: &Request) -> Result<Response> {
    let secret = key(p)?.ok_or_else(|| anyhow!("perfil {} sem chave da API da Anthropic", p.name))?;
    let base = p.base_url.clone().unwrap_or_else(|| "https://api.anthropic.com".into());
    let body = json!({
        "model": p.model,
        "max_tokens": req.max_tokens,
        "system": json_instruction(req),
        "messages": [{"role": "user", "content": req.user}],
    });
    let resp = http()?
        .post(format!("{}/v1/messages", base.trim_end_matches('/')))
        .header("x-api-key", secret)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await
        .context("chamar a API da Anthropic")?;
    let status = resp.status();
    let v: Value = resp.json().await?;
    if !status.is_success() {
        bail!("API da Anthropic HTTP {}: {}", status.as_u16(), v["error"]["message"].as_str().unwrap_or("erro"));
    }
    let text: String = v["content"].as_array().map(|a| a.iter().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("")).unwrap_or_default();
    Ok(Response {
        json: if req.schema.is_some() { extract_json(&text) } else { None },
        model: v["model"].as_str().unwrap_or(&p.model).to_string(),
        input_tokens: v["usage"]["input_tokens"].as_u64().unwrap_or(0),
        output_tokens: v["usage"]["output_tokens"].as_u64().unwrap_or(0),
        cost_usd: 0.0,
        text,
        duration_ms: 0,
    })
}

async fn ollama(p: &Profile, req: &Request) -> Result<Response> {
    let base = p.base_url.clone().unwrap_or_else(|| "http://127.0.0.1:11434".into());
    let mut body = json!({
        "model": p.model,
        "stream": false,
        "messages": [{"role": "system", "content": json_instruction(req)}, {"role": "user", "content": req.user}],
    });
    if let Some(s) = &req.schema {
        body["format"] = serde_json::from_str(s)?;
    }
    let resp = http()?.post(format!("{}/api/chat", base.trim_end_matches('/'))).json(&body).send().await.context("chamar o Ollama")?;
    let status = resp.status();
    let v: Value = resp.json().await?;
    if !status.is_success() {
        bail!("Ollama HTTP {}: {}", status.as_u16(), v["error"].as_str().unwrap_or("erro"));
    }
    let text = v["message"]["content"].as_str().unwrap_or_default().to_string();
    Ok(Response {
        json: if req.schema.is_some() { extract_json(&text) } else { None },
        model: p.model.clone(),
        input_tokens: v["prompt_eval_count"].as_u64().unwrap_or(0),
        output_tokens: v["eval_count"].as_u64().unwrap_or(0),
        cost_usd: 0.0,
        text,
        duration_ms: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompts::REPLY_SCHEMA;

    fn req() -> Request {
        Request::new("reply", "sys".into(), "user".into(), Some(REPLY_SCHEMA))
    }

    #[test]
    fn claude_command_for_subscription_and_openrouter_gateway() {
        let p = Profile::claude_default();
        let (_, args, env) = claude_command(&p, &req(), None).unwrap();
        assert!(args.windows(2).any(|w| w[0] == "--tools" && w[1].is_empty()), "respostas em thread sem ferramentas");
        assert!(args.contains(&"--json-schema".to_string()));
        assert!(env.is_empty(), "assinatura herda o login do claude");
        let mut or = p.clone();
        or.auth = "openrouter".into();
        let (_, _, env) = claude_command(&or, &req(), Some("sk-or")).unwrap();
        assert!(env.contains(&("ANTHROPIC_BASE_URL".into(), "https://openrouter.ai/api".into())));
        assert!(env.contains(&("ANTHROPIC_AUTH_TOKEN".into(), "sk-or".into())));
        assert!(env.contains(&("ANTHROPIC_API_KEY".into(), String::new())));
        assert!(claude_command(&or, &req(), None).is_err());
    }

    #[test]
    fn parses_real_claude_output_shape() {
        let v: Value = serde_json::from_str(r#"{"type":"result","is_error":false,"result":"{\"body\":\"ok\",\"quotes\":[]}","structured_output":{"body":"ok","quotes":[]},"total_cost_usd":0.0007,"usage":{"input_tokens":2,"cache_creation_input_tokens":3766,"cache_read_input_tokens":0,"output_tokens":75},"modelUsage":{"claude-haiku-5-5":{}}}"#).unwrap();
        let r = parse_claude_json(&v, "haiku").unwrap();
        assert_eq!(r.model, "claude-haiku-5-5");
        assert_eq!(r.input_tokens, 3768);
        assert_eq!(r.json.unwrap()["body"], "ok");
    }

    #[tokio::test]
    async fn openrouter_direct_against_local_mock() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = std::sync::Arc::new(tokio::sync::Mutex::new(String::new()));
        let seen2 = seen.clone();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 65536];
            let n = s.read(&mut buf).await.unwrap();
            *seen2.lock().await = String::from_utf8_lossy(&buf[..n]).to_string();
            let body = r#"{"model":"openai/gpt-x","choices":[{"message":{"content":"{\"body\":\"Discordo em parte.\",\"quotes\":[\"trecho\"]}"}}],"usage":{"prompt_tokens":120,"completion_tokens":30,"cost":0.0012}}"#;
            let resp = format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}", body.len(), body);
            s.write_all(resp.as_bytes()).await.unwrap();
        });
        unsafe { std::env::set_var("NOVO_TEST_OR_KEY", "sk-or-test") };
        let p = Profile {
            name: "gpt".into(),
            engine: Engine::OpenrouterHttp,
            model: "openai/gpt-x".into(),
            auth: "openrouter".into(),
            key: Some("env:NOVO_TEST_OR_KEY".into()),
            default: false,
            daily_budget_usd: None,
            daily_budget_turns: None,
            base_url: Some(format!("http://{addr}/api/v1")),
            label: None,
        };
        let r = run(&p, &req()).await.unwrap();
        assert_eq!(r.json.unwrap()["body"], "Discordo em parte.");
        assert_eq!(r.output_tokens, 30);
        assert!((r.cost_usd - 0.0012).abs() < 1e-9);
        let raw = seen.lock().await.clone();
        assert!(raw.contains("POST /api/v1/chat/completions"));
        assert!(raw.to_lowercase().contains("authorization: bearer sk-or-test"));
        assert!(raw.contains("json_schema"));
    }
}
