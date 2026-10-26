// Espaço Observabilidade (spec 4.7): "O que só foi ignorado" (MVP), série diária, e os tracks de depois.
import { rpc } from './api.js';
import { h, mount, toast, errorToast, confirmBox, fmtDate, ago, KIND, t } from './ui.js';
import { openPicker } from './feed.js';

const NS = 'http://www.w3.org/2000/svg';
const s = (tag, attrs = {}, ...kids) => { const el = document.createElementNS(NS, tag); for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, v); kids.forEach((c) => el.append(c)); return el; };

/** Colunas empilhadas: threads (série 1) e tarefas (série 2) ignoradas por dia (RF-OBS-05). */
function chart(series) {
  const wrap = h('div', { class: 'viz-root' });
  if (!series.length) { mount(wrap, h('p', { class: 'muted small' }, 'O gráfico aparece depois da primeira Sessão Foco.')); return wrap; }
  const W = 640, H = 180, L = 28, B = 22, T = 10;
  const max = Math.max(1, ...series.map((d) => d.ignored_threads + d.ignored_tasks));
  const step = (W - L) / series.length;
  const bw = Math.min(24, step * 0.6);
  const y = (v) => H - B - (v / max) * (H - B - T);
  const svg = s('svg', { viewBox: `0 0 ${W} ${H}`, role: 'img', 'aria-label': `Itens ignorados por dia, ${series.length} dias` });
  const ticks = [0, Math.ceil(max / 2), max].filter((v, i, a) => a.indexOf(v) === i);
  ticks.forEach((v) => { svg.append(s('line', { x1: L, x2: W, y1: y(v), y2: y(v), class: 'grid' })); const tx = s('text', { x: L - 6, y: y(v) + 4, class: 'axis', 'text-anchor': 'end' }); tx.textContent = v; svg.append(tx); });
  const tip = h('div', { class: 'tip', hidden: true });
  // barra com ponta arredondada de 4px, quadrada na base
  const col = (x, y0, y1, cls, roundTop) => {
    const hgt = y0 - y1;
    if (hgt <= 0) return null;
    const r = roundTop ? Math.min(4, hgt, bw / 2) : 0;
    const d = `M${x},${y0} V${y1 + r} Q${x},${y1} ${x + r},${y1} H${x + bw - r} Q${x + bw},${y1} ${x + bw},${y1 + r} V${y0} Z`;
    return s('path', { d, class: cls });
  };
  series.forEach((d, i) => {
    const x = L + i * step + (step - bw) / 2;
    const base = y(0);
    const yt = y(d.ignored_threads);
    const yk = y(d.ignored_threads + d.ignored_tasks);
    const g = s('g', { class: 'col' });
    const tasksOnTop = d.ignored_tasks > 0;
    const p1 = col(x, base, yt, 'series-1', !tasksOnTop);
    const p2 = col(x, yt - (d.ignored_threads > 0 && tasksOnTop ? 2 : 0), yk, 'series-2', true);
    if (p1) g.append(p1);
    if (p2) g.append(p2);
    const hit = s('rect', { x: L + i * step, y: T, width: step, height: H - B - T, class: 'hit' });
    hit.addEventListener('mouseenter', () => {
      tip.hidden = false;
      tip.innerHTML = '';
      tip.append(h('strong', {}, fmtDate(d.date)), h('div', {}, h('span', { class: 'sw s1' }), ` Threads ignoradas: ${d.ignored_threads}`), h('div', {}, h('span', { class: 'sw s2' }), ` Tarefas ignoradas: ${d.ignored_tasks}`), d.fomo_7d != null ? h('div', { class: 'muted' }, `FOMO 7d: ${d.fomo_7d}%`) : null);
      const r = svg.getBoundingClientRect();
      tip.style.left = `${((x + bw) / W) * r.width + 8}px`;
      tip.style.top = `${(yk / H) * r.height}px`;
    });
    hit.addEventListener('mouseleave', () => { tip.hidden = true; });
    g.append(hit);
    svg.append(g);
    if (series.length <= 14 || i % Math.ceil(series.length / 10) === 0) { const tx = s('text', { x: x + bw / 2, y: H - 6, class: 'axis', 'text-anchor': 'middle' }); tx.textContent = d.date.slice(8, 10) + '/' + d.date.slice(5, 7); svg.append(tx); }
  });
  const last = series[series.length - 1];
  const table = h('table', { class: 'data', hidden: true }, h('thead', {}, h('tr', {}, h('th', {}, 'Dia'), h('th', {}, 'Threads ignoradas'), h('th', {}, 'Tarefas ignoradas'), h('th', {}, 'FOMO 7d'))),
    h('tbody', {}, series.map((d) => h('tr', {}, h('td', {}, d.date), h('td', {}, d.ignored_threads), h('td', {}, d.ignored_tasks), h('td', {}, d.fomo_7d ?? '—')))));
  mount(wrap, 
    h('div', { class: 'legend' }, h('span', {}, h('span', { class: 'sw s1' }), ` Threads (${last.ignored_threads} no último dia)`), h('span', {}, h('span', { class: 'sw s2' }), ` Tarefas (${last.ignored_tasks} no último dia)`),
      h('div', { class: 'grow' }), h('button', { class: 'link small', onclick: (e) => { table.hidden = !table.hidden; e.target.textContent = table.hidden ? 'ver tabela' : 'ver gráfico'; svg.style.display = table.hidden ? '' : 'none'; } }, 'ver tabela')),
    h('div', { class: 'chart' }, svg, tip), table);
  return wrap;
}

function ignoredThreadRow(x, refresh) {
  const c = x.card;
  return h('div', { class: 'ign' },
    h('span', { class: 'kind' }, KIND[c.kind]),
    h('div', { class: 'grow' },
      h('a', { href: `#/t/${c.id}` }, c.title),
      h('div', { class: 'meta small' }, h('span', { class: 'warn' }, `${c.age_hours} h sem Li`), c.material?.[0]?.site ? h('span', { class: 'muted' }, c.material[0].site) : null, c.superthread_title ? h('span', { class: 'muted' }, `⧉ ${c.superthread_title}`) : null),
      x.summary ? h('div', { class: 'small' }, x.summary) : null),
    h('div', { class: 'actions' },
      h('button', { onclick: async () => { await rpc('thread.read', { id: c.id, read: true }).catch(errorToast); refresh(); } }, '✓ Li'),
      h('button', { onclick: () => openPicker(c.id, refresh) }, '🔗 Relacionar'),
      h('button', { class: 'danger', onclick: async () => { if (await confirmBox('Excluir thread', 'Vai para a lixeira por 30 dias.', 'Excluir', true)) { await rpc('thread.delete', { id: c.id }).catch(errorToast); toast('Excluída.', { label: 'Desfazer', run: async () => { await rpc('thread.restore', { id: c.id }); refresh(); } }); refresh(); } } }, 'Excluir')));
}

function ignoredTaskRow(k, refresh) {
  const date = h('input', { type: 'date', 'aria-label': 'Nova data', onchange: async (e) => { await rpc('task.update', { id: k.id, due: e.target.value || null }).catch(errorToast); toast('Reagendada.'); refresh(); } });
  return h('div', { class: 'ign' },
    h('span', { class: 'kind' }, '☐'),
    h('div', { class: 'grow' }, h('a', { href: `#/tasks/k/${k.id}` }, k.title),
      h('div', { class: 'meta small' }, h('span', { class: 'warn' }, k.reason === 'overdue' ? `vencida há ${k.days} dia(s)` : `parada há ${k.days} dia(s)`), k.due ? h('span', { class: 'muted' }, `prazo ${fmtDate(k.due)}`) : null)),
    h('div', { class: 'actions' },
      h('button', { onclick: async () => { await rpc('task.complete', { id: k.id, done: true }).catch(errorToast); refresh(); } }, '✓ Concluir'),
      h('label', { class: 'small' }, 'Reagendar ', date),
      h('button', { class: 'danger', onclick: async () => { if (await confirmBox('Excluir tarefa', 'Vai para a lixeira.', 'Excluir', true)) { await rpc('task.delete', { id: k.id }).catch(errorToast); refresh(); } } }, 'Excluir')));
}

export async function renderObs(main, state) {
  state.tab ??= 'ignored';
  const refresh = () => renderObs(main, state);
  state.refresh = refresh;
  let ign, ser;
  try { [ign, ser] = await Promise.all([rpc('obs.ignored'), rpc('obs.series', { days: 60 })]); } catch (e) { errorToast(e); return; }
  const latest = ser.latest;
  const tabs = [['ignored', 'O que só foi ignorado'], ['read', 'O que eu tenho mais lido'], ['fomo', '% de FOMO']];
  const body = h('div', {});
  if (state.tab === 'ignored') {
    mount(body, 
      h('p', { class: 'muted small' }, `Thread ignorada: ${ign.rules.only_with_material ? 'com material, ' : ''}sem Li há mais de ${ign.rules.thread_hours} h. Tarefa ignorada: vencida, ou sem prazo e parada há ${ign.rules.task_days} dias. Cada item sai da lista assim que você age.`),
      h('h3', {}, `Threads (${ign.threads.length})`),
      ign.threads.length ? ign.threads.map((x) => ignoredThreadRow(x, refresh)) : h('div', { class: 'empty small' }, 'Nenhuma thread esquecida. 🎉'),
      h('h3', {}, `Tarefas (${ign.tasks.length})`),
      ign.tasks.length ? ign.tasks.map((k) => ignoredTaskRow(k, refresh)) : h('div', { class: 'empty small' }, 'Nenhuma tarefa esquecida.'),
      h('h3', {}, 'Ignorados por dia'),
      chart(ser.series));
  } else if (state.tab === 'read') {
    const mr = latest?.most_read || { sites: [], entities: [] };
    mount(body, h('p', { class: 'muted small' }, 'Prévia (fase 3): temas e fontes das threads marcadas Li nos últimos 7 dias, do último relatório.'),
      h('div', { class: 'two' },
        h('div', {}, h('h3', {}, 'Fontes'), mr.sites.length ? h('ol', {}, mr.sites.map(([s, n]) => h('li', {}, `${s} — ${n}`))) : h('div', { class: 'muted small' }, 'Ainda sem leituras marcadas.')),
        h('div', {}, h('h3', {}, 'Temas'), mr.entities.length ? h('ol', {}, mr.entities.map(([s, n]) => h('li', {}, `#${s} — ${n}`))) : h('div', { class: 'muted small' }, 'Ainda sem temas.'))),
      latest?.themes?.length ? h('p', {}, h('strong', {}, 'Temas do último dia: '), latest.themes.map((x) => `#${x}`).join(' ')) : null);
  } else {
    const f = latest?.fomo_7d;
    mount(body, h('p', { class: 'muted small' }, 'Prévia (fase 3). Fórmula da especificação 4.7: 100 × (threads ignoradas + tarefas ignoradas) ÷ (threads com material + tarefas ativas), nos 7 dias. Hipótese a calibrar.'),
      h('div', { class: 'hero' }, h('div', { class: 'hero-num' }, f == null ? '—' : `${f}%`), h('div', { class: 'muted' }, latest ? `no relatório de ${fmtDate(latest.date)}` : 'sem relatório ainda')));
  }
  main.innerHTML = '';
  mount(main, 
    h('header', { class: 'view-head' }, h('h1', {}, t('Observabilidade')), h('div', { class: 'grow' }),
      h('button', { onclick: async () => { toast('Rodando a Sessão Foco de hoje…'); const day = new Date(Date.now() - 3 * 3600e3).toISOString().slice(0, 10); await rpc('focus.run', { day }).catch(errorToast); refresh(); } }, 'Rodar Sessão Foco agora')),
    latest ? h('div', { class: 'focus-card' },
      h('div', { class: 'muted small' }, `Sessão Foco de ${fmtDate(latest.date)} · gerada ${ago(latest.generated_at)} atrás${latest.agent ? ` · ${latest.agent}` : ''}`),
      latest.summary ? h('p', {}, latest.summary) : null,
      latest.themes?.length ? h('div', { class: 'chips' }, latest.themes.map((x) => h('span', { class: 'chip' }, `#${x}`))) : null) : h('div', { class: 'focus-card muted small' }, 'A primeira Sessão Foco roda à meia-noite (GMT-3).'),
    h('div', { class: 'tabs', role: 'tablist' }, tabs.map(([k, label]) => h('button', { role: 'tab', 'aria-selected': state.tab === k ? 'true' : 'false', class: state.tab === k ? 'on' : '', onclick: () => { state.tab = k; refresh(); } }, t(label)))),
    body);
}
