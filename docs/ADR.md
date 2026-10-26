# Registros de decisão de arquitetura

Um ADR por linha da seção 8.3 da especificação, mais um para o desvio do servidor MCP. Formato curto:
contexto, decisão, alternativa descartada, consequências e o estado na versão 0.1. Os detalhes de formato
(layout do cofre, eventos, arestas) ficam em [`DECISIONS.md`](DECISIONS.md).

## ADR-001 Interface: Tauri 2

- **Contexto:** feed, leitor e editor de markdown precisam de uma interface rica; a lógica deve ficar em Rust.
- **Decisão:** app Tauri 2 (`crates/novo-app`) com a interface em webview. A janela carrega a interface servida pelo
  próprio `novod`, a mesma que abre no navegador com `novo open`.
- **Alternativa:** GPUI, egui ou iced (Rust puro); TUI dentro do Anthive.
- **Consequências:** a interface é JavaScript puro em módulos ES (`web/`), sem TypeScript e sem etapa de build,
  diferente do custo previsto na spec ("um frontend em TypeScript"). Uma só cópia da interface para app e navegador.
- **Estado:** implementado. O `.app` sai de `scripts/bundle-app.sh` com `novod` e `novo` dentro.

## ADR-002 Armazenamento

- **Contexto:** threads recebem eventos de vários lugares (você, agentes, extração); o que você lê precisa ser legível.
- **Decisão:** threads e grafo em JSONL só com acréscimo (fsync por linha); caixas, tarefas, projetos e anotações em
  markdown com frontmatter; índice SQLite/FTS5 derivado em `.index/`, apagável.
- **Alternativa:** tudo em markdown; tudo em SQLite.
- **Consequências:** `novo reindex` reconstrói o índice a partir dos arquivos; uma linha cortada no fim de um JSONL é
  ignorada na leitura.
- **Estado:** implementado; teste `search_and_reindex`.

## ADR-003 Processos

- **Contexto:** app, CLI, MCP, agentes e Sessão Foco escrevem no mesmo cofre.
- **Decisão:** o daemon `novod` é o único escritor (mutex de escrita no serviço) e dono da fila de jobs; app, CLI e
  MCP falam com ele por socket Unix (0600) ou HTTP em 127.0.0.1 com token.
- **Alternativa:** cada processo escreve no cofre.
- **Consequências:** agentes e Foco rodam com o app fechado (LaunchAgent opcional via `novo install-daemon`). Sem o
  daemon, a CLI o inicia; o MCP cai num modo só de leitura.
- **Estado:** implementado.

## ADR-004 Perfis de agente

- **Contexto:** a assinatura do Claude não tem custo extra; o OpenRouter abre outros modelos.
- **Decisão:** quatro motores: `claude_code` (assinatura, ou OpenRouter como gateway via `ANTHROPIC_BASE_URL`),
  `openrouter_http` (HTTP direto, formato OpenAI), `anthropic_api` e `ollama`. Perfil padrão: `claude-assinatura`
  com `haiku`. Chaves só no Keychain.
- **Alternativa:** um provedor só.
- **Consequências:** respostas em thread não usam ferramentas, então HTTP direto basta para os motores não-Claude.
- **Estado:** implementado. Claude Code verificado contra o `claude` real; OpenRouter direto verificado contra um
  servidor local no formato do OpenRouter, não contra o serviço real (sem chave).

## ADR-005 Memória das threads

- **Contexto:** mandar conversas inteiras das threads relacionadas estoura o contexto e o consumo.
- **Decisão:** uma caixa por thread e por superthread (L0 em uma linha, L1 resumo/posição/perguntas, L2 pontos e
  trechos), versionada em `boxes/.versions/`, reescrita pelo agente depois de cada turno. O contexto de uma resposta
  junta a thread, o material, a própria caixa e as caixas L1 dos vizinhos no grafo (1 salto, até 12, 400 tokens cada).
- **Alternativa:** conversa inteira das relacionadas; busca por similaridade.
- **Consequências:** o contexto cresce com as relações que você cria, não com o histórico. Caixas e textos do agente
  entram no prompt com a mesma moldura de dado não confiável do material.
- **Estado:** implementado; testes `uc12_context_uses_only_related_boxes` e `context_respects_box_caps_in_journal`.

## ADR-006 Thread e sessão do Claude Code

- **Contexto:** o Anthive mantém uma sessão do Claude Code por agente.
- **Decisão:** cada resposta é uma chamada sem estado (`claude -p --no-session-persistence --tools ""`), com o contexto
  montado pelo Novo.
- **Alternativa:** uma sessão do Claude Code por thread.
- **Consequências:** funciona com qualquer motor, não depende do formato dos transcripts e permite montar superthreads.
- **Estado:** implementado.

## ADR-007 Embeddings

- **Contexto:** busca semântica ajudaria a achar threads parecidas.
- **Decisão:** embeddings locais com `fastembed` (privacidade, offline, custo zero).
- **Alternativa:** API de embeddings.
- **Consequências:** nenhuma ainda.
- **Estado:** **adiado**. RF-BUS-02 é Should; a busca da 0.1 é FTS5 (sem acentos) + entidades + vizinhos no grafo,
  fundidos por RRF.

## ADR-008 Integração com o Anthive

- **Contexto:** o Anthive é TypeScript/Bun e continua como está.
- **Decisão:** contrato de arquivos (o Novo lê `anthive status --json` e `$ANTHIVE_HOME/projects.json`) e o Novo como
  cliente do `anthive mcp` (`note_write`, `agent_create`), identificado como o agente `novo`.
- **Alternativa:** reescrever o Anthive em Rust agora.
- **Consequências:** delegar uma tarefa cria um agente de verdade no Anthive, por isso exige confirmação explícita.
- **Estado:** implementado; `note_write` verificado contra o binário real do Anthive num `ANTHIVE_HOME` temporário.
  `agent_create` não foi executado de verdade.

## ADR-009 Superthreads

- **Contexto:** relacionar threads deve funcionar como lego sem virar emaranhado.
- **Decisão:** árvore: cada peça com no máximo um pai; arestas `block_of` com posição em `graph.jsonl`; relacionar age
  topo contra topo; superthread com um bloco só se desfaz.
- **Alternativa:** grafo livre de vínculos entre threads.
- **Consequências:** desmontar sempre devolve as peças; não há ciclos.
- **Estado:** implementado; teste de propriedades (400 casos) e de 10 mil operações em `crates/novo-graph/tests/props.rs`.

## ADR-010 Métricas

- **Contexto:** a Observabilidade precisa de números confiáveis.
- **Decisão:** ignorados, contagens, "mais lido" e % de FOMO são calculados por código; o LLM só escreve resumo e temas.
- **Alternativa:** pedir os números ao LLM.
- **Consequências:** recalcular um dia dá os mesmos números.
- **Estado:** implementado; teste `uc08_focus_report_and_determinism`.

## ADR-011 Relógio da Sessão Foco

- **Contexto:** o relatório tem que sair à meia-noite de Brasília, com o Mac em qualquer fuso, ligado ou não.
- **Decisão:** agendador do daemon em UTC (03:00 = 00:00 GMT-3, fuso fixo), conferido a cada minuto, com recuperação
  dos dias faltantes ao acordar (até 7).
- **Alternativa:** agendamento do launchd no fuso local.
- **Consequências:** noites com o Mac desligado por mais de 7 dias perdem os dias mais antigos.
- **Estado:** implementado; teste `fourteen_nights_in_a_row_with_the_mac_off_on_some` (14 noites, 4 desligadas,
  fuso do processo em Tóquio).

## ADR-012 Servidor MCP escrito à mão

- **Contexto:** RF-MCP-01 pede o SDK `rmcp`.
- **Decisão:** JSON-RPC sobre stdio escrito à mão (protocolo 2025-06-18), como o Anthive faz.
- **Alternativa:** `rmcp`.
- **Consequências:** sem dependência de uma API de SDK ainda instável; o comportamento externo (tools, resources,
  prompts, anotações) é o mesmo.
- **Estado:** implementado; verificado com trocas reais de mensagens pelo stdio do `novo mcp`, não dentro do
  Claude Desktop.
