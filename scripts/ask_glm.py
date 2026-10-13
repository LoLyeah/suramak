#!/usr/bin/env python3
"""One-shot GLM 5.3 planning call via OpenCode Zen (opencode-go provider).

Transport is curl (Cloudflare on opencode.ai 403s Python-urllib UAs) and every
request carries x-opencode-session (Zen rejects routeless requests with
HTTP 400 MissingSessionID). glm-5.3 is a reasoning model, so the response may
hit the output ceiling mid-document -> auto-continue until finish_reason != length.

Outputs:
  docs/PLAN-glm.md        assembled plan
  docs/PLAN-glm.raw.json  raw API responses (all rounds, incl. reasoning_content)
"""
import json
import os
import subprocess
import sys
import time
import uuid

API = "https://opencode.ai/zen/go/v1/chat/completions"
MODEL = os.environ.get("PLAN_MODEL", "glm-5.3")
PROMPT = "/home/ubuntu/suramak/docs/plan_prompt.md"
OUT_MD = "/home/ubuntu/suramak/docs/PLAN-glm.md"
OUT_RAW = "/home/ubuntu/suramak/docs/PLAN-glm.raw.json"
PAYLOAD = "/tmp/glm_payload.json"

key = os.environ.get("OPENCODE_GO_API_KEY")
if not key:
    sys.exit("OPENCODE_GO_API_KEY not set")

SESSION = os.environ.get("OC_SESSION_ID") or str(uuid.uuid4())


def call(messages, max_tokens):
    body = {
        "model": MODEL,
        "messages": messages,
        "temperature": 0.3,
        "max_tokens": max_tokens,
        "stream": False,
    }
    with open(PAYLOAD, "w", encoding="utf-8") as f:
        json.dump(body, f)
    cmd = [
        "curl", "-sS", "-m", "1500", "-X", "POST", API,
        "-H", f"Authorization: Bearer {key}",
        "-H", "Content-Type: application/json",
        "-H", f"x-opencode-session: {SESSION}",
        "--data-binary", f"@{PAYLOAD}",
    ]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit(f"curl rc={r.returncode}: {r.stderr[:800]}")
    try:
        resp = json.loads(r.stdout)
    except Exception:
        sys.exit(f"non-JSON response: {r.stdout[:800]}")
    if "choices" not in resp:
        sys.exit(f"API error: {json.dumps(resp)[:800]}")
    return resp


prompt = open(PROMPT, encoding="utf-8").read()
messages = [
    {"role": "system", "content": (
        "You are a senior software architect writing an implementation plan "
        "for AI coding agents. Output ONLY the Markdown plan document. "
        "No preamble, no meta commentary, no questions."
    )},
    {"role": "user", "content": prompt},
]

t0 = time.time()
parts, raws = [], []
max_tokens = 32000
for round_no in range(1, 6):
    resp = call(messages, max_tokens)
    ch = resp["choices"][0]
    msg = ch["message"]
    content = msg.get("content") or ""
    reasoning = msg.get("reasoning_content") or ""
    finish = ch.get("finish_reason")
    raws.append(resp)
    if content:
        parts.append(content)
    usage = resp.get("usage") or {}
    print(
        f"round {round_no}: chars={len(content)} reasoning_chars={len(reasoning)} "
        f"finish={finish} usage={usage} elapsed={int(time.time() - t0)}s",
        flush=True,
    )
    if finish != "length" or not content:
        break
    messages.append({"role": "assistant", "content": content})
    messages.append({"role": "user", "content": (
        "Continue the plan document exactly where it was cut off. "
        "Output only the continuation, no commentary."
    )})
    max_tokens = 16000

plan = "\n".join(parts)
with open(OUT_MD, "w", encoding="utf-8") as f:
    f.write(plan)
with open(OUT_RAW, "w", encoding="utf-8") as f:
    json.dump(raws, f, indent=1)

print(f"DONE chars={len(plan)} rounds={len(parts)} elapsed={int(time.time() - t0)}s")
