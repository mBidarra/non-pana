# Anthive Novo

Um ambiente local, em Rust, para macOS, com quatro espaços: **Rede Social** (você publica o que pensa, lê ou vê, e um agente responde em thread quando você pede), **Tasks** (como no Todoist), **Observabilidade** (o que só foi ignorado) e **Desenvolvimento** (o Anthive como está). Toda meia-noite (GMT-3) a **Sessão Foco** lê o dia e alimenta a Observabilidade.

Especificação: [Anthive Novo — Especificação de Requisitos de Software](https://claude.ai/artifact/PwVDs6vW8kqaDVketELx7H). Decisões de implementação: [`docs/DECISIONS.md`](docs/DECISIONS.md).

## Como está organizado

| Peça | O que faz |
| --- | --- |
| `novod` | Daemon: único escritor do cofre, fila de jobs, agentes, Sessão Foco, HTTP local (127.0.0.1 + token) e socket Unix |
| `novo` | CLI (`post`, `feed`, `foco`, `task add`, `search`, `doctor`, `install-mcp`, `install-daemon`, `open`) e o servidor MCP (`novo mcp`) |
| `Novo.app` | App Tauri 2: janela dos espaços, ícone na barra de menus, atalho global ⌘⇧N para publicar com o link do clipboard |
| `web/` | A interface (JavaScript puro, sem build), servida pelo `novod` |
| `crates/novo-*` | Núcleo, grafo, caixas de memória, índice SQLite/FTS5, extração, agentes, Sessão Foco, serviço |

O cofre fica em `~/.anthive/novo/` (ou `NOVO_HOME`): threads em JSONL só com acréscimo, caixas de memória, tarefas e anotações em markdown (abrem no Obsidian), `graph.jsonl` com as relações que você cria e um índice SQLite que pode ser apagado e reconstruído (`novo reindex`).

## Instalar a partir do código

Precisa de Rust (1.85 ou mais novo) e, para o agente padrão, do [Claude Code](https://docs.anthropic.com/en/docs/claude-code) logado.

```bash
./install.sh
```

Ele compila em modo release, copia `novo` e `novod` para `~/.local/bin` e roda `novo doctor`. Depois:

```bash
novo install-daemon --load        # o novod sobe sozinho com o Mac (LaunchAgent)
novo install-mcp --claude-desktop # o Claude Desktop passa a ver o Novo
novo open                         # abre a interface no navegador
```

O app (`Novo.app`) é empacotado com `npx @tauri-apps/cli build --bundles app` dentro de `crates/novo-app` (veja `scripts/bundle-app.sh`).

## Agentes

Por padrão, o agente é o próprio Claude Code (`claude -p`) na sua assinatura, sem ferramentas e sem sessão salva, com o contexto montado a partir do JSONL da thread e das **caixas de memória** das threads que você relacionou. Outros perfis ficam em `~/.anthive/novo/profiles.toml` ou na tela Configurações:

```toml
[[profile]]
name = "gpt-via-openrouter"
engine = "openrouter_http"     # claude_code | openrouter_http | anthropic_api | ollama
model = "openai/<modelo>"
auth = "openrouter"
key = "keychain:novo-openrouter"   # o segredo fica no Keychain: novo key novo-openrouter < chave.txt
```

Tudo que um agente faz fica no diário (`agent/runs/AAAA-MM-DD.jsonl`, tela Diário do agente): perfil, modelo, tokens, custo, caixas usadas e tamanho do que foi enviado.

## Testes

```bash
cargo test --workspace
```

Os testes rodam com cofres temporários e um `claude` falso (`crates/novo-service/tests/fixtures/fake_claude.py`), então não gastam a assinatura.

## Status

Versão 0.1. O relatório da entrega, critério por critério, está em [`docs/REPORT.md`](docs/REPORT.md).
