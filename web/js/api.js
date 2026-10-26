// Cliente da API do novod: JSON-RPC em POST /rpc com token, eventos em GET /events (SSE).

const KEY = 'novo.token';

export function token() {
  const m = location.hash.match(/token=([0-9a-f]+)/);
  if (m) {
    try { localStorage.setItem(KEY, m[1]); } catch {}
    history.replaceState(null, '', location.pathname + '#/feed');
    return m[1];
  }
  try { return localStorage.getItem(KEY) || ''; } catch { return ''; }
}

let seq = 0;
export async function rpc(method, params = {}) {
  const res = await fetch('/rpc', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token()}` },
    body: JSON.stringify({ jsonrpc: '2.0', id: ++seq, method, params }),
  });
  if (res.status === 401) throw new Error('auth');
  const v = await res.json();
  if (v.error) throw new Error(v.error.message || 'erro');
  return v.result;
}

export function events(onEvent) {
  let es;
  const open = () => {
    es = new EventSource(`/events?token=${token()}`);
    es.onmessage = (m) => { try { onEvent(JSON.parse(m.data)); } catch {} };
    es.onerror = () => { es.close(); setTimeout(open, 3000); };
  };
  open();
}

export function attachmentUrl(thread, file) {
  return `/attachment/${encodeURIComponent(thread)}/${encodeURIComponent(file)}?token=${token()}`;
}
