#!/usr/bin/env python3
"""One-shot GLM 5.3 planning call via OpenCode Zen (opencode-go provider).

Reads docs/plan_prompt.md, POSTs to the OpenAI-compatible endpoint,
saves the plan to docs/PLAN-glm.md and the raw response to docs/PLAN-glm.raw.json.
"""
import json
import os
import sys
import time
import urllib.request

API = "https://opencode.ai/zen/go/v1/chat/completions"
MODEL = os.environ.get("PLAN_MODEL", "glm-5.3")
PROMPT = "/home/ubuntu/suramak/docs/plan_prompt.md"
OUT_MD = "/home/ubuntu/suramak/docs/PLAN-glm.md"
OUT_RAW = "/home/ubuntu/suramak/docs/PLAN-glm.raw.json"

key = os.environ.get("OPENCODE_GO_API_KEY")
if not key:
    sys.exit("OPENCODE_GO_API_KEY not set")

prompt = open(PROMPT, encoding="utf-8").read()
body = json.dumps({
    "model": MODEL,
    "messages": [
        {"role": "system", "content": "You are a senior software architect writing an implementation plan for AI coding agents. Output ONLY the Markdown plan document. No preamble, no meta commentary, no questions."},
        {"role": "user", "content": prompt},
    ],
    "temperature": 0.3,
    "max_tokens": 16000,
    "stream": False,
}).encode()

req = urllib.request.Request(API, data=body, headers={
    "Authorization": f"Bearer {key}",
    "Content-Type": "application/json",
})
t0 = time.time()
try:
    with urllib.request.urlopen(req, timeout=1500) as r:
        resp = json.load(r)
except urllib.error.HTTPError as e:
    detail = e.read().decode("utf-8", "replace")[:2000]
    sys.exit(f"HTTP {e.code}: {detail}")

if "choices" not in resp:
    sys.exit(f"unexpected response: {json.dumps(resp)[:1000]}")

content = resp["choices"][0]["message"]["content"]
with open(OUT_MD, "w", encoding="utf-8") as f:
    f.write(content)
with open(OUT_RAW, "w", encoding="utf-8") as f:
    json.dump(resp, f, indent=1)

finish = resp["choices"][0].get("finish_reason")
print(f"OK model={resp.get('model')} chars={len(content)} finish={finish} usage={resp.get('usage')} elapsed={int(time.time() - t0)}s")
