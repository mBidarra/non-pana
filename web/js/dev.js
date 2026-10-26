// Espaço Desenvolvimento (spec 4.8): o Anthive como está, lido por `anthive status --json`.
import { rpc } from './api.js';
import { h, mount, toast, errorToast, ago, t, modal } from './ui.js';

/** Novo projeto do Anthive: o mesmo formulário da tela inicial dele (nome e pasta, criada se faltar). */
async function newProject(refresh) {
  const name = h('input', { type: 'text', class: 'wide', placeholder: t('como você chama o projeto') });
  const dir = h('input', { type: 'text', class: 'wide', placeholder: '~/Documents/<nome>' });
  const body = h('div', { class: 'form' },
    h('label', {}, t('Nome'), name),
    h('label', {}, t('Pasta'), dir),
    h('p', { class: 'muted small' }, t('A pasta é criada se não existir. O projeto aparece no Anthive e em Tasks.')));
  const ok = await modal(t('Novo projeto'), body, [{ label: t('Cancelar'), value: null }, { label: t('Criar'), primary: true, run: () => true }]);
  if (!ok || !name.value.trim()) return;
  try {
    const r = await rpc('dev.create', { name: name.value.trim(), dir: dir.value.trim() || null });
    toast(r.existing ? `${t('Essa pasta já é o projeto')} ${r.project.name}.` : `${r.project.name} ${t('criado em')} ${r.project.cwd}`);
    refresh();
  } catch (e) { errorToast(e); }
}

export async function renderDev(main, state) {
  const refresh = () => renderDev(main, state);
  state.refresh = refresh;
  main.innerHTML = '';
  mount(main, h('header', { class: 'view-head' }, h('h1', {}, t('Desenvolvimento')), h('div', { class: 'grow' }), h('button', { class: 'primary', onclick: () => newProject(refresh) }, `＋ ${t('Novo projeto')}`)), h('div', { class: 'muted' }, 'Lendo o Anthive…'));
  let v;
  try { v = await rpc('dev.status'); } catch (e) {
    main.lastChild.replaceWith(h('div', { class: 'empty' }, `Não consegui ler o Anthive: ${e.message}`));
    return;
  }
  if (v.synced) toast(`${v.synced} projeto(s) do Anthive agora aparecem em Tasks.`);
  const a = v.anthive;
  const proj = (p) => h('article', { class: 'card dev' },
    h('div', { class: 'card-head' }, h('strong', {}, `⌘ ${p.name}`), h('span', { class: 'muted small' }, p.cwd), h('div', { class: 'grow' }),
      p.running ? h('span', { class: 'badge agent-replying' }, `${p.running} rodando`) : null,
      p.needs?.length ? h('span', { class: 'badge agent-failed' }, `${p.needs.length} precisa(m) de você`) : null,
      h('a', { class: 'chip', href: `#/tasks/p/p-anthive-${p.id}` }, 'Tarefas'),
      h('button', { onclick: () => rpc('dev.open', { dir: p.cwd }).then(() => toast('Abrindo no Anthive…')).catch(errorToast) }, 'Abrir no Anthive')),
    p.agents?.length ? h('ul', { class: 'agents' }, p.agents.map((ag) => h('li', {}, h('strong', {}, ag.name), h('span', { class: `badge st-${ag.state}` }, ag.state), h('span', { class: 'muted small' }, ` ${ag.model || ''} · ${ag.branch || ''} · contexto ${Math.round((ag.context || 0) * 100)}%`), ag.doing ? h('div', { class: 'small clamp' }, ag.doing) : null))) : h('div', { class: 'muted small' }, 'Sem agentes.'),
    p.needs?.length ? h('div', { class: 'notice small' }, p.needs.map((n) => h('div', {}, n.text))) : null);
  main.lastChild.replaceWith(h('div', {},
    h('p', { class: 'muted small' }, `${a.running} agente(s) rodando · ${a.needs} pedido(s) esperando você`, a.at ? ` · atualizado ${ago(new Date(a.at).toISOString())} atrás` : '', a.limits ? ` · janela de 5 h em ${Math.round(a.limits.fiveHour * 100)}%` : ''),
    h('div', { class: 'dev-grid' }, (a.projects || []).map(proj)),
    h('button', { onclick: refresh }, 'Atualizar')));
}
