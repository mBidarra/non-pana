# Relatório da entrega — Anthive Novo 0.1

Implementação completa da especificação, em Rust, local, para macOS.
Janela: 26/10/2026, 02:38 → 09:20 (GMT-3). Uma sessão, com uma revisão de código independente feita por outro agente.

Os critérios são os de "10.1 Critério de pronto do MVP" da especificação. **VERIFICADO** quer dizer evidência real
(teste que roda o código de verdade, ou execução contra o serviço real). Onde o teste usa um `claude` falso ou um
servidor imitando o OpenRouter, isso está dito.

## Critérios de pronto

| # | Critério | Estado | Evidência |
|---|---|---|---|
| 1 | Todos os requisitos Must implementados e cobertos por teste | IMPLEMENTADO, NÃO VERIFICADO | Os espaços, o menu ⋯, superthreads, caixas, Foco, Observabilidade, Tasks, MCP e Desenvolvimento estão implementados. Não há uma matriz requisito por requisito ligando cada RF Must a um teste; a cobertura é pelos casos de uso (critério 2) e pela navegação manual da interface no navegador. |
| 2 | UC-01 a UC-12 passam de ponta a ponta | IMPLEMENTADO, NÃO VERIFICADO (10 de 12 verificados) | `crates/novo-service/tests/e2e.rs`, 18 testes passando: UC-01/02, 03, 04/05, 06, 07, 08, 09, 11, 12. UC-01 também rodou contra o `claude` real (opinião com 3 trechos literais, haiku, ~US$ 0,0025). **UC-10** (Claude Desktop): servidor MCP verificado pelo stdio do `novo mcp`, não dentro do Claude Desktop, porque isso exige mudar a configuração real dele. **UC-11** (OpenRouter): verificado contra um servidor local no formato do OpenRouter, não contra o serviço. |
| 3 | 14 relatórios do Foco seguidos, incluindo noites com o Mac desligado | IMPLEMENTADO, NÃO VERIFICADO em 14 noites reais | `tests/focus_nights.rs`: 14 noites com relógio simulado, 4 com o Mac "desligado", processo no fuso de Tóquio; saem 14 relatórios seguidos, cada um com a janela 00:00 GMT-3 do próprio dia, e os perdidos são recuperados ao "ligar". O laço do agendador (conferência a cada 60 s no `novod`) não foi exercitado por 14 dias reais. |
| 4 | Teste de propriedades das superthreads e do grafo sem perda em 10 mil operações | VERIFICADO | `crates/novo-graph/tests/props.rs`: proptest com 400 casos e o teste de 10 mil operações, passando. |
| 5 | O contexto respeita o grafo e os tetos das caixas, conferido no diário | VERIFICADO | `uc12_context_uses_only_related_boxes` (só caixas ligadas no grafo, conferido em `boxes_used` do diário) e `context_respects_box_caps_in_journal` (16 peças ligadas, 12 caixas no diário, tamanho enviado dentro do teto); `neighbor_text_respects_token_cap` em `novo-memory`. A montagem do contexto e o diário são reais; o modelo é o `claude` falso. |
| 6 | Um perfil Claude Code pela assinatura e um perfil OpenRouter direto respondendo na mesma thread | BLOQUEADO (metade OpenRouter) | Claude Code pela assinatura verificado contra o `claude` real. OpenRouter direto testado só contra o servidor local (`uc11_openrouter_profile_answers_in_thread`, os dois perfis na mesma thread): falta uma chave do OpenRouter, e a chave tem que entrar no Keychain por você. |
| 7 | `novo doctor` verde numa máquina limpa, instalação por `install.sh` | IMPLEMENTADO, NÃO VERIFICADO | `install.sh` escrito; rodar com um `HOME` vazio foi recusado pela permissão da sessão. `novo doctor` num cofre novo: 8 verificações ok e 3 avisos esperados (daemon parado, LaunchAgent e MCP do Claude Desktop não instalados), que só ficam verdes depois de `novo install-daemon --load` e `novo install-mcp --claude-desktop` — os dois mexem no seu sistema, então ficaram para você. |
| 8 | Índice apagado e reconstruído sem perda | VERIFICADO | `index_file_deleted_and_rebuilt_without_loss`: apaga `.index/` do disco, reabre o serviço e a busca devolve os mesmos itens, com a mesma contagem. |
| 9 | Suíte de prompt injection sem nenhuma ação indevida | VERIFICADO (estrutural) | `prompt_injection_in_material_stays_data`: página que tenta fechar a moldura (`</UNTRUSTED-CONTENT >`) e mandar criar tarefa, relacionar threads e vazar o token. Confere que a injeção só chega ao agente dentro da moldura, que ele roda sem ferramentas, que nada foi criado e que a proposta do chat só vale com o clique. O teste encontrou uma falha real (textos do agente voltavam sem moldura no turno seguinte), já corrigida. Como o modelo é falso, o que se verifica é a garantia estrutural, não a reação de um LLM de verdade. |
| 10 | ADRs da seção 8.3 registrados | VERIFICADO | [`docs/ADR.md`](ADR.md): os 11 da seção 8.3 mais o do servidor MCP escrito à mão, cada um com o estado na 0.1. |

## Revisão independente

Um agente revisou o serviço e a interface sem o meu viés de autor e achou 11 problemas reais. Todos foram corrigidos;
os marcados com teste têm teste de regressão.

| Problema | Correção |
|---|---|
| Impasse entre as travas do índice e do grafo (daemon travava) | Ordem única das travas; o grafo é copiado antes de pegar o índice |
| Thread privada chegando a LLM remoto e ao MCP (caixa vizinha, título automático da superthread, relatório do Foco, recurso de projeto, `novo://box/.versions/../t-…`) | Privada nunca entra no contexto de outra thread nem sai pelo MCP; título automático sem o texto dela — **teste** `private_neighbor_never_reaches_another_thread` |
| Excluir durante a extração estragava o restaurar | A extração confere se a thread existe; restaurar valida antes de mover — **teste** `delete_during_extraction_then_restore` |
| Caixa e título da superthread sobrescreviam edição sua feita durante a chamada ao agente | Relê depois da chamada; edição sua e superthread desfeita prevalecem |
| Fila: pedido novo absorvido pelo job em andamento; resumo duplicado ao tentar de novo no modo "ambos" | Jobs em andamento fora da deduplicação; modo já respondido não responde de novo |
| XSS por link `javascript:` no cartão do material | A interface só põe `href` em http(s); o serviço recusa outros esquemas — **teste** `ids_and_links_are_validated` |
| `novo_post` do MCP repassava `files` e `file://` | Só os campos do schema; MCP e extensão não anexam arquivos locais — **teste** |
| Leitura de qualquer arquivo por `/attachment/%2F…`, escrita fora do cofre por `box.save` com `../`, comando no shell por `dev.open` | Ids validados na entrada da API; caminho do anexo só com componentes normais (conferido por HTTP: 400); `dev.open` só abre pastas de projetos cadastrados, com aspas do shell — **teste** `sh_quote_keeps_everything_literal` |
| Restaurar recriava arestas para uma vizinha na lixeira | Só arestas entre peças que existem; a estrutura volta inteira ou não volta; vínculos com projeto voltam um a um — **teste** `restore_skips_edges_to_trashed_neighbors` |
| O observador de arquivos voltava o grafo a um estado parcial | Recarrega sob a trava de escrita |
| Moldura de dado não confiável escapável por maiúsculas; caixas e respostas do agente sem moldura | Escape sem diferenciar caixa; caixas e textos do agente com moldura — **teste** de injeção |

Sem teste determinístico: as correções de concorrência (travas, observador, fila, releitura depois do agente). Elas
estão no código e a suíte inteira passa, mas uma corrida não se reproduz de forma confiável num teste.

## Como está agora

- `cargo test --workspace`: todos passando (núcleo, grafo, caixas, Foco, índice, extração, agentes, 18 de ponta a ponta,
  14 noites). `cargo clippy`: só avisos de estilo.
- `Anthive Novo.app` empacotado (`scripts/bundle-app.sh`), com `novod` e `novo` dentro; só arm64.
- Interface percorrida no navegador embutido contra um daemon de demonstração.
- Espaço Desenvolvimento: `note_write` verificado contra o binário real do Anthive num `ANTHIVE_HOME` temporário.

## Próximos passos recomendados

1. **OpenRouter (critério 6):** `novo key novo-openrouter` com a sua chave, criar um perfil `openrouter_http` em
   Configurações e pedir uma opinião numa thread que já tenha resposta do Claude.
2. **Máquina limpa (critério 7):** `./install.sh`, `novo install-daemon --load`, `novo install-mcp --claude-desktop`,
   reiniciar o Claude Desktop e rodar `novo doctor` (deve ficar todo verde).
3. **UC-10:** no Claude Desktop, pedir "publique no Novo: …" e "o que eu ignorei ontem?".
4. **Foco real (critério 3):** deixar o LaunchAgent rodando duas semanas e conferir `focus/` (14 arquivos seguidos).
5. **Matriz de rastreabilidade (critério 1):** ligar cada RF Must a um teste; os casos de uso cobrem a maior parte.
6. Embeddings locais (ADR-007, Should) e binário universal (x86_64) ficaram fora desta versão.
7. O token ainda vai na query string dos anexos e do SSE (o navegador não manda cabeçalho nesses casos); trocar por
   cookie `HttpOnly` de sessão local fecha isso.
