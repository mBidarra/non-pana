// Espaço Rede Social: compositor, feed, thread, superthread, menu ⋯, Relacionar e chat sobre a thread.
import { rpc, attachmentUrl } from './api.js';
import { h, mount, md, unescapeMd, ago, toast, errorToast, modal, confirmBox, prompt, menu, KIND, KIND_LABEL, kindOf, t } from './ui.js';

let profiles = [];
export async function loadProfiles() {
  try { profiles = (await rpc('profile.list')).map((p) => p.profile); } catch { profiles = []; }
  return profiles;
}

// ------------------------------------------------------------------ compositor (RF-PUB-01, 02, 07)

export function composer(onDone, initial = '') {
  const text = h('textarea', { class: 'compose-text', rows: 3, placeholder: t('O que você está pensando, lendo ou vendo?'), 'aria-label': 'Texto da thread' }, initial);
  let kind = /https?:\/\//.test(initial) ? 'reading' : 'thinking';
  const kinds = h('div', { class: 'seg', role: 'radiogroup', 'aria-label': 'Tipo' },
    ['thinking', 'reading', 'watching'].map((k) => h('button', { type: 'button', role: 'radio', 'aria-checked': k === kind ? 'true' : 'false', class: k === kind ? 'on' : '', 'data-k': k, onclick: (e) => {
      kind = k; kinds.querySelectorAll('button').forEach((b) => { b.classList.toggle('on', b.dataset.k === k); b.setAttribute('aria-checked', b.dataset.k === k); });
      autoKind = false; e.preventDefault();
    } }, `${KIND[k]} ${t(KIND_LABEL[k])}`)));
  let autoKind = true;
  text.addEventListener('input', () => {
    if (autoKind) {
      const hasUrl = /https?:\/\//.test(text.value);
      const want = hasUrl ? 'reading' : 'thinking';
      if (want !== kind) { kind = want; kinds.querySelectorAll('button').forEach((b) => b.classList.toggle('on', b.dataset.k === kind)); }
    }
  });
  const agent = h('select', { 'aria-label': t('Pedir ao agente') },
    h('option', { value: 'off' }, `${t('Pedir ao agente')}: ${t('desligado')}`),
    h('option', { value: 'summary' }, `${t('Pedir ao agente')}: ${t('resumo')}`),
    h('option', { value: 'opinion' }, `${t('Pedir ao agente')}: ${t('opinião')}`),
    h('option', { value: 'both' }, `${t('Pedir ao agente')}: ${t('os dois')}`));
  const prof = h('select', { 'aria-label': 'Perfil de agente', title: 'Perfil de agente' },
    profiles.map((p) => h('option', { value: p.name, selected: p.default }, p.label || p.name)));
  const priv = h('input', { type: 'checkbox', id: 'compose-private' });
  priv.addEventListener('change', () => {
    if (priv.checked && agent.value !== 'off') {
      const p = profiles.find((x) => x.name === prof.value);
      if (p && p.engine !== 'ollama') { agent.value = 'off'; toast('Thread privada: o agente remoto fica indisponível.'); }
    }
  });
  const btn = h('button', { class: 'primary', type: 'submit' }, t('Publicar'));
  const send = async () => {
    if (!text.value.trim()) { text.focus(); return; }
    btn.disabled = true;
    try {
      const v = await rpc('thread.create', { text: text.value, kind, agent: agent.value, profile: prof.value || null, private: priv.checked });
      text.value = ''; agent.value = 'off'; priv.checked = false; autoKind = true;
      onDone?.(v);
    } catch (e) { errorToast(e); } finally { btn.disabled = false; }
  };
  text.addEventListener('keydown', (e) => { if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) { e.preventDefault(); send(); } });
  const form = h('form', { class: 'composer', onsubmit: (e) => { e.preventDefault(); send(); } },
    text,
    h('div', { class: 'compose-bar' }, kinds, h('div', { class: 'grow' }), agent, prof,
      h('label', { class: 'check', for: 'compose-private', title: 'Nunca vai a um LLM remoto' }, priv, ` 🔒 ${t('Privada')}`), btn));
  form.focusText = () => text.focus();
  return form;
}

// ------------------------------------------------------------------ cartão da thread

function materialCard(m, threadId) {
  if (!m) return null;
  const snap = m.has_snapshot ? h('a', { href: attachmentUrl(threadId, 'snapshot-0.md'), target: '_blank', rel: 'noopener', class: 'muted small' }, 'cópia salva') : null;
  return h('div', { class: `material ${m.status}` },
    m.image ? h('img', { src: m.image, alt: '', loading: 'lazy', referrerpolicy: 'no-referrer', onerror: (e) => e.target.remove() }) : null,
    h('div', { class: 'material-body' },
      h('div', { class: 'muted small' }, m.site || (m.kind === 'file' ? 'arquivo' : ''), m.status === 'pending' ? ' · extraindo…' : '', m.status === 'failed' ? ` · falhou: ${m.error || ''}` : ''),
      h('a', { href: /^https?:\/\//i.test(m.url) ? m.url : '#', target: '_blank', rel: 'noopener noreferrer', class: 'material-title' }, m.title || m.url),
      m.excerpt ? h('div', { class: 'small clamp' }, m.excerpt) : null,
      snap));
}

export function agentBadge(c) {
  if (!c.agent_state || (c.agent === 'off' && c.agent_state === 'done')) return null;
  const label = { queued: 'agente na fila', reading: 'agente lendo…', replying: 'agente respondendo…', failed: 'agente falhou', done: 'agente respondeu' }[c.agent_state] || c.agent_state;
  return h('span', { class: `badge agent-${c.agent_state}`, title: c.agent_detail || '' }, label);
}

export function threadCard(c, { onChange, selected } = {}) {
  const li = h('button', {
    class: `li ${c.read ? 'on' : ''}`, title: c.read ? 'Desmarcar Li' : 'Marcar Li', 'aria-pressed': c.read ? 'true' : 'false',
    onclick: async (e) => { e.stopPropagation(); try { await rpc('thread.read', { id: c.id, read: !c.read }); onChange?.(); } catch (er) { errorToast(er); } },
  }, c.read ? '✓ Li' : 'Li');
  const more = h('button', { class: 'icon', 'aria-label': 'Mais ações', title: 'Mais ações', onclick: (e) => { e.stopPropagation(); threadMenu(more, c.id, { onChange }); } }, '⋯');
  return h('article', { class: `card ${selected ? 'selected' : ''} ${c.ignored ? 'ignored' : ''}`, tabindex: 0, 'data-id': c.id, onclick: () => (location.hash = `#/t/${c.id}`) },
    h('div', { class: 'card-head' },
      h('span', { class: 'kind', title: t(KIND_LABEL[c.kind]) }, KIND[c.kind]),
      h('span', { class: 'muted small' }, ago(c.created)),
      c.pinned ? h('span', { class: 'small', title: 'Fixada' }, '📌') : null,
      c.private ? h('span', { class: 'small', title: 'Privada' }, '🔒') : null,
      c.has_material && !c.read ? h('span', { class: `age ${c.ignored ? 'warn' : ''}` }, c.ignored ? `ignorada há ${c.age_hours} h` : `sem Li há ${c.age_hours} h`) : null,
      agentBadge(c),
      h('div', { class: 'grow' }), li, more),
    h('div', { class: 'card-text', html: md(c.excerpt) }),
    c.material?.length ? materialCard(c.material[0], c.id) : null,
    h('div', { class: 'chips' },
      c.superthread ? h('a', { class: 'chip st', href: `#/st/${c.superthread}`, onclick: (e) => e.stopPropagation() }, `⧉ ${c.superthread_title || c.superthread}`) : null,
      c.relations.map((r) => h('a', { class: `chip ${r.type}`, href: r.type === 'project' ? `#/tasks/p/${r.id}` : `#/tasks/k/${r.id}`, onclick: (e) => e.stopPropagation() }, `${r.type === 'project' ? '▣' : '☐'} ${r.title}`)),
      c.replies ? h('span', { class: 'muted small' }, `${c.replies} resposta(s)`) : null,
      c.agent_replies ? h('span', { class: 'muted small' }, `${c.agent_replies} do agente`) : null));
}

// ------------------------------------------------------------------ menu ⋯ (RF-MEN-01 a 09)

export function threadMenu(anchor, id, { onChange, isSuper = false, parent = null } = {}) {
  const items = [
    { label: `🗑 ${t('Excluir')}`, danger: true, run: () => deleteThread(id, onChange) },
    { label: `💬 ${t('Abrir chat sobre a thread')}`, run: () => openChat(id) },
    { label: `🔗 ${t('Relacionar ao projeto, thread ou tarefa')}`, run: () => openPicker(id, onChange) },
  ];
  if (isSuper) {
    items.splice(0, 1);
    items.push({ label: '✎ Renomear', run: async () => { const v = await prompt('Renomear superthread'); if (v) { await rpc('superthread.rename', { id, title: v }).catch(errorToast); onChange?.(); } } });
  }
  if (parent) items.push({ label: '⇱ Desmontar (tirar da superthread)', run: async () => { await rpc('superthread.disassemble', { block: id }).then(() => toast('Peça devolvida.')).catch(errorToast); onChange?.(); } });
  if (!isSuper) items.push({ label: '📌 Fixar / desafixar', run: async () => { const v = await rpc('thread.get', { id }); await rpc('thread.pin', { id, pinned: !v.pinned }).catch(errorToast); onChange?.(); } });
  menu(anchor, items);
}

async function deleteThread(id, onChange) {
  const ok = await confirmBox('Excluir thread', 'A thread vai para a lixeira por 30 dias e sai da superthread em que estiver. Dá para desfazer.', t('Excluir'), true);
  if (!ok) return;
  try {
    const r = await rpc('thread.delete', { id });
    toast(r.dissolved?.length ? 'Thread excluída; a superthread com um só bloco se desfez.' : 'Thread excluída.', { label: t('Desfazer'), run: async () => { await rpc('thread.restore', { id }).catch(errorToast); onChange?.(); } });
    if (location.hash.includes(id)) location.hash = '#/feed';
    onChange?.();
  } catch (e) { errorToast(e); }
}

/** Seletor único do Relacionar (RF-MEN-06): projetos, tarefas, threads e superthreads, ou tarefa nova. */
export async function openPicker(source, onChange, kinds = null) {
  const input = h('input', { type: 'search', class: 'wide', placeholder: 'Buscar projeto, tarefa, thread ou superthread…', 'aria-label': 'Buscar' });
  const list = h('div', { class: 'pick-list', role: 'listbox' });
  let close;
  const choose = async (target, newTask) => {
    try {
      const r = newTask ? await rpc('relate', { a: source, new_task: newTask }) : await rpc('relate', { a: source, b: target });
      close?.();
      if (r?.created) toast('Superthread criada. O agente vai sugerir um título.', { label: 'Abrir', run: () => (location.hash = `#/st/${r.created}`) });
      else if (newTask) toast('Tarefa criada e relacionada.');
      else toast('Relacionado.');
      onChange?.();
    } catch (e) { errorToast(e); }
  };
  const search = async () => {
    const q = input.value.trim();
    let hits = [];
    try { hits = await rpc('pick', { q, exclude: source }); } catch (e) { errorToast(e); }
    if (kinds) hits = hits.filter((x) => kinds.includes(x.kind));
    list.innerHTML = '';
    const groups = { project: 'Projetos', task: 'Tarefas', superthread: 'Superthreads', thread: 'Threads' };
    for (const [k, label] of Object.entries(groups)) {
      const hs = hits.filter((x) => x.kind === k);
      if (!hs.length) continue;
      list.append(h('div', { class: 'pick-group' }, label));
      for (const x of hs.slice(0, 8)) list.append(h('button', { class: 'pick-item', role: 'option', onclick: () => choose(x.id) }, h('strong', {}, x.title), h('span', { class: 'muted small' }, ` ${x.id}`)));
    }
    if (q && (!kinds || kinds.includes('task'))) list.append(h('button', { class: 'pick-item new', onclick: () => choose(null, q) }, `＋ Criar tarefa “${q}” já relacionada`));
    if (!list.children.length) list.append(h('div', { class: 'muted small' }, 'Nada encontrado.'));
  };
  let timer;
  input.addEventListener('input', () => { clearTimeout(timer); timer = setTimeout(search, 150); });
  input.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); list.querySelector('button')?.click(); } });
  search();
  const p = modal('Relacionar', h('div', {}, h('p', { class: 'muted small' }, 'Thread com thread monta uma superthread; projeto e tarefa viram vínculo.'), input, list), [{ label: 'Fechar', value: null }]);
  close = () => document.querySelector('#modal-root .backdrop')?.click();
  await p;
}

// ------------------------------------------------------------------ feed

const FILTERS = [
  ['all', 'Todas', {}], ['pending', 'Pendentes de Li', { pending: true }], ['agent', 'Com agente', { agent: true }], ['ignored', 'Ignoradas', { ignored: true }],
];

export async function renderFeed(main, state) {
  state.filter ??= 'all';
  main.innerHTML = '';
  const list = h('div', { class: 'feed', role: 'feed', 'aria-label': 'Feed' });
  const refresh = async () => {
    const f = FILTERS.find((x) => x[0] === state.filter)[2];
    let cards = [];
    try { cards = await rpc('feed', { ...f, project: state.project || null, limit: 200 }); } catch (e) { errorToast(e); }
    list.innerHTML = '';
    if (!cards.length) list.append(h('div', { class: 'empty' }, state.filter === 'all' ? 'Nada publicado ainda. Comece pelo que está lendo agora.' : 'Nada aqui.'));
    cards.forEach((c, i) => list.append(threadCard(c, { onChange: refresh, selected: i === state.sel })));
    state.cards = cards;
  };
  state.refresh = refresh;
  const projects = await rpc('project.list').catch(() => []);
  const comp = composer(() => { refresh(); toast('Publicada.'); });
  state.composer = comp;
  mount(main, 
    h('header', { class: 'view-head' }, h('h1', {}, t('Rede Social'))),
    comp,
    h('div', { class: 'filters' },
      FILTERS.map(([k, label]) => h('button', { class: `chip-btn ${state.filter === k ? 'on' : ''}`, onclick: () => { state.filter = k; renderFeed(main, state); } }, t(label))),
      h('div', { class: 'grow' }),
      h('select', { 'aria-label': 'Filtrar por projeto', onchange: (e) => { state.project = e.target.value || null; refresh(); } },
        h('option', { value: '' }, 'Todos os projetos'), projects.map((p) => h('option', { value: p.project.id, selected: state.project === p.project.id }, p.project.name)))),
    list);
  await refresh();
}

// ------------------------------------------------------------------ thread

function post(p, threadId, onReply) {
  const agent = p.is_agent;
  return h('div', { class: `post ${agent ? 'agent' : 'mine'} ${p.mode === 'error' ? 'error' : ''}`, id: `post-${p.id}` },
    h('div', { class: 'post-head' },
      h('strong', {}, agent ? `✦ ${p.author.replace('profile:', '')}` : 'você'),
      agent && p.model ? h('span', { class: 'muted small' }, ` · ${p.model}`) : null,
      agent && p.mode ? h('span', { class: 'badge' }, { summary: 'resumo', opinion: 'opinião', followup: 'resposta', chat_summary: 'resumo do chat', error: 'erro' }[p.mode] || p.mode) : null,
      p.reply_to ? h('a', { class: 'muted small', href: `#/t/${threadId}`, onclick: (e) => { e.preventDefault(); document.getElementById(`post-${p.reply_to}`)?.scrollIntoView({ behavior: 'smooth', block: 'center' }); } }, ` ↪ ${p.reply_to}`) : null,
      p.edited ? h('span', { class: 'muted small' }, ' · editado') : null,
      h('span', { class: 'muted small' }, ` · ${ago(p.ts)}`),
      h('div', { class: 'grow' }),
      h('button', { class: 'link small', onclick: () => onReply(p) }, t('Responder'))),
    h('div', { class: 'post-body', html: md(p.body) }),
    p.quotes?.length ? h('div', { class: 'quotes' }, p.quotes.map((q) => h('blockquote', {}, `“${unescapeMd(q)}”`))) : null);
}

export function boxPanel(id, box, onChange) {
  if (!box) return null;
  const edit = async () => {
    const f = {
      abstract: h('input', { class: 'wide', value: box.abstract_ }),
      summary: h('textarea', { class: 'wide', rows: 4 }, box.summary),
      stance: h('textarea', { class: 'wide', rows: 2 }, box.stance),
      questions: h('textarea', { class: 'wide', rows: 2 }, box.questions.join('\n')),
      key_points: h('textarea', { class: 'wide', rows: 3 }, box.key_points.join('\n')),
    };
    const ok = await modal('Editar caixa de memória', h('div', { class: 'form' },
      h('label', {}, 'Em uma linha', f.abstract), h('label', {}, 'Resumo', f.summary), h('label', {}, 'Sua posição', f.stance),
      h('label', {}, 'Perguntas abertas (uma por linha)', f.questions), h('label', {}, 'Pontos-chave (um por linha)', f.key_points),
      h('p', { class: 'muted small' }, 'A edição vira uma versão sua; o agente parte dela e preserva o que você escreveu.')),
    [{ label: 'Cancelar', value: false }, { label: 'Salvar', primary: true, value: true }]);
    if (!ok) return;
    const lines = (el) => el.value.split('\n').map((s) => s.trim()).filter(Boolean);
    await rpc('box.save', { id, box: { abstract: f.abstract.value, summary: f.summary.value, stance: f.stance.value, questions: lines(f.questions), key_points: lines(f.key_points), quotes: box.quotes, entities: box.head.entities } }).catch(errorToast);
    onChange?.();
  };
  return h('details', { class: 'box-panel' },
    h('summary', {}, '🧠 Caixa de memória ', h('span', { class: 'muted small' }, `v${box.head.version} · ${{ agent: 'agente', voce: 'você', fallback: 'automática' }[box.head.edited_by] || box.head.edited_by}`)),
    h('p', {}, h('strong', {}, box.abstract_)),
    h('p', {}, box.summary),
    box.stance ? h('p', {}, h('em', {}, 'Sua posição: '), box.stance) : null,
    box.questions.length ? h('ul', {}, box.questions.map((q) => h('li', {}, q))) : null,
    box.head.entities.length ? h('div', { class: 'chips' }, box.head.entities.map((e) => h('span', { class: 'chip' }, `#${e}`))) : null,
    h('button', { class: 'link small', onclick: edit }, 'Editar caixa'));
}

export async function renderThread(main, id, state) {
  let v;
  try { v = await rpc('thread.get', { id }); } catch (e) { main.innerHTML = ''; mount(main, h('div', { class: 'empty' }, `Thread não encontrada: ${e.message}`)); return; }
  const refresh = () => renderThread(main, id, state);
  state.refresh = refresh;
  let replyTo = null;
  const replyTxt = h('textarea', { rows: 2, class: 'wide', placeholder: 'Responder… (mencione @agente para chamar o agente)', 'aria-label': 'Resposta' });
  const replyHint = h('div', { class: 'muted small' });
  const sendReply = async () => {
    if (!replyTxt.value.trim()) return;
    try { await rpc('thread.reply', { id, body: replyTxt.value, reply_to: replyTo }); replyTxt.value = ''; refresh(); } catch (e) { errorToast(e); }
  };
  replyTxt.addEventListener('keydown', (e) => { if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) { e.preventDefault(); sendReply(); } });
  const setReply = (p) => { replyTo = p.id; replyHint.textContent = `Respondendo a ${p.id} (${p.is_agent ? 'agente' : 'você'})`; replyTxt.focus(); };
  const agentSel = h('select', { 'aria-label': 'Agente nesta thread', onchange: async (e) => {
    try { await rpc('thread.agent', { id, mode: e.target.value, profile: profSel.value || null }); refresh(); } catch (er) { errorToast(er); refresh(); }
  } }, ['off', 'summary', 'opinion', 'both'].map((m) => h('option', { value: m, selected: v.agent === m }, `Agente: ${t({ off: 'desligado', summary: 'resumo', opinion: 'opinião', both: 'os dois' }[m])}`)));
  const profSel = h('select', { 'aria-label': 'Perfil' }, profiles.map((p) => h('option', { value: p.name, selected: (v.profile || profiles.find((x) => x.default)?.name) === p.name }, p.label || p.name)));
  const more = h('button', { class: 'icon', 'aria-label': 'Mais ações', onclick: () => threadMenu(more, id, { onChange: refresh, parent: v.superthread }) }, '⋯');
  main.innerHTML = '';
  mount(main, 
    h('header', { class: 'view-head' },
      h('a', { href: '#/feed', class: 'back' }, '← Feed'),
      h('div', { class: 'grow' }),
      h('button', { class: `li ${v.read ? 'on' : ''}`, onclick: async () => { await rpc('thread.read', { id, read: !v.read }).catch(errorToast); refresh(); } }, v.read ? '✓ Li' : 'Marcar Li'),
      v.chat ? h('button', { class: 'icon', title: 'Chat sobre a thread', onclick: () => openChat(id) }, '💬') : null,
      more),
    h('div', { class: 'thread' },
      h('div', { class: 'card-head' }, h('span', { class: 'kind' }, KIND[v.kind]), h('span', { class: 'muted small' }, `${t(KIND_LABEL[v.kind])} · ${ago(v.created)} · ${v.source}`), v.private ? h('span', {}, ' 🔒') : null, agentBadge(v)),
      v.material.map((m, i) => h('div', {},
        materialCard(m, id),
        m.status === 'failed' ? h('button', { class: 'link small', onclick: async () => {
          const ta = h('textarea', { rows: 8, class: 'wide', placeholder: 'Cole aqui o texto do material' });
          const ok = await modal('Colar o texto do material', ta, [{ label: 'Cancelar', value: false }, { label: 'Salvar', primary: true, value: true }]);
          if (ok && ta.value.trim()) { await rpc('thread.paste', { id, index: i, text: ta.value }).catch(errorToast); refresh(); }
        } }, 'Colar o texto à mão') : null)),
      h('div', { class: 'chips' },
        v.superthread ? h('a', { class: 'chip st', href: `#/st/${v.superthread}` }, `⧉ ${v.superthread_title}`) : null,
        v.relations.map((r) => h('span', { class: `chip ${r.type}` }, h('a', { href: r.type === 'project' ? `#/tasks/p/${r.id}` : `#/tasks/k/${r.id}` }, `${r.type === 'project' ? '▣' : '☐'} ${r.title}`),
          h('button', { class: 'x', title: 'Desfazer vínculo', onclick: async () => { await rpc('unrelate', { a: id, b: r.id }).catch(errorToast); refresh(); } }, '×')))),
      v.posts.map((p) => post(p, id, setReply)),
      v.agent_state === 'failed' ? h('div', { class: 'notice' }, `O agente falhou: ${v.agent_detail || ''} `, h('button', { onclick: async () => { await rpc('thread.retry', { id }).catch(errorToast); refresh(); } }, 'Tentar de novo')) : null,
      h('div', { class: 'reply-box' }, replyHint, replyTxt, h('div', { class: 'compose-bar' }, agentSel, profSel, h('div', { class: 'grow' }), h('button', { class: 'primary', onclick: sendReply }, t('Responder')))),
      boxPanel(id, v.box, refresh)));
}

// ------------------------------------------------------------------ superthread (RF-SUP-06 a 11)

export async function renderSuperthread(main, id, state) {
  let v;
  try { v = await rpc('superthread.get', { id }); } catch (e) { main.innerHTML = ''; mount(main, h('div', { class: 'empty' }, `Superthread não encontrada: ${e.message}`)); return; }
  const refresh = () => renderSuperthread(main, id, state);
  state.refresh = refresh;
  const blocks = h('ol', { class: 'blocks' });
  let dragging = null;
  const order = () => [...blocks.children].map((li) => li.dataset.id);
  v.blocks.forEach((b) => {
    const bid = b.kind === 'thread' ? b.card.id : b.id;
    const body = b.kind === 'thread'
      ? h('details', { open: v.blocks.length <= 3 },
        h('summary', {}, `${KIND[b.card.kind]} ${b.card.title}`, h('span', { class: 'muted small' }, ` · ${ago(b.card.created)} · ${b.card.read ? '✓ Li' : 'sem Li'}`)),
        h('div', { class: 'block-body' }, h('a', { href: `#/t/${bid}`, class: 'small' }, 'abrir thread'), b.posts.slice(0, 6).map((p) => h('div', { class: `post mini ${p.is_agent ? 'agent' : 'mine'}`, html: md(p.body) }))))
      : h('details', {}, h('summary', {}, `⧉ ${b.title}`, h('span', { class: 'muted small' }, ` · ${b.blocks.length} blocos`)),
        h('div', { class: 'block-body' }, h('p', {}, b.abstract || ''), h('a', { href: `#/st/${bid}`, class: 'small' }, 'abrir superthread')));
    const more = h('button', { class: 'icon', 'aria-label': 'Mais ações', onclick: () => threadMenu(more, bid, { onChange: refresh, parent: id, isSuper: b.kind !== 'thread' }) }, '⋯');
    const li = h('li', { class: 'block', draggable: 'true', 'data-id': bid,
      ondragstart: () => { dragging = li; li.classList.add('dragging'); },
      ondragend: async () => { li.classList.remove('dragging'); dragging = null; await rpc('superthread.reorder', { id, order: order() }).catch(errorToast); },
      ondragover: (e) => { e.preventDefault(); if (dragging && dragging !== li) { const r = li.getBoundingClientRect(); blocks.insertBefore(dragging, e.clientY < r.top + r.height / 2 ? li : li.nextSibling); } } },
    h('span', { class: 'grip', title: 'Arraste para reordenar' }, '⠿'), h('div', { class: 'grow' }, body), more);
    blocks.append(li);
  });
  const ask = h('textarea', { rows: 2, class: 'wide', placeholder: 'Pergunte ao agente o que estas threads têm em comum…' });
  const more = h('button', { class: 'icon', 'aria-label': 'Mais ações', onclick: () => threadMenu(more, id, { onChange: refresh, isSuper: true, parent: v.parent }) }, '⋯');
  main.innerHTML = '';
  mount(main, 
    h('header', { class: 'view-head' }, h('a', { href: '#/feed', class: 'back' }, '← Feed'), h('div', { class: 'grow' }),
      h('button', { onclick: async () => { const r = await rpc('superthread.read', { id }).catch(errorToast); if (r) toast(`${r.read} threads marcadas como Li.`); refresh(); } }, '✓ Li em todas'),
      more),
    h('div', { class: 'thread' },
      h('h1', { class: 'st-title' }, `⧉ ${v.title}`, v.title_by === 'agent' ? h('span', { class: 'muted small' }, ' (título sugerido pelo agente)') : null),
      v.parent ? h('p', { class: 'small' }, 'Dentro de ', h('a', { href: `#/st/${v.parent}` }, v.parent)) : null,
      h('div', { class: 'chips' }, v.relations.map((r) => h('a', { class: `chip ${r.type}`, href: r.type === 'project' ? `#/tasks/p/${r.id}` : `#/tasks/k/${r.id}` }, `${r.type === 'project' ? '▣' : '☐'} ${r.title}`))),
      boxPanel(id, v.box, refresh),
      h('h3', {}, `Blocos (${v.blocks.length})`),
      blocks,
      h('h3', {}, 'Conversa da superthread'),
      v.conversation.map((p) => h('div', { class: `post ${p.author === 'voce' ? 'mine' : 'agent'}` }, h('div', { class: 'post-head' }, h('strong', {}, p.author === 'voce' ? 'você' : `✦ ${p.author.replace('profile:', '')}`), h('span', { class: 'muted small' }, ` · ${ago(p.ts)}`)), h('div', { class: 'post-body', html: md(p.body) }))),
      h('div', { class: 'reply-box' }, ask, h('div', { class: 'compose-bar' }, h('div', { class: 'grow' }),
        h('button', { class: 'primary', onclick: async () => { if (!ask.value.trim()) return; await rpc('superthread.converse', { id, body: ask.value, agent: true }).catch(errorToast); ask.value = ''; toast('O agente está comparando as threads…'); refresh(); } }, 'Perguntar')))));
}

// ------------------------------------------------------------------ chat sobre a thread (RF-MEN-04, 05)

export async function openChat(target) {
  const drawer = document.getElementById('drawer');
  let chat;
  try { chat = await rpc('chat.open', { target }); } catch (e) { errorToast(e); return; }
  const input = h('textarea', { rows: 3, class: 'wide', placeholder: 'Converse com o agente sobre esta thread…' });
  const log = h('div', { class: 'chat-log' });
  const draw = (c) => {
    log.innerHTML = '';
    let pi = 0;
    for (const e of c.entries) {
      if (e.type === 'user') log.append(h('div', { class: 'post mine' }, h('div', { class: 'post-body', html: md(e.text) })));
      if (e.type === 'agent') {
        log.append(h('div', { class: 'post agent' }, h('div', { class: 'post-head' }, h('strong', {}, '✦ agente'), h('span', { class: 'muted small' }, ` · ${e.model}`)), h('div', { class: 'post-body', html: md(e.text) }),
          e.proposals.map((p) => { const idx = pi++; return h('div', { class: 'proposal' }, h('span', {}, `${p.kind === 'task' ? '☐ Tarefa' : '🔗 Relação'}: ${p.title} `, h('span', { class: 'muted small' }, p.reason)),
            h('button', { onclick: async () => { const r = await rpc('chat.apply', { id: c.id, index: idx }).catch(errorToast); if (r) toast(`Aplicado: ${r.result}`); } }, 'Aplicar')); })));
      }
      if (e.type === 'applied') log.append(h('div', { class: 'muted small' }, `✓ ${e.result}`));
      if (e.type === 'published') log.append(h('div', { class: 'muted small' }, `↗ resumo publicado na thread (${e.post})`));
    }
    log.scrollTop = log.scrollHeight;
  };
  const send = async () => {
    if (!input.value.trim()) return;
    const text = input.value; input.value = '';
    log.append(h('div', { class: 'post mine' }, h('div', { class: 'post-body', html: md(text) })), h('div', { class: 'muted small typing' }, 'o agente está pensando…'));
    try { chat = await rpc('chat.send', { id: chat.id, text }); } catch (e) { errorToast(e); chat = await rpc('chat.get', { id: chat.id }); }
    draw(chat);
  };
  input.addEventListener('keydown', (e) => { if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) { e.preventDefault(); send(); } });
  drawer.innerHTML = '';
  mount(drawer, 
    h('header', { class: 'view-head' }, h('h2', {}, `💬 ${chat.title}`), h('div', { class: 'grow' }), h('button', { class: 'icon', 'aria-label': 'Fechar', onclick: () => { drawer.hidden = true; } }, '×')),
    h('p', { class: 'muted small' }, 'O agente lê o cofre, mas não grava nada sem o seu clique.'),
    log, input,
    h('div', { class: 'compose-bar' },
      kindOf(target) === 'thread' ? h('button', { onclick: async () => { const r = await rpc('chat.publish', { id: chat.id }).catch(errorToast); if (r) toast('Resumo publicado na thread.'); } }, '↗ Publicar resumo na thread') : null,
      h('div', { class: 'grow' }), h('button', { class: 'primary', onclick: send }, 'Enviar')));
  drawer.hidden = false;
  draw(chat);
  input.focus();
}
