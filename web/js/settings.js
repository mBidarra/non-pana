// Configurações: perfis de agente e chaves (spec 5.5), preferências, diário dos agentes (RF-AGT-11) e lixeira.
import { rpc } from './api.js';
import { h, mount, toast, errorToast, modal, fmtDate, t } from './ui.js';
import { getLang, setLang } from './i18n.js';

const ENGINES = { claude_code: 'Claude Code (claude -p)', openrouter_http: 'OpenRouter direto', anthropic_api: 'API da Anthropic', ollama: 'Local (Ollama)' };
const AUTH = { claude_code: ['subscription', 'openrouter', 'anthropic_api'], openrouter_http: ['openrouter'], anthropic_api: ['anthropic_api'], ollama: ['local'] };

async function editProfile(p, all, refresh) {
  const isNew = !p;
  p = p || { name: '', engine: 'openrouter_http', model: '', auth: 'openrouter', key: null, default: false };
  const f = {
    name: h('input', { value: p.name, placeholder: 'ex.: gpt-via-openrouter' }),
    label: h('input', { value: p.label || '', placeholder: 'nome amigável (opcional)' }),
    engine: h('select', {}, Object.entries(ENGINES).map(([k, v]) => h('option', { value: k, selected: p.engine === k }, v))),
    auth: h('select', {}),
    model: h('input', { value: p.model, placeholder: 'haiku, sonnet, openai/<modelo>, llama3…' }),
    key: h('input', { value: p.key || '', placeholder: 'keychain:novo-openrouter' }),
    secret: h('input', { type: 'password', placeholder: 'cole a chave para guardar no Keychain (opcional)', autocomplete: 'off' }),
    base: h('input', { value: p.base_url || '', placeholder: 'base da API (opcional)' }),
    usd: h('input', { type: 'number', step: '0.01', value: p.daily_budget_usd ?? '' }),
    turns: h('input', { type: 'number', value: p.daily_budget_turns ?? '' }),
    def: h('input', { type: 'checkbox', checked: !!p.default }),
  };
  const fillAuth = () => { f.auth.innerHTML = ''; AUTH[f.engine.value].forEach((a) => f.auth.append(h('option', { value: a, selected: p.auth === a }, a))); };
  f.engine.addEventListener('change', fillAuth); fillAuth();
  const ok = await modal(isNew ? 'Novo perfil de agente' : `Perfil ${p.name}`, h('div', { class: 'form' },
    h('label', {}, 'Nome', f.name), h('label', {}, 'Rótulo', f.label), h('label', {}, 'Motor', f.engine), h('label', {}, 'Autenticação', f.auth), h('label', {}, 'Modelo', f.model),
    h('label', {}, 'Referência da chave', f.key), h('label', {}, 'Chave (vai só para o Keychain)', f.secret), h('label', {}, 'Base URL', f.base),
    h('div', { class: 'form-row' }, h('label', {}, 'Orçamento diário (US$)', f.usd), h('label', {}, 'Chamadas por dia', f.turns)),
    h('label', { class: 'check' }, f.def, ' Perfil padrão'),
    h('p', { class: 'muted small' }, 'Modelos de outros fornecedores usam o motor OpenRouter direto; pelo Claude Code o OpenRouter avisa que só modelos da Anthropic funcionam bem.')),
  [{ label: 'Cancelar', value: false }, { label: 'Salvar', primary: true, value: true }]);
  if (!ok) return;
  try {
    let key = f.key.value.trim() || null;
    if (f.secret.value.trim()) {
      const account = (key && key.startsWith('keychain:')) ? key.slice(9) : `novo-${f.name.value.trim()}`;
      const r = await rpc('profile.set_key', { account, secret: f.secret.value.trim() });
      key = r.key;
    }
    const np = { name: f.name.value.trim(), label: f.label.value.trim() || null, engine: f.engine.value, auth: f.auth.value, model: f.model.value.trim(), key, base_url: f.base.value.trim() || null,
      daily_budget_usd: f.usd.value ? Number(f.usd.value) : null, daily_budget_turns: f.turns.value ? Number(f.turns.value) : null, default: f.def.checked };
    let list = all.filter((x) => x.name !== p.name);
    if (np.default) list = list.map((x) => ({ ...x, default: false }));
    list.push(np);
    await rpc('profile.save', { profiles: list });
    toast('Perfil salvo.');
    refresh();
  } catch (e) { errorToast(e); }
}

export async function renderSettings(main, state) {
  const refresh = () => renderSettings(main, state);
  state.refresh = refresh;
  const [profiles, cfg] = await Promise.all([rpc('profile.list'), rpc('config.get')]).catch((e) => { errorToast(e); return [[], {}]; });
  const all = profiles.map((x) => x.profile);
  const num = (path, label, min = 0) => {
    const [a, b] = path.split('.');
    const i = h('input', { type: 'number', min, value: b ? cfg[a][b] : cfg[a] });
    i.addEventListener('change', async () => { if (b) cfg[a][b] = Number(i.value); else cfg[a] = Number(i.value); await rpc('config.set', cfg).then(() => toast('Salvo.')).catch(errorToast); });
    return h('label', {}, label, ' ', i);
  };
  const chk = (path, label) => {
    const [a, b] = path.split('.');
    const i = h('input', { type: 'checkbox', checked: !!cfg[a][b] });
    i.addEventListener('change', async () => { cfg[a][b] = i.checked; await rpc('config.set', cfg).then(() => toast('Salvo.')).catch(errorToast); });
    return h('label', { class: 'check' }, i, ` ${label}`);
  };
  main.innerHTML = '';
  mount(main, 
    h('header', { class: 'view-head' }, h('h1', {}, t('Configurações'))),
    h('section', {},
      h('h3', {}, 'Perfis de agente ', h('button', { class: 'link small', onclick: () => editProfile(null, all, refresh) }, '＋ novo')),
      h('div', { class: 'profiles' }, profiles.map(({ profile: p, key_ok }) => h('div', { class: 'profile' },
        h('div', { class: 'grow' }, h('strong', {}, p.label || p.name), p.default ? h('span', { class: 'badge' }, 'padrão') : null,
          h('div', { class: 'muted small' }, `${ENGINES[p.engine]} · ${p.model} · ${p.auth}${p.key ? ` · ${p.key}` : ''}${key_ok ? '' : ' · ⚠︎ chave ausente'}`)),
        h('button', { onclick: async () => { toast('Testando…'); const r = await rpc('profile.test', { name: p.name }).catch(errorToast); if (r) toast(`OK: ${r.model} em ${r.duration_ms} ms`); } }, 'Testar'),
        h('button', { onclick: () => editProfile(p, all, refresh) }, 'Editar'))))),
    h('section', {},
      h('h3', {}, 'Observabilidade e Foco'),
      h('div', { class: 'form' }, num('ignored.thread_hours', 'Horas sem Li para uma thread virar ignorada'), num('ignored.task_days', 'Dias parada para uma tarefa sem prazo virar ignorada'),
        chk('ignored.only_with_material', 'Só threads com material contam para o Li'), chk('focus.enabled', 'Sessão Foco diária às 00:00 GMT-3'),
        chk('notifications.task_due', 'Avisar no prazo das tarefas'), chk('notifications.focus_morning', 'Avisar quando a Sessão Foco terminar'))),
    h('section', {},
      h('h3', {}, 'Idioma'),
      h('select', { onchange: (e) => { setLang(e.target.value); location.reload(); } }, h('option', { value: 'pt-BR', selected: getLang() === 'pt-BR' }, 'Português (Brasil)'), h('option', { value: 'en', selected: getLang() === 'en' }, 'English'))),
    h('section', {},
      h('h3', {}, 'Prompts dos agentes'),
      h('button', { onclick: async () => { const r = await rpc('prompts.export').catch(errorToast); if (r) toast(`Prompts copiados para ${r.dir}; edite à vontade.`); } }, 'Copiar prompts padrão para editar')));
}

export async function renderJournal(main, state) {
  state.day ??= new Date(Date.now() - 3 * 3600e3).toISOString().slice(0, 10);
  const refresh = () => renderJournal(main, state);
  state.refresh = refresh;
  const runs = await rpc('journal', { day: state.day }).catch((e) => { errorToast(e); return []; });
  const total = runs.reduce((a, r) => a + (r.cost_usd || 0), 0);
  const day = h('input', { type: 'date', value: state.day, onchange: (e) => { state.day = e.target.value; refresh(); } });
  main.innerHTML = '';
  mount(main, 
    h('header', { class: 'view-head' }, h('h1', {}, t('Diário do agente')), h('div', { class: 'grow' }), day),
    h('p', { class: 'muted small' }, `${runs.length} chamadas · US$ ${total.toFixed(4)} (custo de lista; na assinatura não há cobrança por chamada). Cada linha mostra o que foi enviado: tamanho do contexto e caixas usadas.`),
    h('table', { class: 'data' },
      h('thead', {}, h('tr', {}, ['Hora', 'Para quê', 'Perfil', 'Modelo', 'Tokens (entrada/saída)', 'Enviado', 'Caixas usadas', 'Duração', 'Resultado'].map((x) => h('th', {}, x)))),
      h('tbody', {}, runs.slice().reverse().map((r) => h('tr', { class: r.ok ? '' : 'bad' },
        h('td', {}, fmtDate(r.ts)), h('td', {}, r.purpose, r.thread ? h('div', {}, h('a', { href: r.thread.startsWith('st-') ? `#/st/${r.thread}` : `#/t/${r.thread}`, class: 'small' }, r.thread)) : null), h('td', {}, r.profile), h('td', {}, r.model),
        h('td', {}, `${r.input_tokens}/${r.output_tokens}`), h('td', {}, `${(r.sent_chars / 1000).toFixed(1)} mil car.`), h('td', { class: 'small' }, (r.boxes_used || []).join(', ') || '—'),
        h('td', {}, `${(r.duration_ms / 1000).toFixed(1)} s`), h('td', { class: 'small' }, r.ok ? r.output_preview : `erro: ${r.error}`))))));
}

export async function renderTrash(main, state) {
  const refresh = () => renderTrash(main, state);
  state.refresh = refresh;
  const items = await rpc('trash.list').catch((e) => { errorToast(e); return []; });
  main.innerHTML = '';
  mount(main, h('header', { class: 'view-head' }, h('h1', {}, t('Lixeira'))),
    h('p', { class: 'muted small' }, 'Itens ficam 30 dias aqui e depois são apagados do disco.'),
    items.length ? items.map((x) => h('div', { class: 'ign' }, h('div', { class: 'grow' }, x.title, h('div', { class: 'muted small' }, `excluída em ${fmtDate(x.day)} · ${x.id}`)),
      x.id.startsWith('t-') ? h('button', { onclick: async () => { await rpc('thread.restore', { id: x.id }).then(() => toast('Restaurada.')).catch(errorToast); refresh(); } }, 'Restaurar') : null)) : h('div', { class: 'empty' }, 'Lixeira vazia.'));
}
