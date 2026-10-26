// Anthive Novo: roteador, barra lateral, atalhos de teclado (RNF-18), paleta ⌘K (RF-BUS-04) e eventos ao vivo.
import { rpc, events, token } from './api.js';
import { h, mount, toast, errorToast, KIND, t } from './ui.js';
import { renderFeed, renderThread, renderSuperthread, loadProfiles, openPicker, threadMenu, openChat, composer } from './feed.js';
import { renderTasks, renderProject, renderTask } from './tasks.js';
import { renderObs } from './obs.js';
import { renderDev } from './dev.js';
import { renderSettings, renderJournal, renderTrash } from './settings.js';

const main = document.getElementById('main');
const state = { feed: {}, tasks: {}, obs: {}, dev: {}, view: {} };
let current = { name: '', st: {} };

const NAV = [
  ['#/feed', '🌐', 'Rede Social'], ['#/tasks/today', '☑', 'Tasks'], ['#/obs', '👁', 'Observabilidade'], ['#/dev', '⌘', 'Desenvolvimento'],
];

function sidebar() {
  const nav = document.getElementById('nav');
  nav.innerHTML = '';
  let hash = location.hash || '#/feed';
  if (hash.startsWith('#/t/') || hash.startsWith('#/st/')) hash = '#/feed';
  mount(nav, 
    h('div', { class: 'brand' }, h('span', { class: 'logo' }, '⬡'), h('span', {}, 'Anthive ', h('strong', {}, 'Novo'))),
    NAV.map(([href, icon, label]) => h('a', { href, class: hash.startsWith(href.split('/').slice(0, 2).join('/')) ? 'on' : '' }, h('span', { class: 'ico' }, icon), t(label))),
    h('div', { class: 'grow' }),
    h('button', { class: 'nav-btn', onclick: palette, title: '⌘K' }, h('span', { class: 'ico' }, '⌕'), t('Buscar'), h('kbd', {}, '⌘K')),
    h('a', { href: '#/journal', class: hash.startsWith('#/journal') ? 'on' : '' }, h('span', { class: 'ico' }, '✦'), t('Diário do agente')),
    h('a', { href: '#/trash', class: hash.startsWith('#/trash') ? 'on' : '' }, h('span', { class: 'ico' }, '🗑'), t('Lixeira')),
    h('a', { href: '#/settings', class: hash.startsWith('#/settings') ? 'on' : '' }, h('span', { class: 'ico' }, '⚙'), t('Configurações')));
}

/** Compositor rápido (janela do atalho ⌘⇧N): só o compositor, com o link do clipboard. */
function renderCompose() {
  document.body.classList.add('compact');
  const q = new URLSearchParams((location.hash.split('?')[1] || ''));
  const done = () => { location.href = '/__close'; };
  const comp = composer(() => { mount(main, h('div', { class: 'empty' }, 'Publicada ✓')); setTimeout(done, 700); }, q.get('text') ? `${q.get('text')}\n` : '');
  main.innerHTML = '';
  mount(main, comp);
  document.addEventListener('keydown', (e) => { if (e.key === 'Escape') done(); });
  comp.focusText();
}

async function route() {
  if ((location.hash || '').startsWith('#/compose')) {
    if (token()) { await loadProfiles(); renderCompose(); }
    return;
  }
  document.body.classList.remove('compact');
  if (!token()) { main.innerHTML = ''; mount(main, h('div', { class: 'empty' }, h('h2', {}, 'Abra o Novo pelo app ou por `novo open`'), h('p', {}, 'Esta página precisa do token local do novod.'))); return; }
  sidebar();
  document.querySelectorAll('.popover').forEach((p) => p.remove());
  const parts = (location.hash || '#/feed').slice(2).split('/');
  const [a, b, c] = parts;
  const st = (state.view[location.hash] ??= {});
  current = { name: a, st };
  try {
    if (a === 'feed' || !a) await renderFeed(main, state.feed), (current.st = state.feed);
    else if (a === 't') await renderThread(main, b, st);
    else if (a === 'st') await renderSuperthread(main, b, st);
    else if (a === 'tasks' && b === 'p') await renderProject(main, c, st);
    else if (a === 'tasks' && b === 'k') await renderTask(main, c, st);
    else if (a === 'tasks') await renderTasks(main, b || 'today', (current.st = state.tasks[b] ??= {}));
    else if (a === 'obs') await renderObs(main, (current.st = state.obs));
    else if (a === 'dev') await renderDev(main, (current.st = state.dev));
    else if (a === 'settings') await renderSettings(main, st);
    else if (a === 'journal') await renderJournal(main, st);
    else if (a === 'trash') await renderTrash(main, st);
    else location.hash = '#/feed';
  } catch (e) {
    console.error(e);
    if (e.message === 'auth') { try { localStorage.removeItem('novo.token'); } catch {} location.reload(); return; }
    errorToast(e);
  }
  main.focus({ preventScroll: true });
}

// ------------------------------------------------------------------ paleta ⌘K

async function palette() {
  const root = document.getElementById('modal-root');
  const input = h('input', { type: 'search', class: 'wide', placeholder: 'Buscar threads, tarefas, projetos, anotações… ou digite um comando', 'aria-label': 'Buscar' });
  const list = h('div', { class: 'pick-list', role: 'listbox' });
  const close = () => { root.innerHTML = ''; root.hidden = true; };
  const actions = [
    ['＋ Publicar uma thread', () => { location.hash = '#/feed'; setTimeout(() => state.feed.composer?.focusText(), 200); }],
    ['☑ Nova tarefa', () => { location.hash = '#/tasks/inbox'; setTimeout(() => document.querySelector('.task-list')?.previousElementSibling?.focus(), 200); }],
    ['👁 Rodar Sessão Foco de hoje', async () => { await rpc('focus.run', { day: new Date(Date.now() - 3 * 3600e3).toISOString().slice(0, 10) }).catch(errorToast); location.hash = '#/obs'; }],
  ];
  let items = [];
  const draw = async () => {
    const q = input.value.trim();
    list.innerHTML = '';
    items = [];
    if (q) {
      const hits = await rpc('search', { q, filters: { include_private: true }, limit: 20 }).catch(() => []);
      for (const x of hits) {
        const href = x.kind === 'thread' ? `#/t/${x.id}` : x.kind === 'superthread' ? `#/st/${x.id}` : x.kind === 'task' ? `#/tasks/k/${x.id}` : x.kind === 'project' ? `#/tasks/p/${x.id}` : x.kind === 'note' ? '#/tasks/inbox' : '#/feed';
        items.push([`${{ thread: '•', superthread: '⧉', task: '☐', project: '▣', note: '✎' }[x.kind] || ''} ${x.title}`, () => (location.hash = href), x.snippet]);
      }
    }
    for (const [label, run] of actions) if (!q || label.toLowerCase().includes(q.toLowerCase())) items.push([label, run]);
    items.forEach(([label, run, sub], i) => list.append(h('button', { class: `pick-item ${i === 0 ? 'focus' : ''}`, role: 'option', onclick: () => { close(); run(); } }, h('div', {}, label), sub ? h('div', { class: 'muted small clamp' }, sub) : null)));
  };
  let timer;
  input.addEventListener('input', () => { clearTimeout(timer); timer = setTimeout(draw, 120); });
  input.addEventListener('keydown', (e) => {
    const btns = [...list.querySelectorAll('button')];
    let i = btns.findIndex((b) => b.classList.contains('focus'));
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      btns[i]?.classList.remove('focus');
      i = (i + (e.key === 'ArrowDown' ? 1 : -1) + btns.length) % btns.length;
      btns[i]?.classList.add('focus'); btns[i]?.scrollIntoView({ block: 'nearest' });
    }
    if (e.key === 'Enter') { e.preventDefault(); btns[Math.max(0, i)]?.click(); }
    if (e.key === 'Escape') close();
  });
  root.innerHTML = '';
  root.append(h('div', { class: 'backdrop', onclick: close }), h('div', { class: 'modal palette', role: 'dialog', 'aria-label': 'Buscar' }, input, list));
  root.hidden = false;
  input.focus();
  draw();
}

// ------------------------------------------------------------------ teclado (RNF-18)

function selectable() {
  return [...main.querySelectorAll('.feed .card, .task-list .task')];
}

document.addEventListener('keydown', (e) => {
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') { e.preventDefault(); palette(); return; }
  const tag = (document.activeElement?.tagName || '').toLowerCase();
  if (['input', 'textarea', 'select'].includes(tag) || !document.getElementById('modal-root').hidden || e.metaKey || e.ctrlKey || e.altKey) return;
  const items = selectable();
  const cur = items.findIndex((x) => x === document.activeElement || x.classList.contains('selected'));
  const focus = (i) => { items.forEach((x) => x.classList.remove('selected')); const el = items[Math.max(0, Math.min(items.length - 1, i))]; if (el) { el.classList.add('selected'); el.focus(); el.scrollIntoView({ block: 'nearest' }); } };
  const sel = items[cur];
  switch (e.key) {
    case 'j': focus(cur + 1); break;
    case 'k': focus(cur - 1); break;
    case 'Enter': if (sel && document.activeElement === sel) sel.click(); break;
    case 'l': if (sel?.dataset.id?.startsWith('t-')) sel.querySelector('.li')?.click(); break;
    case 'x': case '.': if (sel) sel.querySelector('button.icon')?.click(); break;
    case 'r': if (sel?.dataset.id) openPicker(sel.dataset.id, () => current.st.refresh?.()); break;
    case 'c': if (sel?.dataset.id?.startsWith('t-')) openChat(sel.dataset.id); break;
    case 'n': e.preventDefault(); if (location.hash.startsWith('#/tasks')) main.querySelector('input.wide')?.focus(); else { location.hash = '#/feed'; setTimeout(() => state.feed.composer?.focusText(), 100); } break;
    case ' ': if (sel?.classList.contains('task')) { e.preventDefault(); sel.querySelector('input[type=checkbox]')?.click(); } break;
    case '?': toast('Atalhos: j/k mover · Enter abrir · l Li · x menu ⋯ · r relacionar · c chat · n novo · espaço concluir tarefa · ⌘K buscar'); break;
    case 'g': break;
    case '1': location.hash = '#/feed'; break;
    case '2': location.hash = '#/tasks/today'; break;
    case '3': location.hash = '#/obs'; break;
    case '4': location.hash = '#/dev'; break;
    default: return;
  }
});

// ------------------------------------------------------------------ eventos ao vivo

let pending = null;
events((ev) => {
  if (['queue', 'journal'].includes(ev.type) && current.name !== 'journal') return;
  clearTimeout(pending);
  pending = setTimeout(() => {
    const typing = ['input', 'textarea'].includes((document.activeElement?.tagName || '').toLowerCase()) && document.activeElement.value;
    const busy = !document.getElementById('modal-root').hidden || document.querySelector('.popover') || (document.getElementById('drawer') && !document.getElementById('drawer').hidden && document.activeElement?.closest('#drawer'));
    if (!typing && !busy) current.st.refresh?.();
  }, 400);
});

window.addEventListener('hashchange', route);
if (!(location.hash || '').startsWith('#/compose')) loadProfiles().finally(route); else route();
