# Anthive Novo — contrato de decisões

Fonte da verdade do produto: a especificação "Anthive Novo — Especificação de Requisitos de Software"
(https://claude.ai/artifact/PwVDs6vW8kqaDVketELx7H). Este arquivo fixa as decisões transversais da
implementação. Todo crate e a interface leem daqui; mudar algo aqui exige mudar o código que depende disso.

## 1. Cofre

- Raiz: `NOVO_HOME` ou `~/.anthive/novo`. Testes sempre usam `NOVO_HOME` temporário.
- Layout (spec 3.5):

```text
config.toml                 preferências, orçamento, prazos de ignorado, porta HTTP
profiles.toml               perfis de agente (chaves só no Keychain)
threads/AAAA/MM/<id>.jsonl  um evento por linha, só acréscimo
threads/AAAA/MM/<id>/       anexos: snapshot.md, snapshot.html, original.pdf
boxes/<id>.md               caixa de memória da thread ou superthread
boxes/.versions/<id>/v<N>.md versões anteriores
superthreads/<id>.json      título e data; composição no grafo
graph.jsonl                 arestas, só acréscimo
chats/<id>.jsonl            conversas laterais
tasks/<id>.md  projects/<id>.md  notes/<id>.md
focus/AAAA-MM-DD.md         relatório da Sessão Foco
trash/<AAAA-MM-DD>/<id>/    excluídos (thread inteira + arestas removidas), 30 dias
prompts/*.md                prompts editáveis (padrões embutidos no binário)
agent/runs/AAAA-MM-DD.jsonl diário dos agentes
access.jsonl                acessos a threads (abrir, responder, Li) — RF-MEM-08
inbox/                      pasta monitorada (RF-CAP/PUB-09)
.index/novo.db              índice SQLite (FTS5 + arestas), reconstruível
.queue/                     fila de extração e de agentes (um JSON por job)
.runtime/                   novod.sock, http.port, token, novod.pid
```

- Escrita: arquivos inteiros por tmp + rename + fsync; JSONL por `O_APPEND` + fsync.
- Um único escritor: o processo `novod` (mutex de escrita no `Service`). CLI e MCP falam com o daemon.

## 2. IDs

- Formato: prefixo + `-` + base32 minúsculo de 10 caracteres (48 bits de tempo em ms + 2 aleatórios).
  - thread `t-…`, superthread `st-…`, tarefa `k-…`, projeto `p-…` (ou `p-<slug>` importado do Anthive:
    `p-anthive-<id do anthive>`), anotação `n-…`, chat `c-…`, evento `e-…`.
- Ordenação lexicográfica = ordem de criação.

## 3. Thread JSONL (spec 3.1, 3.6)

Primeira linha: `{"schema":1,"id":"t-…","type":"thread.created","ts":RFC3339,"kind":"thinking|reading|watching",
"agent":"off|summary|opinion|both","profile":"<nome>|null","private":bool,"source":"app|cli|mcp:<cliente>|extension|inbox|import",
"material":[{"url","canonical_url","title","site","image","lang","snapshot","status":"pending|ok|failed","error"}]}`

Eventos seguintes (`type`):
- `post` — `{id, author:"voce", body, reply_to?}` (o post raiz é o primeiro `post`)
- `agent_reply` — `{id, author:"profile:<nome>", model, mode:"summary|opinion|chat_summary|error", reply_to, body, quotes:[…]}`
- `agent_status` — `{state:"queued|reading|replying|failed|done", detail?}`
- `edit` — `{target:<post id>, body}`
- `read` / `unread` — marca "Li"
- `agent_on` / `agent_off` — `{mode?, profile?}`
- `material` — atualização de material extraído `{index, …campos}`
- `pin` / `unpin`

O estado é derivado dobrando os eventos (`ThreadState`).

## 4. Grafo (spec 3.2)

`graph.jsonl`: `{"ts","op":"add|remove","type":"block_of|relates_project|relates_task","from","to","pos"?}`.
- `block_of`: from = thread ou superthread, to = superthread, `pos` ordem.
- Regras: um pai só; relacionar age topo contra topo; mesma árvore → recusa; superthread com 1 bloco se desfaz
  (remove a aresta restante e marca a superthread como `dissolved` em `superthreads/<id>.json`).
- Nenhuma aresta automática.

## 5. Caixa de memória (spec 5.4)

`boxes/<id>.md` com frontmatter YAML: `kind: box, schema: 1, about, version, supersedes, updated, edited_by: agent|voce,
entities: [...]` e corpo com seções fixas: `**Em uma linha:**` (L0), `**Resumo:**`, `**Sua posição:**`,
`**Perguntas abertas**` (L1), `**Pontos-chave**`, `**Trechos**` (L2). Parser tolerante.
Contexto: thread atual + material + caixa própria + caixas L1 dos vizinhos (1 salto padrão, máx. 2, até 12).

## 6. Perfis e motores (spec 5.5)

`profiles.toml`: `[[profile]] name, engine = "claude_code|openrouter_http|anthropic_api|ollama", model,
auth = "subscription|openrouter|anthropic_api|local", key = "keychain:<serviço>"?, default = bool,
daily_budget_usd?, daily_budget_turns?`.
- Claude Code: `claude -p --output-format json --no-session-persistence --tools "" --safe-mode --model M
  --system-prompt S --json-schema J`. Assinatura: env herdado (ou `CLAUDE_CODE_OAUTH_TOKEN` do Keychain).
  OpenRouter como gateway: `ANTHROPIC_BASE_URL=https://openrouter.ai/api`, `ANTHROPIC_AUTH_TOKEN=<chave>`, `ANTHROPIC_API_KEY=""`.
- OpenRouter direto: `POST https://openrouter.ai/api/v1/chat/completions` (formato OpenAI). Base URL configurável
  (`NOVO_OPENROUTER_BASE`) para testes.
- Keychain: serviço `anthive-novo`, conta = nome da chave.
- Perfil padrão quando não há `profiles.toml`: `claude-assinatura` (claude_code, subscription, model `haiku`).

## 7. API do daemon

JSON-RPC 2.0. Transportes: socket Unix `.runtime/novod.sock` (uma requisição por linha) e HTTP `POST /rpc` em
`127.0.0.1:<porta>` com `Authorization: Bearer <token>`; `GET /events` (SSE) para a UI; `GET /` serve `web/`.
Métodos (nomes estáveis): ver `crates/novo-service/src/rpc.rs` (tabela `METHODS`).

## 8. Tempo

- Fuso de referência fixo GMT-3 (`FixedOffset::west(3h)`). "Dia" das métricas = [00:00, 24:00) GMT-3.
- Sessão Foco: 03:00 UTC; ao iniciar/acordar, roda os dias faltantes (até 7).
- Ignorado: thread com material sem `read` há > 24 h (config `ignored.thread_hours`); tarefa vencida e aberta, ou
  sem prazo e sem alteração há 7 dias (`ignored.task_days`).

## 9. Desvios assumidos em relação à especificação

- RF-MCP-01 pede o SDK `rmcp`; o servidor MCP foi escrito à mão (JSON-RPC sobre stdio, protocolo 2025-06-18),
  como o Anthive faz, para não depender da API instável do SDK. O comportamento externo é o mesmo.
- RF-BUS-02 (embeddings locais, Should): fora desta entrega; a busca é FTS5 + entidades + vizinhos no grafo com RRF.
- Socket: quando o caminho `.runtime/novod.sock` passa de ~100 bytes (limite do `sockaddr_un` no macOS, comum em
  cofres de teste), o socket vai para `/tmp/novo-$USER/<sha256(raiz)[..16]>.sock` (pasta 0700). `Vault::socket_path`.
- Keychain pelo binário `security` do macOS, não pelo crate `keyring`: menos dependências, mesmo armazenamento,
  e o segredo nunca passa por arquivo nem por argumento visível em `ps` (vai pelo stdin de `security -i`).
- Fallback de extração com o Chrome headless via `--dump-dom` (perfil próprio em `.runtime/chrome-profile` ou
  `NOVO_CHROME_PROFILE`), não via CDP: cobre páginas que dependem de JavaScript sem uma dependência de CDP.
- Espaço Desenvolvimento: além de `anthive status --json`, o Novo lê `$ANTHIVE_HOME/projects.json`, porque o
  snapshot do Anthive só lista projetos com agentes vivos; o registro dá os demais.
- O app Tauri carrega a interface do próprio `novod` (URL externa `http://127.0.0.1:<porta>/#token=…`), em vez de
  embutir uma segunda cópia; o `web/` é a mesma interface do navegador.
