#!/usr/bin/env python3
"""Gera docs/log/dashboard.html e STATUS.md a partir de docs/log/progress.json.

Uso: python3 -I scripts/gen-dashboard.py   (rode ao fechar cada pacote)
Conta erros (## E-NNN) e decisões (## D-NNN) dos arquivos de docs/log.
"""
import html, json, re, sys
from pathlib import Path

root = Path(__file__).resolve().parent.parent
log = root / "docs" / "log"
data = json.loads((log / "progress.json").read_text())


def count(name, prefix):
    return len(re.findall(rf"^## {prefix}-\d+", (log / name).read_text(), re.M))


def last_days(n=3):
    text = (log / "journal.md").read_text()
    parts = re.split(r"^## ", text, flags=re.M)[1:]
    return ["## " + p.strip() for p in parts[-n:]]


errors, decisions = count("errors.md", "E"), count("decisions.md", "D")
pk = data["packages"]
planned = [p for p in pk if p["total"]]
done = sum(p["done"] for p in planned)
total = sum(p["total"] for p in planned)
t = data["tests"]

# STATUS.md
rows = "\n".join(
    f"| {p['id']} | {p['name']} | {p['done']}/{p['total']} | {p['status']} | {p['dates']} |"
    for p in pk
)
pending = "\n".join(f"- {x}" for x in data["pending"])
(root / "STATUS.md").write_text(
    f"""# Status

Gerado por `scripts/gen-dashboard.py` a partir de `docs/log/progress.json`. Não edite à mão.
Atualizado em {data['updated']}. Painel: [`docs/log/dashboard.html`](docs/log/dashboard.html).

- Tarefas planejadas concluídas: **{done}/{total}**
- Testes: **{t['passed']} passam, {t['failed']} falham** ({t['note']})
- Erros registrados: {errors} ([errors.md](docs/log/errors.md)); decisões: {decisions} ([decisions.md](docs/log/decisions.md))
- Diário: [journal.md](docs/log/journal.md)

| Pacote | Nome | Tarefas | Estado | Datas |
|---|---|---|---|---|
{rows}

## Pendências
{pending}
"""
)

# dashboard
e = html.escape
cards = "".join(
    f'<div class="card"><b>{n}</b><span>{l}</span></div>'
    for n, l in [(f"{done}/{total}", "tarefas"), (t["passed"], "testes ok"), (t["failed"], "testes falhando"), (errors, "erros"), (decisions, "decisões")]
)
trs = ""
for p in pk:
    pct = int(100 * p["done"] / p["total"]) if p["total"] else 0
    trs += (
        f"<tr><td>{e(p['id'])}</td><td>{e(p['name'])}<br><small>{e(p['plan'])}</small></td>"
        f"<td><div class='bar'><i style='width:{pct}%'></i></div>{p['done']}/{p['total']}</td>"
        f"<td>{e(p['status'])}</td><td>{e(p['dates'])}</td></tr>"
    )
pend = "".join(f"<li>{e(x)}</li>" for x in data["pending"])
days = "".join(f"<pre>{e(d)}</pre>" for d in last_days())
(log / "dashboard.html").write_text(
    f"""<!doctype html><html lang="pt-BR"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1"><title>linux-store: painel</title>
<style>
:root{{--bg:#fff;--fg:#1d2126;--mut:#5b6670;--line:#d9dee3;--acc:#2f6fed;--card:#f3f5f7}}
@media (prefers-color-scheme:dark){{:root{{--bg:#14171a;--fg:#e6e9ec;--mut:#98a2ab;--line:#2c333a;--acc:#6b9bff;--card:#1d2227}}}}
body{{background:var(--bg);color:var(--fg);font:15px/1.5 system-ui,sans-serif;margin:0 auto;max-width:960px;padding:0 16px 48px}}
h1{{font-size:1.4rem}}h2{{font-size:1.05rem;margin-top:2rem}}
.cards{{display:flex;flex-wrap:wrap;gap:12px}}.card{{background:var(--card);border-radius:8px;padding:10px 16px}}
.card b{{display:block;font-size:1.5rem}}.card span{{color:var(--mut);font-size:.8rem}}
table{{border-collapse:collapse;width:100%}}td,th{{border-bottom:1px solid var(--line);padding:6px 8px;text-align:left;vertical-align:top}}
small{{color:var(--mut)}}.bar{{background:var(--line);border-radius:4px;height:8px;width:120px}}.bar i{{display:block;background:var(--acc);height:8px;border-radius:4px}}
pre{{background:var(--card);padding:10px;border-radius:8px;white-space:pre-wrap;overflow-wrap:anywhere}}
@media (max-width:600px){{td:nth-child(5),th:nth-child(5){{display:none}}}}
</style></head><body>
<h1>linux-store: painel</h1><small>Atualizado em {e(data['updated'])}. Gerado de docs/log/progress.json.</small>
<div class="cards">{cards}</div>
<h2>Pacotes</h2><table><tr><th>#</th><th>Pacote</th><th>Tarefas</th><th>Estado</th><th>Datas</th></tr>{trs}</table>
<h2>Pendências</h2><ul>{pend}</ul>
<h2>Últimos dias no diário</h2>{days}
</body></html>"""
)
print(f"ok: STATUS.md e docs/log/dashboard.html ({done}/{total} tarefas, {errors} erros, {decisions} decisões)")
