// Componentes pequenos: elementos, markdown seguro, tempo relativo, toast, modal, menu, seletor.
import { t } from './i18n.js';

/** `append` que achata arrays e ignora null/false (Element.append transformaria arrays em texto). */
export function mount(el, ...kids) {
  for (const c of kids.flat(Infinity)) {
    if (c == null || c === false) continue;
    el.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
  return el;
}

export function h(tag, attrs = {}, ...children) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v == null || v === false) continue;
    if (k === 'class') el.className = v;
    else if (k === 'style' && typeof v === 'object') Object.assign(el.style, v);
    else if (k.startsWith('on') && typeof v === 'function') el.addEventListener(k.slice(2).toLowerCase(), v);
    else if (k === 'html') el.innerHTML = v;
    else if (k === 'value') el.value = v;
    else if (v === true) el.setAttribute(k, '');
    else el.setAttribute(k, v);
  }
  for (const c of children.flat(Infinity)) {
    if (c == null || c === false) continue;
    el.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
  return el;
}

const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const safeUrl = (u) => (/^(https?:|mailto:)/i.test(u) ? u : '#');

/** Tira escapes de markdown (`\.` → `.`) que extratores deixam no texto. */
export const unescapeMd = (s) => String(s ?? '').replace(/\\([\\`*_{}\[\]()#+\-.!|>])/g, '$1');

function inline(s) {
  let out = esc(unescapeMd(s));
  out = out.replace(/`([^`]+)`/g, '<code>$1</code>');
  out = out.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
  out = out.replace(/(^|[\s(])\*([^*\s][^*]*)\*/g, '$1<em>$2</em>');
  out = out.replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, (_, txt, url) => `<a href="${esc(safeUrl(url.replace(/&amp;/g, '&')))}" target="_blank" rel="noopener noreferrer">${txt}</a>`);
  out = out.replace(/(^|[\s>])(https?:\/\/[^\s<]+[^\s<.,;:!?)])/g, (_, pre, url) => `${pre}<a href="${url}" target="_blank" rel="noopener noreferrer">${url}</a>`);
  out = out.replace(/(^|\s)@([a-z0-9][\w-]*)/gi, '$1<span class="mention">@$2</span>');
  return out;
}

/** Markdown mínimo e seguro: tudo é escapado antes de virar HTML. */
export function md(src) {
  const lines = String(src ?? '').replace(/\r/g, '').split('\n');
  let html = '', list = null, code = null, para = [];
  const flush = () => { if (para.length) { html += `<p>${para.map(inline).join('<br>')}</p>`; para = []; } };
  const close = () => { if (list) { html += `</${list}>`; list = null; } };
  for (const line of lines) {
    if (code !== null) {
      if (line.startsWith('```')) { html += `<pre><code>${esc(code)}</code></pre>`; code = null; } else code += line + '\n';
      continue;
    }
    if (line.startsWith('```')) { flush(); close(); code = ''; continue; }
    const m = line.match(/^(\s*)([-*]|\d+\.)\s+(.*)$/);
    if (m) {
      flush();
      const kind = /\d/.test(m[2]) ? 'ol' : 'ul';
      if (list !== kind) { close(); html += `<${kind}>`; list = kind; }
      html += `<li>${inline(m[3])}</li>`;
      continue;
    }
    close();
    const hd = line.match(/^(#{1,4})\s+(.*)$/);
    if (hd) { flush(); html += `<h${hd[1].length + 2}>${inline(hd[2])}</h${hd[1].length + 2}>`; continue; }
    if (line.startsWith('>')) { flush(); html += `<blockquote>${inline(line.replace(/^>\s?/, ''))}</blockquote>`; continue; }
    if (!line.trim()) { flush(); continue; }
    para.push(line);
  }
  if (code !== null) html += `<pre><code>${esc(code)}</code></pre>`;
  flush(); close();
  return html;
}

/** Acima disto, a mensagem chega recolhida: um trecho e um acordeão para expandir. */
export const LONG_POST_WORDS = 800;

/** Corpo de uma mensagem em markdown; mensagens longas mostram as primeiras palavras e abrem num acordeão. */
export function postBody(src, cls = 'post-body') {
  const text = String(src ?? '');
  const words = text.match(/\S+/g) || [];
  if (words.length <= LONG_POST_WORDS) return h('div', { class: cls, html: md(text) });
  // o trecho corta no fim da 80ª palavra, mantendo as quebras de linha do original
  const re = /\S+/g;
  let end = 0;
  for (let i = 0; i < 80 && re.exec(text); i++) end = re.lastIndex;
  const preview = h('div', { class: `${cls} long-preview`, html: md(`${text.slice(0, end)} …`) });
  const hint = h('span', { class: 'muted small' }, t('expandir'));
  const full = h('div', { class: cls });
  const box = h('details', { class: 'long-post' },
    h('summary', {}, h('span', { class: 'badge' }, `${words.length.toLocaleString('pt-BR')} ${t('palavras')}`), ' ', hint),
    full);
  box.addEventListener('toggle', () => {
    // o markdown inteiro só é montado na primeira vez que abre
    if (box.open && !full.childNodes.length) full.innerHTML = md(text);
    preview.hidden = box.open;
    hint.textContent = t(box.open ? 'recolher' : 'expandir');
    if (!box.open) wrap.scrollIntoView({ block: 'nearest' });
  });
  const wrap = h('div', { class: 'long' }, preview, box);
  return wrap;
}

export function ago(ts) {
  const d = (Date.now() - new Date(ts).getTime()) / 1000;
  if (d < 60) return 'agora';
  if (d < 3600) return `${Math.floor(d / 60)} min`;
  if (d < 86400) return `${Math.floor(d / 3600)} h`;
  return `${Math.floor(d / 86400)} d`;
}

export function fmtDate(s) {
  if (!s) return '';
  const d = new Date(s.length <= 10 ? s + 'T12:00:00-03:00' : s);
  const opts = { day: '2-digit', month: 'short' };
  if (s.length > 10) Object.assign(opts, { hour: '2-digit', minute: '2-digit' });
  return d.toLocaleString('pt-BR', opts);
}

export function toast(msg, action) {
  const root = document.getElementById('toasts');
  const el = h('div', { class: 'toast', role: 'status' }, h('span', {}, msg));
  if (action) el.append(h('button', { class: 'link', onclick: () => { action.run(); el.remove(); } }, action.label));
  root.append(el);
  setTimeout(() => el.remove(), action ? 8000 : 3500);
}

export function errorToast(e) { toast(`⚠︎ ${e.message || e}`); }

export function modal(title, body, actions = []) {
  return new Promise((resolve) => {
    const root = document.getElementById('modal-root');
    const close = (v) => { root.innerHTML = ''; root.hidden = true; document.removeEventListener('keydown', onKey); resolve(v); };
    const onKey = (e) => { if (e.key === 'Escape') close(null); };
    document.addEventListener('keydown', onKey);
    const box = h('div', { class: 'modal', role: 'dialog', 'aria-modal': 'true', 'aria-label': title },
      h('h2', {}, title), body,
      h('div', { class: 'modal-actions' }, ...actions.map((a) => h('button', { class: a.primary ? 'primary' : a.danger ? 'danger' : '', onclick: async () => close(a.value !== undefined ? a.value : await (a.run?.() ?? true)) }, a.label))));
    root.innerHTML = '';
    root.append(h('div', { class: 'backdrop', onclick: () => close(null) }), box);
    root.hidden = false;
    box.querySelector('input,textarea,select,button.primary')?.focus();
  });
}

export async function confirmBox(title, text, label = 'Confirmar', danger = false) {
  return (await modal(title, h('p', {}, text), [{ label: 'Cancelar', value: false }, { label, value: true, primary: !danger, danger }])) === true;
}

export async function prompt(title, value = '', placeholder = '') {
  const input = h('input', { type: 'text', value, placeholder, class: 'wide' });
  const v = await modal(title, input, [{ label: 'Cancelar', value: null }, { label: 'OK', primary: true, run: () => input.value }]);
  return v;
}

/** Menu ⋯ (popover) ancorado num botão. Navegável por setas e Enter. */
export function menu(anchor, items) {
  document.querySelectorAll('.popover').forEach((p) => p.remove());
  const pop = h('div', { class: 'popover', role: 'menu' },
    items.filter(Boolean).map((it) => h('button', { role: 'menuitem', class: it.danger ? 'danger' : '', onclick: () => { pop.remove(); it.run(); } }, it.label)));
  document.body.append(pop);
  const r = anchor.getBoundingClientRect();
  pop.style.top = `${Math.min(window.innerHeight - pop.offsetHeight - 8, r.bottom + 4)}px`;
  pop.style.left = `${Math.max(8, Math.min(window.innerWidth - pop.offsetWidth - 8, r.right - pop.offsetWidth))}px`;
  const buttons = [...pop.querySelectorAll('button')];
  buttons[0]?.focus();
  const onKey = (e) => {
    const i = buttons.indexOf(document.activeElement);
    if (e.key === 'ArrowDown') { buttons[(i + 1) % buttons.length].focus(); e.preventDefault(); }
    if (e.key === 'ArrowUp') { buttons[(i - 1 + buttons.length) % buttons.length].focus(); e.preventDefault(); }
    if (e.key === 'Escape') { pop.remove(); anchor.focus(); }
  };
  pop.addEventListener('keydown', onKey);
  setTimeout(() => document.addEventListener('click', function off(e) { if (!pop.contains(e.target)) { pop.remove(); document.removeEventListener('click', off); } }), 0);
}

export const KIND = { thinking: '💭', reading: '📖', watching: '👀' };
export const KIND_LABEL = { thinking: 'pensando', reading: 'lendo', watching: 'vendo' };
export const kindOf = (id) => (id.startsWith('st-') ? 'superthread' : id.startsWith('t-') ? 'thread' : id.startsWith('k-') ? 'tarefa' : id.startsWith('p-') ? 'projeto' : id.startsWith('n-') ? 'anotação' : '');
export { t };
