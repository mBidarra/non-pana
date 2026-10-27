# Non Panã

A local environment for macOS, written in Rust, with four spaces: **Social** (*Rede Social*: you post what you are thinking, reading or watching, and an agent replies in the thread when you ask), **Tasks** (like Todoist), **Observability** (*Observabilidade*: what you simply ignored) and **Development** (*Desenvolvimento*: Anthive as it is). Every midnight (GMT-3) the **Focus Session** (*Sessão Foco*) reads the day and feeds Observability.

Specification: [Anthive Novo — Especificação de Requisitos de Software](https://claude.ai/artifact/PwVDs6vW8kqaDVketELx7H) (in Portuguese). Implementation decisions: [`docs/DECISIONS.md`](docs/DECISIONS.md).

## How it is organized

| Piece | What it does |
| --- | --- |
| `novod` | Daemon: the vault's only writer, job queue, agents, Focus Session, local HTTP (127.0.0.1 + token) and a Unix socket |
| `novo` | CLI (`post`, `feed`, `foco`, `task add`, `search`, `doctor`, `install-mcp`, `install-daemon`, `open`) and the MCP server (`novo mcp`) |
| `Non Panã.app` | Tauri 2 app: the spaces window, a menu bar icon, and a global ⌘⇧N shortcut to post with the link on the clipboard |
| `web/` | The interface (plain JavaScript, no build step), served by `novod` |
| `crates/novo-*` | Core, graph, memory boxes, SQLite/FTS5 index, extraction, agents, Focus Session, service |

The vault lives in `~/.anthive/novo/` (or `NOVO_HOME`): append-only JSONL threads, memory boxes, tasks and notes in Markdown (they open in Obsidian), `graph.jsonl` with the relations you create, and a SQLite index that can be deleted and rebuilt (`novo reindex`).

## Install from source

Requires Rust (1.85 or newer) and, for the default agent, a logged-in [Claude Code](https://docs.anthropic.com/en/docs/claude-code).

```bash
./install.sh
```

It builds in release mode, copies `novo` and `novod` to `~/.local/bin` and runs `novo doctor`. Then:

```bash
novo install-daemon --load        # novod starts with the Mac (LaunchAgent)
novo install-mcp --claude-desktop # Claude Desktop can see Non Panã
novo open                         # opens the interface in the browser
```

The app (`Non Panã.app`) is bundled with `npx @tauri-apps/cli build --bundles app` inside `crates/novo-app` (see `scripts/bundle-app.sh`).

## YouTube videos

A video page only carries the description. So the agent can read what was said, `novod` asks a local service (`services/yt-transcript`, Python, 127.0.0.1 only) for the transcript and stores the text as the thread's material, not in the chat:

```bash
services/yt-transcript/install.sh   # venv in ~/.local/share/novo-yt-transcript + LaunchAgent com.anthive.novo-yt
curl -s -X POST 127.0.0.1:8790/transcript -d '{"url":"https://youtu.be/<id>"}'
```

It tries `youtube-transcript-api` and falls back to `yt-dlp`; it prefers human-made captions and caches in `~/Library/Caches/novo-yt-transcript`. Without the service, the material says the transcript is missing. The address is in `config.toml` (`[ingest] transcript_url`; empty turns it off). YouTube often blocks cloud IPs, so the service is meant to run on your Mac. When it stops working, run `install.sh` again to update the libraries.

## Agents

By default the agent is Claude Code itself (`claude -p`) on your subscription, with no tools and no saved session, and with context built from the thread's JSONL and the **memory boxes** of the threads you related to it. Other profiles go in `~/.anthive/novo/profiles.toml` or on the Settings screen (*Configurações*):

```toml
[[profile]]
name = "gpt-via-openrouter"
engine = "openrouter_http"     # claude_code | openrouter_http | anthropic_api | ollama
model = "openai/<model>"
auth = "openrouter"
key = "keychain:novo-openrouter"   # the secret lives in the Keychain: novo key novo-openrouter < key.txt
```

Everything an agent does goes to the journal (`agent/runs/YYYY-MM-DD.jsonl`, the Agent Journal screen, *Diário do agente*): profile, model, tokens, cost, boxes used and the size of what was sent.

## Tests

```bash
cargo test --workspace
```

The tests run against temporary vaults and a fake `claude` (`crates/novo-service/tests/fixtures/fake_claude.py`), so they don't spend your subscription.

## Status

Version 0.1. The delivery report, criterion by criterion, is in [`docs/REPORT.md`](docs/REPORT.md) (in Portuguese).
