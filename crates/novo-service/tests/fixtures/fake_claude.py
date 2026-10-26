#!/usr/bin/env python3
# Claude Code falso para testes: devolve o JSON no formato real do `claude -p --output-format json`.
import json, re, sys, os
args = sys.argv[1:]
schema = args[args.index("--json-schema") + 1] if "--json-schema" in args else ""
stdin = sys.stdin.read()
log = os.environ.get("FAKE_CLAUDE_LOG")
if log:
    with open(log, "a") as f:
        f.write(json.dumps({"args": args, "stdin": stdin}) + "\n")
flag = os.environ.get("FAKE_FAIL_FLAG", "")
if "FAIL-THIS" in stdin and flag and os.path.exists(flag):
    print(json.dumps({"type": "result", "is_error": True, "result": "API Error: overloaded"}))
    sys.exit(1)
# um trecho literal do material, se houver
quote = ""
m = re.search(r'<untrusted-content source="(?!memoria:|agente:)[^"]*">(.*?)</untrusted-content>', stdin, re.S)
if m:
    body = m.group(1).split("---", 1)[-1]
    sentences = [s.strip() for s in re.split(r"(?<=[.!?])\s+", body) if len(s.strip()) > 25]
    sentences = [s for s in sentences if not s.startswith(("Título:", "URL:", "Site:", "This is content"))]
    if sentences:
        quote = sentences[0][:150]
if '"themes"' in schema:
    out = {"summary": "Dia de leitura sobre onboarding.", "themes": ["onboarding", "pricing"]}
elif '"proposals"' in schema:
    out = {"reply": "Comparei com as outras threads.", "proposals": [{"kind": "task", "title": "Reduzir passos do onboarding", "target": "", "reason": "o artigo sugere 3 passos"}]}
elif '"key_points"' in schema and '"title"' in schema:
    out = {"title": "Ativação e preço", "abstract": "Duas leituras sobre ativação", "summary": "As threads tratam de ativação.", "key_points": ["valor antes do cadastro"], "entities": ["onboarding"]}
elif '"abstract"' in schema:
    out = {"abstract": "Artigo sobre onboarding em 3 passos", "summary": "Mostrar valor antes do cadastro.", "stance": "", "questions": [], "key_points": ["valor antes do cadastro"], "quotes": [quote] if quote else [], "entities": ["onboarding", "ativacao"]}
else:
    out = {"body": "Concordo com o ponto central." + (" Para mim o trecho chave é: " + quote if quote else ""), "quotes": [quote] if quote else []}
print(json.dumps({"type": "result", "subtype": "success", "is_error": False, "result": json.dumps(out), "structured_output": out,
                  "total_cost_usd": 0.0001, "usage": {"input_tokens": len(stdin) // 4, "output_tokens": 20, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0},
                  "modelUsage": {"claude-haiku-fake": {}}}))
