// Português do Brasil por padrão; inglês opcional (RNF-18).
const pt = {};
const en = {
  'Rede Social': 'Social', 'Tasks': 'Tasks', 'Observabilidade': 'Observability', 'Desenvolvimento': 'Development',
  'Buscar': 'Search', 'Configurações': 'Settings', 'Lixeira': 'Trash', 'Diário do agente': 'Agent log',
  'O que você está pensando, lendo ou vendo?': 'What are you thinking, reading or watching?', 'Publicar': 'Post',
  'pensando': 'thinking', 'lendo': 'reading', 'vendo': 'watching', 'Pedir ao agente': 'Ask the agent', 'desligado': 'off',
  'resumo': 'summary', 'opinião': 'opinion', 'os dois': 'both', 'Privada': 'Private', 'Li': 'Read', 'Responder': 'Reply',
  'Excluir': 'Delete', 'Abrir chat sobre a thread': 'Open chat about the thread', 'Relacionar ao projeto, thread ou tarefa': 'Relate to project, thread or task',
  'Desfazer': 'Undo', 'Todas': 'All', 'Pendentes de Li': 'Not read', 'Com agente': 'With agent', 'Ignoradas': 'Ignored',
  'Entrada': 'Inbox', 'Hoje': 'Today', 'Próximos 7 dias': 'Next 7 days', 'Projetos': 'Projects', 'Concluídas': 'Done',
  'Novo projeto': 'New project', 'Nome': 'Name', 'Pasta': 'Folder', 'Criar': 'Create', 'Cancelar': 'Cancel', 'criado em': 'created in', 'Essa pasta já é o projeto': 'That folder is already the project',
  'como você chama o projeto': 'what you call the project', 'A pasta é criada se não existir. O projeto aparece no Anthive e em Tasks.': 'The folder is created if missing. The project shows up in Anthive and in Tasks.',
  'expandir': 'expand', 'recolher': 'collapse', 'palavras': 'words',
  'O que só foi ignorado': 'What was only ignored', 'O que eu tenho mais lido': 'What I read most', '% de FOMO': 'FOMO %',
};
let lang = (() => { try { return localStorage.getItem('novo.lang') || 'pt-BR'; } catch { return 'pt-BR'; } })();
export function setLang(l) { lang = l; try { localStorage.setItem('novo.lang', l); } catch {} }
export function getLang() { return lang; }
export function t(s) { return lang === 'en' ? (en[s] || s) : (pt[s] || s); }
