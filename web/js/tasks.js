// Espaço Tasks (spec 4.5): Entrada, Hoje, Próximos 7 dias, projetos, subtarefas, anotações e threads relacionadas.
import { rpc } from './api.js';
import { h, mount, md, toast, errorToast, modal, confirmBox, prompt, fmtDate, t } from './ui.js';
import { threadCard } from './feed.js';

const VIEWS = [['inbox', 'Entrada'], ['today', 'Hoje'], ['next7', 'Próximos 7 dias'], ['done', 'Concluídas']];
const PRIO = { 1: 'p1', 2: 'p2', 3: 'p3', 4: 'p4' };

function quickAdd(onDone, project) {
  const input = h('input', { type: 'text', class: 'wide', placeholder: 'Nova tarefa — ex.: revisar landing amanhã 9h p1 #marca', 'aria-label': 'Nova tarefa' });
  input.addEventListener('keydown', async (e) => {
    if (e.key !== 'Enter' || !input.value.trim()) return;
    try { await rpc('task.create', { quick: input.value, project: project || undefined }); input.value = ''; onDone(); } catch (er) { errorToast(er); }
  });
  return input;
}

function taskRow(r, onChange, selected) {
  const box = h('input', { type: 'checkbox', checked: r.status === 'done', 'aria-label': 'Concluir', onclick: (e) => e.stopPropagation(),
    onchange: async (e) => { try { await rpc('task.complete', { id: r.id, done: e.target.checked }); onChange(); if (e.target.checked) toast('Tarefa concluída.', { label: t('Desfazer'), run: async () => { await rpc('task.complete', { id: r.id, done: false }); onChange(); } }); } catch (er) { errorToast(er); } } });
  return h('div', { class: `task ${r.status === 'done' ? 'done' : ''} ${selected ? 'selected' : ''}`, tabindex: 0, 'data-id': r.id, onclick: () => (location.hash = `#/tasks/k/${r.id}`) },
    h('span', { class: `prio ${PRIO[r.priority]}`, title: `P${r.priority}` }), box,
    h('div', { class: 'grow' },
      h('div', {}, r.title, r.parent ? h('span', { class: 'muted small' }, ' · subtarefa') : null),
      h('div', { class: 'meta small' },
        r.due ? h('span', { class: r.overdue ? 'warn' : '' }, `📅 ${fmtDate(r.due)}`) : null,
        r.project_name ? h('span', {}, `▣ ${r.project_name}`) : null,
        r.labels.map((l) => h('span', { class: 'muted' }, `@${l}`)),
        r.subtasks_total ? h('span', { class: 'muted' }, `☐ ${r.subtasks_total - r.subtasks_open}/${r.subtasks_total}`) : null,
        r.threads ? h('span', { class: 'muted' }, `⧉ ${r.threads}`) : null,
        r.ignored ? h('span', { class: 'warn' }, r.ignored === 'overdue' ? 'ignorada: vencida' : 'ignorada: parada') : null,
        r.delegated_to ? h('span', { class: 'muted' }, `→ agente ${r.delegated_to}`) : null)));
}

export async function renderTasks(main, view, state) {
  main.innerHTML = '';
  const projects = await rpc('project.list').catch(() => []);
  const list = h('div', { class: 'task-list' });
  const refresh = async () => {
    let rows = [];
    try { rows = await rpc('task.list', { view }); } catch (e) { errorToast(e); }
    list.innerHTML = '';
    if (!rows.length) list.append(h('div', { class: 'empty' }, view === 'today' ? 'Nada para hoje.' : 'Nada aqui.'));
    rows.forEach((r, i) => list.append(taskRow(r, refresh, i === state.sel)));
    state.rows = rows;
  };
  state.refresh = refresh;
  mount(main, 
    h('header', { class: 'view-head' }, h('h1', {}, t('Tasks'))),
    h('div', { class: 'tasks-layout' },
      h('nav', { class: 'subnav' },
        VIEWS.map(([k, label]) => h('a', { href: `#/tasks/${k}`, class: k === view ? 'on' : '' }, t(label))),
        h('div', { class: 'subnav-title' }, t('Projetos'), h('button', { class: 'link small', onclick: async () => {
          const name = await prompt('Novo projeto');
          if (name) { const p = await rpc('project.create', { name }).catch(errorToast); if (p) location.hash = `#/tasks/p/${p.id}`; }
        } }, '＋')),
        projects.map((p) => h('a', { href: `#/tasks/p/${p.project.id}` }, `${p.project.type === 'development' ? '⌘' : '▣'} ${p.project.name}`, h('span', { class: 'muted small' }, ` ${p.open_tasks || ''}`)))),
      h('section', { class: 'grow' }, view !== 'done' ? quickAdd(refresh) : null, list)));
  await refresh();
}

export async function renderProject(main, id, state) {
  let v;
  try { v = await rpc('project.get', { id }); } catch (e) { main.innerHTML = ''; mount(main, h('div', { class: 'empty' }, e.message)); return; }
  const refresh = () => renderProject(main, id, state);
  state.refresh = refresh;
  const p = v.project;
  const body = h('textarea', { class: 'wide', rows: 3, placeholder: 'Perfil do projeto: objetivo, público, desafios…' }, v.body);
  body.addEventListener('change', () => rpc('project.update', { id, body: body.value }).catch(errorToast));
  main.innerHTML = '';
  mount(main, 
    h('header', { class: 'view-head' }, h('a', { href: '#/tasks/inbox', class: 'back' }, `← ${t('Tasks')}`), h('div', { class: 'grow' }),
      p.type === 'development' && p.dir ? h('button', { onclick: () => rpc('dev.open', { dir: p.dir }).then(() => toast('Abrindo no Anthive…')).catch(errorToast) }, '⌘ Abrir no Anthive') : null,
      h('button', { onclick: async () => { const n = await prompt('Renomear projeto', p.name); if (n) { await rpc('project.update', { id, name: n }).catch(errorToast); refresh(); } } }, 'Renomear'),
      h('button', { class: 'danger', onclick: async () => { if (await confirmBox('Arquivar projeto', 'O projeto some das listas; tarefas e threads ficam.', 'Arquivar', true)) { await rpc('project.update', { id, archived: true }).catch(errorToast); location.hash = '#/tasks/inbox'; } } }, 'Arquivar')),
    h('div', { class: 'thread' },
      h('h1', {}, `${p.type === 'development' ? '⌘' : '▣'} ${p.name}`, p.type === 'development' ? h('span', { class: 'muted small' }, ` · Anthive · ${p.dir || ''}`) : null),
      body,
      h('h3', {}, 'Tarefas'),
      quickAdd(refresh, id),
      h('div', { class: 'task-list' }, v.tasks.length ? v.tasks.map((r) => taskRow(r, refresh)) : h('div', { class: 'empty' }, 'Sem tarefas.')),
      h('h3', {}, 'Threads e superthreads relacionadas'),
      v.related.length ? v.related.map((x) => (x.kind === 'thread' ? threadCard(x.card, { onChange: refresh }) : x.kind === 'superthread' ? h('a', { class: 'chip st', href: `#/st/${x.id}` }, `⧉ ${x.title} (${x.threads} threads)`) : null)) : h('div', { class: 'muted small' }, 'Nenhuma. Use o menu ⋯ de uma thread para relacionar.'),
      notesBlock(id, v.notes, refresh)));
}

function notesBlock(about, notes, refresh) {
  const add = async (n) => {
    const title = h('input', { class: 'wide', value: n?.title || '', placeholder: 'Título' });
    const body = h('textarea', { class: 'wide', rows: 8, placeholder: 'Markdown' }, n?.body || '');
    const ok = await modal(n ? 'Editar anotação' : 'Nova anotação', h('div', { class: 'form' }, title, body), [{ label: 'Cancelar', value: false }, n ? { label: 'Excluir', danger: true, value: 'del' } : null, { label: 'Salvar', primary: true, value: true }].filter(Boolean));
    if (ok === 'del') { await rpc('note.delete', { id: n.id }).catch(errorToast); refresh(); return; }
    if (!ok) return;
    await rpc('note.save', { id: n?.id, title: title.value, body: body.value, about }).catch(errorToast);
    refresh();
  };
  return h('div', {},
    h('h3', {}, 'Anotações ', h('button', { class: 'link small', onclick: () => add(null) }, '＋ nova')),
    notes.map((n) => h('div', { class: 'note', onclick: () => add(n) }, h('strong', {}, n.title || '(sem título)'), h('div', { class: 'small', html: md(n.body) }))));
}

export async function renderTask(main, id, state) {
  let v;
  try { v = await rpc('task.get', { id }); } catch (e) { main.innerHTML = ''; mount(main, h('div', { class: 'empty' }, e.message)); return; }
  const refresh = () => renderTask(main, id, state);
  state.refresh = refresh;
  const tk = v.task;
  const projects = await rpc('project.list').catch(() => []);
  const save = (patch) => rpc('task.update', { id, ...patch }).then(() => toast('Salvo.')).catch(errorToast);
  const title = h('input', { class: 'wide title-input', value: tk.title, 'aria-label': 'Título' });
  title.addEventListener('change', () => save({ title: title.value }));
  const desc = h('textarea', { class: 'wide', rows: 4, placeholder: 'Descrição' }, tk.description);
  desc.addEventListener('change', () => save({ description: desc.value }));
  const dueDate = h('input', { type: 'date', value: tk.due ? tk.due.slice(0, 10) : '' });
  const dueTime = h('input', { type: 'time', value: tk.due && tk.due.length > 10 ? tk.due.slice(11, 16) : '' });
  const saveDue = () => save({ due: dueDate.value ? (dueTime.value ? `${dueDate.value}T${dueTime.value}:00-03:00` : dueDate.value) : null });
  dueDate.addEventListener('change', saveDue); dueTime.addEventListener('change', saveDue);
  const prio = h('select', { onchange: (e) => save({ priority: Number(e.target.value) }) }, [1, 2, 3, 4].map((p) => h('option', { value: p, selected: tk.priority === p }, `P${p}`)));
  const proj = h('select', { onchange: (e) => save({ project: e.target.value || null }).then(refresh) }, h('option', { value: '' }, t('Entrada')), projects.map((p) => h('option', { value: p.project.id, selected: tk.project === p.project.id }, p.project.name)));
  const labels = h('input', { value: tk.labels.join(' '), placeholder: 'etiquetas separadas por espaço' });
  labels.addEventListener('change', () => save({ labels: labels.value.split(/\s+/).filter(Boolean) }));
  const isDev = projects.find((p) => p.project.id === tk.project)?.project.type === 'development';
  const delegate = async () => {
    const b = await rpc('dev.briefing', { task: id }).catch(errorToast);
    if (!b) return;
    const name = h('input', { class: 'wide', placeholder: 'nome do agente (opcional)' });
    const ok = await modal('Delegar ao Anthive', h('div', { class: 'form' }, h('p', { class: 'small' }, 'Isto cria um agente de verdade no Anthive com este briefing:'), h('pre', { class: 'briefing' }, b.briefing), name),
      [{ label: 'Cancelar', value: false }, { label: 'Criar agente', primary: true, value: true }]);
    if (!ok) return;
    const r = await rpc('dev.delegate', { task: id, agent: name.value, confirm: true }).catch(errorToast);
    if (r) { toast(`Agente ${r.agent} criado no Anthive.`); refresh(); }
  };
  main.innerHTML = '';
  mount(main, 
    h('header', { class: 'view-head' }, h('a', { href: tk.project ? `#/tasks/p/${tk.project}` : '#/tasks/inbox', class: 'back' }, `← ${tk.project_name || t('Entrada')}`), h('div', { class: 'grow' }),
      isDev ? h('button', { onclick: delegate }, '⌘ Delegar ao Anthive') : null,
      h('button', { onclick: () => relateThread(id, refresh) }, '🔗 Relacionar thread'),
      h('button', { class: 'danger', onclick: async () => { if (await confirmBox('Excluir tarefa', 'A tarefa vai para a lixeira.', t('Excluir'), true)) { await rpc('task.delete', { id }).catch(errorToast); history.back(); } } }, t('Excluir'))),
    h('div', { class: 'thread' },
      h('label', { class: 'check big' }, h('input', { type: 'checkbox', checked: tk.status === 'done', onchange: async (e) => { await rpc('task.complete', { id, done: e.target.checked }).catch(errorToast); refresh(); } }), title),
      h('div', { class: 'form-row' }, h('label', {}, 'Prazo ', dueDate, dueTime), h('label', {}, 'Prioridade ', prio), h('label', {}, 'Projeto ', proj), h('label', {}, 'Etiquetas ', labels)),
      tk.ignored ? h('div', { class: 'notice' }, tk.ignored === 'overdue' ? 'Esta tarefa está vencida e aparece em O que só foi ignorado.' : 'Esta tarefa está parada há dias e aparece em O que só foi ignorado.') : null,
      tk.delegated_to ? h('div', { class: 'notice' }, `Delegada ao agente ${tk.delegated_to} no Anthive.`) : null,
      desc,
      h('h3', {}, 'Subtarefas'),
      h('div', { class: 'task-list' }, v.subtasks.map((r) => taskRow(r, refresh))),
      (() => { const i = h('input', { class: 'wide', placeholder: 'Nova subtarefa' }); i.addEventListener('keydown', async (e) => { if (e.key === 'Enter' && i.value.trim()) { await rpc('task.create', { title: i.value, parent: id }).catch(errorToast); refresh(); } }); return i; })(),
      h('h3', {}, 'Threads relacionadas'),
      v.threads.length ? v.threads.map((x) => (x.kind === 'thread' ? threadCard(x.card, { onChange: refresh }) : x.kind === 'superthread' ? h('a', { class: 'chip st', href: `#/st/${x.id}` }, `⧉ ${x.title}`) : null)) : h('div', { class: 'muted small' }, 'Nenhuma.'),
      notesBlock(id, v.notes, refresh)));
}

/** Relacionar a partir de Tasks (RF-TSK-07): escolhe uma thread ou superthread e liga à tarefa ou projeto. */
async function relateThread(target, refresh) {
  const input = h('input', { type: 'search', class: 'wide', placeholder: 'Buscar thread ou superthread…' });
  const list = h('div', { class: 'pick-list' });
  const search = async () => {
    const hits = (await rpc('pick', { q: input.value }).catch(() => [])).filter((x) => x.kind === 'thread' || x.kind === 'superthread');
    list.innerHTML = '';
    hits.slice(0, 12).forEach((x) => list.append(h('button', { class: 'pick-item', onclick: async () => {
      await rpc('relate', { a: x.id, b: target }).then(() => toast('Relacionado.')).catch(errorToast);
      document.querySelector('#modal-root .backdrop')?.click(); refresh();
    } }, `${x.kind === 'superthread' ? '⧉' : '•'} ${x.title}`)));
  };
  input.addEventListener('input', search);
  search();
  await modal('Relacionar thread', h('div', {}, input, list), [{ label: 'Fechar', value: null }]);
}
