# M17.1-A — Local SLM runtime feasibility (llama.cpp, EN+ID polish)

**Decision: STOP for M17.2 as scoped (optional local polish at 1.5–3B).** The runtime
direction is *not* the problem — `llama-cpp-2` builds and runs cleanly here and is the
right binding if the feature is ever re-opened. The model envelope is the problem: on this
2-vCPU / 3.6 GiB box no tested instruct model (Qwen2.5-1.5B, Qwen3-1.7B, Qwen2.5-3B at
q4_K_M) produced the endpoint's polish contract. The endpoint's exact prompt yields a
near-no-op; a few-shot local prompt yields real edits but also meaning-changing errors
(a translated segment, an invented clause, a flipped negation) that the production
validator cannot see. Add ~3–7 s of CPU per transcript segment and a 1.7–1.8 GiB RSS
peak (2.6 GiB for the 3B, which does not fit this box) and the feature is both slower
than the endpoint and worse than doing nothing.

STOP is the first-class outcome the workorder asked for. §7 records the re-visit
condition; §6 is the slice list a future GO would need.

Provenance: workorder `docs/WORKORDER-M17A.md`, plan `docs/PLAN-M15-M18.md` §4 M17.1,
contract reference `crates/suramak-core/src/polish.rs` (read-only). Tree `main` @
`36362bb` (v0.4.1). Throwaway probe under gitignored `build/m17a-probe`; models in
`~/.hermes/cache/m17a/models` (never the repo). No production file, dependency, CI file
or test was touched (`git status` clean apart from this report).

---

## 1. Machine-named numbers

```text
probe_date=2026-11-11
m17a.decision=STOP
m17a.runtime.direction=llama-cpp-2
host=VM-0-5-ubuntu
cpu=AMD EPYC 7K62 48-Core, 2 vCPU, 1 thread/core
ram_total_mib=3718          # 3.63 GiB, 6144 MiB swap; 1.3–2.6 GiB available during runs
kernel=7.0.0-14-generic
rustc=1.99.0
llama_cpp_version=0.6.0     # vendored inside llama-cpp-sys-2 0.1.161
bindings=llama-cpp-2 0.1.161 (MIT OR Apache-2.0)
build_cmd=cargo build --release (probe crate, default-features = false)
build_wall_s=381

binary_probe_bytes=6487656
binary_hello_bytes=444664
binary_delta_bytes=6042992          # +5.76 MiB, llama.cpp static + serde_json/sha2
static_archive_bytes=14691100       # libllama 10,996,056 + ggml-cpu 2,078,286
                                    # + ggml-base 1,544,182 + ggml 72,576
```

Model files (all downloaded to `~/.hermes/cache/m17a/models`, sha256 verified locally
against the HuggingFace LFS oid):

```text
qwen25_1_5b.repo=Qwen/Qwen2.5-1.5B-Instruct-GGUF
qwen25_1_5b.file=qwen2.5-1.5b-instruct-q4_k_m.gguf
qwen25_1_5b.bytes=1117320736
qwen25_1_5b.sha256=6a1a2eb6d15622bf3c96857206351ba97e1af16c30d7a74ee38970e434e9407e

qwen25_3b.repo=Qwen/Qwen2.5-3B-Instruct-GGUF
qwen25_3b.file=qwen2.5-3b-instruct-q4_k_m.gguf
qwen25_3b.bytes=2104932768
qwen25_3b.sha256=626b4a6678b86442240e33df819e00132d3ba7dddfe1cdc4fbb18e0a9615c62d

qwen3_1_7b.repo=unsloth/Qwen3-1.7B-GGUF      # official Qwen/Qwen3-1.7B-GGUF ships Q8_0 only
qwen3_1_7b.file=Qwen3-1.7B-Q4_K_M.gguf
qwen3_1_7b.bytes=1107409472
qwen3_1_7b.sha256=b139949c5bd74937ad8ed8c8cf3d9ffb1e99c866c823204dc42c0d91fa181897
```

---

## 2. Runtime direction: `llama-cpp-2`, not hand-rolled FFI, not candle

`m17a.runtime.direction=llama-cpp-2`

| | `llama-cpp-2` 0.1.161 | vendored FFI over llama.cpp | `candle` |
|---|---|---|---|
| maintenance | 0.1.161 published 2026-11-03; ≥12 releases in 2026; 1.92 M downloads (885 k recent); repo pushed 2026-11-09; 669 ★, 68 open issues | upstream llama.cpp is a moving target (b12271 released the day of this probe, MIT, 134 k ★) — we would own the binding layer | active, but a different runtime: no GGUF/llama.cpp kernel parity, we would re-validate everything |
| build integration | vendors llama.cpp (40 MB source, `LLAMA_VERSION 0.6.0`) and builds it with `cmake` + `bindgen` — **the exact stack `whisper-rs-sys` 0.15.0 already uses** (`cmake::Config`, `bindgen::Builder`, `-lstatic=whisper/ggml-*`) | same cmake work, plus hand-written bindings and a safe wrapper (~1–2 kLOC) to maintain per platform | pure Rust, `cargo` only, no C toolchain |
| package size | static link: **+6.0 MB** on the executable (measured); archives 14.7 MB pre-link | same native payload, plus our wrapper | model weights dominate (~1.1 GB), runtime is small |
| platform reach for later gates | features already present: `metal` (auto on macOS aarch64), `cuda`, `vulkan`, `rocm`, `opencl`, `android-shared-stdcxx`/`static-stdcxx`, `dynamic-backends`, `mtmd` | we would implement each | Metal/CUDA supported |
| API gaps found in source | **no cancel/abort API**; `LlamaContext.context` is `pub(crate)` so `llama_set_abort_callback` (llama.h:1123) is unreachable; `apply_chat_template(tmpl, chat, add_ass)` takes no template kwargs (no `enable_thinking=false`); GBNF grammar *is* exposed (`LlamaSampler::grammar`) | full control, including abort + kwargs | n/a |

Why the binding wins: it is a thin wrapper around exactly the C API we would bind, it is
released weekly against upstream, and its build is byte-for-byte the bundled-C pattern the
repo already trusts (`symphonia-adapter-libopus`/`whisper-rs-sys`). Hand-rolling FFI would
buy two functions (abort callback, template kwargs) at the cost of owning a binding layer
across macOS/Windows/Linux/Android. The crate's own README calls it "not safe" — it is a
thin unsafe wrapper, so the M17.2 code would audit its call sites like the whisper
trampoline already does.

Measured build/run facts that back this: `cargo build --release` on 2 cores = **381 s**,
binary delta **+6.0 MB**, model load 0.65–0.86 s warm (3.98 s cold for 1.1 GB), context
creation 27–46 ms. Nothing in the build needed sudo or a system package that
`whisper-rs-sys` does not already need (cmake + libclang for bindgen).

**Cancellation is the one real gap.** `llama.cpp` can abort a decode via
`llama_set_abort_callback`; `llama-cpp-2` exposes neither the callback nor the raw
context pointer, so the only cancellation is cooperative, between `decode` calls:

```text
cancel_during_prefill.delay_ms=300
cancel_during_prefill.prefill_ms=1402
cancel_during_prefill.abort_latency_ms=1317.9   # waits for the in-flight prefill decode
cancel_during_decode.delay_ms=5000
cancel_during_decode.abort_latency_ms=42.1      # one token
```

Consequence for any future local backend: prefill must be chunked (≤ ~8 segments per
request) so a cancel never waits more than ~1–2 s, or the binding needs a one-line raw
escape hatch (fork/patch of `llama-cpp-sys-2`, which is a dependency decision, not a
probe).

`candle` was not evaluated: the plan says one direction at a time, and llama.cpp did not
fail a *runtime* gate.

---

## 3. Candidate models: license review

| model | HF repo (pinned file) | bytes | license | gated | verdict |
|---|---|---|---|---|---|
| Qwen2.5-1.5B-Instruct | `Qwen/Qwen2.5-1.5B-Instruct-GGUF` · `qwen2.5-1.5b-instruct-q4_k_m.gguf` | 1,117,320,736 | Apache-2.0 | no | probed |
| Qwen3-1.7B | `unsloth/Qwen3-1.7B-GGUF` · `Qwen3-1.7B-Q4_K_M.gguf` (official repo ships only Q8_0, 1.83 GB) | 1,107,409,472 | Apache-2.0 | no | probed |
| Qwen2.5-3B-Instruct | `Qwen/Qwen2.5-3B-Instruct-GGUF` · `qwen2.5-3b-instruct-q4_k_m.gguf` | 2,104,932,768 | Apache-2.0 | no | probed (memory gate) |
| Gemma-2-2b-it | `google/gemma-2-2b-it-GGUF` · `2b_it_v2.gguf` (single 10.46 GB file, no q4_K_M) | 10,463,413,632 | Gemma Terms (non-OSI) | **yes — `manual`; raw fetch returns HTTP 401** | disqualified |
| Phi-3.5-mini-instruct | `bartowski/Phi-3.5-mini-instruct-GGUF` · `Phi-3.5-mini-instruct-Q4_K_M.gguf` | 2,393,232,672 | MIT | no | not probed: 3.8 B/2.39 GB is outside this box's envelope (3B already failed it) |

Pinned file hashes (the sha256 of the exact GGUF blob, from the HF LFS oid; the three
downloaded files were re-hashed locally and matched):

```text
qwen25_1_5b.sha256=6a1a2eb6d15622bf3c96857206351ba97e1af16c30d7a74ee38970e434e9407e
qwen3_1_7b.sha256=b139949c5bd74937ad8ed8c8cf3d9ffb1e99c866c823204dc42c0d91fa181897
qwen25_3b.sha256=626b4a6678b86442240e33df819e00132d3ba7dddfe1cdc4fbb18e0a9615c62d
phi35_mini.sha256=e4165e3a71af97f1b4820da61079826d8752a2088e313af0c7d346796c38eff5
gemma2.sha256=unavailable            # gated: LFS oid not exposed anonymously (HTTP 401)
```

License texts, hashed as fetched:

```text
apache2.license_file_bytes=11343
apache2.license_sha256=832dd9e00a68dd83b3c3fb9f5588dad7dcf337a0db50f7d9483f310cd292e92e
# identical bytes in Qwen/Qwen2.5-1.5B-Instruct-GGUF/LICENSE and Qwen/Qwen3-1.7B/LICENSE
mit_phi35.license_file_bytes=1084
mit_phi35.license_sha256=fa8235e5b48faca34e3ca98cf4f694ef08bd216d28b58071a1f85b1d50cb814d

gemma2.readme_http_status=401        # gated: no fetch without an HF account/token
gemma2.license=Gemma Terms of Use (https://ai.google.dev/gemma/terms)
gemma2.redistribution=pass-through terms required; use restrictions apply; not OSI
```

The Gemma gate is disqualifying on its own for M17.2's download story: the app cannot
fetch the weights anonymously, so "optional offline polish" would require the user to
hold a HuggingFace account and paste a token before the feature works. The only Gemma
GGUF in the official repo is also a 10.46 GB file, not a q4_K_M — there is no 1.1–1.6 GB
artifact to pin.

Both probed Qwen repos are non-gated, Apache-2.0, and their LFS sha256 matched the
downloaded bytes exactly (verified with `sha256sum`).

---

## 4. Probe results

### 4.1 Load, RAM, tokens/s at 1/2/4 threads

79-token prompt, 64 generated tokens, greedy (temperature 0), `n_ctx=4096` unless noted.
`load` is model load from page cache (the first, cold load of the 1.5B model took
3,975 ms); `hwm` is `VmHWM` peak RSS for the whole process.

| run | threads | load ms | ctx ms | prefill t/s | decode t/s | hwm MiB |
|---|---|---|---|---|---|---|
| qwen2.5-1.5B | 1 | 796 | 87 | 25.89 | 11.36 | 1838 |
| qwen2.5-1.5B | 2 | 719 | 46 | 28.07 | **15.44** | 1838 |
| qwen2.5-1.5B | 4 | 647 | 46 | 18.37 | **0.61** | 1839 |
| qwen2.5-1.5B (ctx 2048) | 2 | 662 | 27 | 28.64 | 15.61 | 1782 |
| qwen2.5-1.5B (ctx 1024) | 4 | 4120¹ | 16 | 18.72 | 0.60 | 1745 |
| Qwen3-1.7B | 1 | 4187¹ | 600 | 4.24¹ | 9.22 | 1857 |
| Qwen3-1.7B | 2 | 859 | 318 | 15.27 | 10.37 | 1856 |
| Qwen3-1.7B | 4 | 781 | 304 | 16.80 | 0.60 | 1855 |
| Qwen2.5-3B (ctx 2048) | 2 | 9789¹ | 149 | 5.51¹ | 5.70¹ | **2685** |

¹ cold page cache / swap-affected: the 3B run paged ~970 MB out during the bench
(`pswpout` +248,325 pages) and never recovered; the Qwen3 t1 run's 4.2 s load and 4.24 t/s
prefill are the same first-touch effect.

**The 4-thread column is a hard failure, not noise.** 0.60–0.61 t/s at 4 threads appears
on both models, at ctx 4096 and ctx 1024, with no cgroup CPU quota (`cpu.max` empty,
`nr_throttled 0`) and only 53 MB of swap during the ctx-1024 run. A plain pthread
CPU-burner on the same box costs only 2× at 4 threads (0.173 s → 0.351 s), so the box
itself does not have that pathology — it is ggml's spin-barrier
(`ggml_barrier` busy-waits on `n_barrier_passed`) under oversubscription. Product rule:
**`n_threads = physical cores`, never more.**

Decode scaling 1→2 threads is only 1.36× (11.36 → 15.44 t/s), i.e. decode is
memory-bandwidth-bound; prefill barely scales at all (25.9 → 28.1 t/s). More threads
cannot rescue this box.

Peak RSS 1.74–1.81 GiB for a 1.5B q4 model at 2–4 K context, on a 3.6 GiB machine that
also runs the app. That is the honest desktop worst case the workorder asked for.

### 4.2 Quality — the endpoint's exact prompt (`SYSTEM_PROMPT`, temperature 0)

10 cases (5 EN + 5 ID), 3 segments each, ASR-shaped text with embedded names, numbers and
negations. "changed" = output segment differs from input; "beyond case" = differs even
after case-folding (i.e. a real edit, not just capitalisation).

| model | production-valid | changed / segments | beyond case | invented names | dropped names | invented numbers | dropped numbers |
|---|---|---|---|---|---|---|---|
| Qwen2.5-1.5B | **10/10** | 8/30 | **1/30** | 0 | 0 | 0 | 0 |
| Qwen3-1.7B (`/no_think`) | **0/10** | 10/30 | 2/30 | 0 | 0 | 0 | 0 |

Qwen2.5 follows the JSON envelope perfectly and then **copies the input back**. The one
real edit in 30 segments was `cant` → `can't`. No punctuation is added anywhere:

```text
case=en-01 in : "so priya and marcus will take the migration plan to the board on the 14th"
case=en-01 out: "so priya and marcus will take the migration plan to the board on the 14th"
case=en-02 in : "we dont have a date for the 18 percent latency improvement yet"
case=en-02 out: "we dont have a date for the 18 percent latency improvement yet"
case=id-01 in : "rina bilang rapat dengan pak andi dipindah ke tanggal 15"
case=id-01 out: "rina bilang rapat dengan pak andi dipindah ke tanggal 15"
```

That is the worst failure mode for this feature: `parse_and_validate` accepts it (same
ids, same count, non-empty), so the app would report a successful polish that changed
nothing. 9.0–13.6 s wall per 3-segment batch.

Qwen3-1.7B edits 10/30 segments (8 of those capitalisation only) but **fails the
production parser on all 10**: its template emits an empty reasoning block (the
think open/close markers with nothing between them) before the JSON, and
`strip_fences` only strips Markdown JSON fences. With thinking left on (no `/no_think`), it
spent the entire 512-token budget on monologue and produced no JSON at all (0/7 cases,
~49 s each).

### 4.3 Quality — few-shot local prompt (the ceiling test)

Same 6 cases, same JSON envelope, plus two worked examples in the system prompt (EN + ID).
This is *not* the endpoint's prompt; it tests whether the quality ceiling is the prompt or
the model.

| model | production-valid | changed / segments | meaning-changing errors |
|---|---|---|---|
| Qwen2.5-1.5B | 6/6 | 18/18 | **2/18**: `id-02` segment translated to English; `id-05` invented clause |
| Qwen3-1.7B (`/no_think`) | **0/6** (think prefix) | 18/18 | **1/18**: `en-05` negation flipped (`can't` → `can`) |
| Qwen2.5-3B | — | — | run aborted: no batch completed in 8 min under memory pressure |

Verbatim evidence (the three errors, all contract-breaking):

```text
id-02 in : "tolong cek nomor resi nya ya"
id-02 out: "Please check the resi number, okay."          # translated; "Keep the original language"

id-05 in : "tolong kirim undangan baru ke semua peserta"
id-05 out: "Berdasarkan permintaan tersebut, saya minta kirim undangan baru ke semua peserta."
                                                          # invented clause ("Based on that request, I ask")

en-05 in : "sam cant make the 7 pm call so we moved it to tomorrow morning"
en-05 out: "Sam can make the 7 pm call so we moved it to tomorrow morning."
                                                          # negation dropped: meaning reversed
```

With the few-shot prompt the model *does* punctuate and capitalise properly (18/18 changed,
including ID), and names/numbers survive 100 % (0 invented, 0 dropped across 36 segments).
But 3 of 36 segments (8.3 %) carry a meaning change, and the production validator sees none
of them: it checks ids, count and non-emptiness only. The endpoint's own prompt avoids
these errors by never editing — which is why the two regimes are a lose/lose at this size.

### 4.4 Where the time goes (UX envelope)

Per 3-segment batch on 2 threads: 9.0–13.6 s (Qwen2.5 contract prompt), 10.2–17.2 s
(Qwen3), 16.6–21.6 s with the few-shot prompt (longer prompt). That is **≈3–7 s per transcript
segment**. A 30-minute meeting is ~250–350 segments, and the endpoint sends all segments in
*one* request — a local equivalent would need chunking (a 2 K context and a 512-token
output budget hold ~16 segments)
and would take **15–35 minutes of pinned CPU** versus seconds for the endpoint. The local
path is not a substitute; at best a background job for short notes.

### 4.5 Package size

```text
binary_probe_bytes=6487656     # llama.cpp static + serde_json + sha2 + our 400-line probe
binary_hello_bytes=444664      # same profile, no deps
binary_delta_bytes=6042992     # +5.76 MiB -> the runtime's executable cost
static_archive_bytes=14691100  # pre-link: libllama 10.5 MiB, ggml-cpu 2.0 MiB, ggml-base 1.5 MiB
model_download_bytes=1117320736..2104932768
```

A local backend is therefore ~6 MB of binary plus a 1.1–2.1 GB model download the user must
consent to. The 6 MB is acceptable; the model download is the product decision.

---

## 5. What this probe did *not* establish (honest boundaries)

- **One machine.** 2 vCPU EPYC / 3.6 GiB / no GPU. Nothing here is a verdict on the owner's
  Mac (Metal, more RAM) or on any 4 GB Android device — both remain **later gates**, as the
  workorder says. Apple Silicon could be several times faster; it will not fix a prompt-fidelity
  failure (the few-shot errors are capacity errors, not speed errors).
- **No long-transcript run.** Chunking, KV-cache reuse, unload behaviour and download
  resume/corruption were not exercised; they belong to M17.2.
- **The 3B quality question is open, not answered.** The 3B model could not complete a
  single batch on this box (2.62 GiB peak RSS, ~970 MB paged out, killed after 8 min), so
  "a bigger model fixes the fidelity errors" is *unproven*. It is also unaffordable on a
  3.6 GiB machine.
- **One few-shot prompt, two examples.** The 2-error / 1-error rates are small-sample
  point estimates, not a benchmark. The *class* of error (translation, invention, negation)
  is what matters: it is exactly what the plan's gate names.
- **`candle` not evaluated.** Per the plan's one-direction rule; llama.cpp did not fail a
  runtime gate.
- **Windows/macOS builds of the probe not run.** The crate advertises the features; the
  repo's own release matrix would be the place to prove them.

---

## 6. If the owner overrides STOP: the M17.2 slice list this probe implies

1. **Model pin + download**: sha256-pinned GGUF (the table in §3), resumable download,
   integrity check, explicit disk-usage consent, delete/unload path.
2. **Chunking**: 8–12 segments per request (≈250–350 prompt and ≈250–350 output tokens),
   so prefill stays near ~2 s and cancellation is bounded; reuse one context across chunks.
3. **Grammar**: GBNF-constrained JSON (`LlamaSampler::grammar`) so `{"segments":[{"id","text"}]}`
   cannot drift; keep `parse_and_validate` as the final gate (read-only reuse).
4. **A local-only fidelity gate** the endpoint does not need: strip a leading think block,
   then diff names/numbers/negations/language against the input and *refuse the whole
   batch* on any mismatch (fall back to the unpolished text, never silently accept).
5. **Diff preview UI**: never replace transcript text without showing the change set — the
   measured errors are silent.
6. **Threads = physical cores**, `n_ctx` 2048, unload after idle, and no automatic
   endpoint fallback on failure (the plan's own M17.2 rule).
7. **macOS/Windows/Android builds** proven before any platform ships it; the Android gate
   needs a real 4 GB device and a memory ceiling well below the 1.74 GiB measured here.

## 7. Re-visit condition

Re-open M17.1 when a **non-gated, permissively licensed instruct model** meets *all* of:

```text
quality: 0 meaning-changing errors (names / numbers / negation / language / invention)
         over a 20-case EN+ID fidelity set, using the endpoint's prompt shape
speed:   >= 20 decode tok/s at 2 threads on this VPS (today: 15.4 at 1.5B)
memory:  <= 1.5 GiB peak RSS at n_ctx 2048 (today: 1.74 GiB at 1.5B)
size:    <= 1.2 GB q4 file (so the download is a 1-click, not a 2 GB commitment)
```

or when the feature is **re-scoped away from fidelity**: a local *notes/summary* pass
(fabrication-tolerant, user-editable) has a different gate and might pass at 1.5B. Polish
— "correct the text, change nothing else" — does not pass at this size today.

A cheaper alternative that this probe supports: keep the endpoint as the only polish path
and revisit when a 2–3B model with a 1.2 GB quant ships (Qwen3.5-class, Gemma 4-class).
The measured runtime work (binding choice, build recipe, thread/RAM envelope, cancellation
behaviour) carries over unchanged.

---

## 8. Verification of the untouched workspace

```text
git_status=?? docs/reports/M17a-slm-feasibility.md   (only the report is new)
cargo_fmt_all_check=clean
cargo_clippy_p_suramak_core_lib=-D warnings: clean (exit 0)
cargo_test_p_suramak_core=all green (lib 176 + integration suites; tests/polish.rs 22 passed)
probe_crate=build/m17a-probe (gitignored, not a workspace member, no committed dependency)
```

No production code, dependency, CI file, or test was modified; the probe never entered the
workspace graph, so the workspace's green state is the pre-existing one, re-confirmed above.

---

## Appendix — reproduction

```bash
# probe crate (gitignored throwaway; kept under build/ for re-runs)
build/m17a-probe/{Cargo.toml,src/main.rs,cases.json,cases_fewshot.json,sys_fewshot.txt}

cargo build --release                     # 381 s on 2 cores, llama.cpp 0.6.0 via cmake

# tokens/s + RAM (one JSON line per run)
./target/release/m17a-probe bench --model <gguf> --threads 2 --tokens 64 --ctx 4096

# the 10-case polish set, endpoint prompt
./target/release/m17a-probe polish --model <gguf> --threads 2 --tokens 512 --ctx 2048 \
  --cases cases.json --label qwen25 --out results/polish_qwen25.jsonl

# cancellation (cooperative; the binding exposes no abort callback)
./target/release/m17a-probe cancel --model <gguf> --threads 2 --delay-ms 300

# scoring (contract validity + segment diff + name/number/negation counts)
python3 analyze.py cases.json results/polish_qwen25.jsonl
```

Raw result lines: `build/m17a-probe/results/*.jsonl`; model files and their sha256s are
listed in §1 and were removed from the cache after the probe
(`~/.hermes/cache/m17a/models`, never in the repo).
