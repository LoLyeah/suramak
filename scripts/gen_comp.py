#!/usr/bin/env python3
"""Generate an image via OpenRouter (chat-completions image modality).

usage: gen_comp.py <model-id> <prompt-file> <out.png> [ref-image.png]
"""
import base64
import json
import os
import sys
import urllib.request

key = os.environ.get("OPENROUTER_API_KEY")
if not key:
    sys.exit("OPENROUTER_API_KEY not set")

model, prompt_file, out = sys.argv[1], sys.argv[2], sys.argv[3]
ref = sys.argv[4] if len(sys.argv) > 4 else None

prompt = open(prompt_file, encoding="utf-8").read()
content = [{"type": "text", "text": prompt}]
if ref:
    b64 = base64.b64encode(open(ref, "rb").read()).decode()
    content.append({"type": "image_url", "image_url": {"url": f"data:image/png;base64,{b64}"}})

body = {
    "model": model,
    "messages": [{"role": "user", "content": content}],
    "modalities": ["image", "text"],
}
req = urllib.request.Request(
    "https://openrouter.ai/api/v1/chat/completions",
    data=json.dumps(body).encode(),
    headers={
        "Authorization": f"Bearer {key}",
        "Content-Type": "application/json",
        "HTTP-Referer": "https://localhost",
        "X-Title": "suramak-comp",
    },
)
try:
    resp = json.load(urllib.request.urlopen(req, timeout=900))
except urllib.error.HTTPError as e:
    sys.exit(f"HTTP {e.code}: {e.read().decode('utf-8','replace')[:800]}")

if "choices" not in resp:
    sys.exit(f"unexpected response: {json.dumps(resp)[:600]}")

msg = resp["choices"][0].get("message", {})
images = msg.get("images") or []
if not images:
    sys.exit(f"no image in response. text so far: {(msg.get('content') or '')[:200]!r}")

data_url = images[0]["image_url"]["url"]
if data_url.startswith("data:"):
    b64 = data_url.split(",", 1)[1]
    open(out, "wb").write(base64.b64decode(b64))
else:
    open(out, "wb").write(urllib.request.urlopen(data_url, timeout=300).read())

print(f"saved {out} model={resp.get('model')} usage={resp.get('usage')}")
