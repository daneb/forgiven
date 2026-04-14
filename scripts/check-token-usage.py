import sys, json
rows = [json.loads(l) for l in sys.stdin if l.strip()]
rows.sort(key=lambda r: r.get('prompt_tokens',0), reverse=True)
for r in rows[:20]:
    print(f\"{r.get('ts',0)}  {r.get('model','?'):<30}  {r.get('prompt_tokens',0):>6}t  {r.get('pct',0):>3}%  session_total={r.get('session_prompt_total',0)}t\")
