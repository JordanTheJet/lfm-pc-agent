# Model benchmark — which model should drive the agent?

The orchestrator delegates *grounding* to the macOS accessibility tree, so the model's
only job is **reasoning/selection**: given a task + a numbered element list, emit one JSON
action. We benchmarked that directly with the `bench` subcommand, which reuses the **exact
live prompt and parser** (`build_user_text` + `reason::decide`) so every model is judged
apples-to-apples.

```bash
cargo run -- bench --url http://<server>:<port> --model <name>   # 14-case suite in bench/fixtures.json
```

Suite: 14 cases over launch / type / save-sheet / dialog / browser / Finder, including
**adversarial** ones (ambiguous "Save" vs "Don't Save", empty element list, already-complete
task, target-not-present). Scoring per case: valid-JSON, action-type match, **exact-match**
(action *and* target), safety (no click on a non-existent id), and latency.

## Fair re-run (33 cases, `fair` scoring) — the definitive result

The original 14-case table below **overstated the 24B**. On an expanded, rebalanced 33-case
suite (`bench/fixtures-v2.json`) with a `fair` metric that credits clicking the *correct* field
when the expected action is `type` into it (Qwen's valid focus-then-type strategy):

| model / mode | exact | **fair** | safe | p50 | VRAM |
|--------------|:-----:|:--------:|:----:|:---:|:----:|
| **Qwen3.6-35B-A3B (grammar)** | **82%** | **100%** | **100%** | 839 ms | 23 GB |
| Qwen3.6-35B-A3B (thinking) | 76% | 91% | 100% | 1093 ms | 23 GB |
| LFM2-24B-A2B (grammar) | 76% | 76% | 94% | **176 ms** | 15 GB |
| LFM2.5-8B-A1B (grammar) | 67–73% | 73–82% | 97–100% | 1511 ms | 5.5 GB |
| LFM2.5-8B-A1B (unconstrained) | 9% | 9% | 100% | — | 5.5 GB |

The newer **LFM2.5-8B-A1B** is competitive on accuracy (≈the 24B) and comparably safe at a third
the VRAM — but it is **grammar-*dependent*** (un-constrained it collapses to 15% valid JSON; the
grammar rails are what make a small model usable) and, surprisingly, the **slowest** (~1500 ms;
the LFM2.5 hybrid arch is under-optimized in llama.cpp — "smaller ≠ faster"). Net: Qwen 3.6 is the
most accurate/safest model; the **24B's one real, defensible merit is speed** (fast *and*
accurate-enough for an interactive loop) — not being the best model.

**Corrected conclusion: Qwen 3.6 is genuinely more accurate *and* safer on this task** (82% exact
/ 100% fair vs the 24B's 76%/76%; 100% safe vs 94% — the 24B made the only unsafe hallucinated
clicks and failed an OK-vs-Cancel trap). The 24B's **only** real edge is **~5× lower latency**
(a size/no-thinking effect, not capability). The earlier "24B wins" was an artifact of a tiny
14-case suite over-weighted toward the exact scenarios where Qwen's click-to-focus was penalized.
Picking the 24B for the demo is defensible on **latency / VRAM / on-brand-Liquid** grounds — not
on accuracy.

## Results (original 14-case run — kept for history; superseded by the fair re-run above)

| model | params | exact-match | valid-JSON | safe | p50/step | served by |
|-------|--------|:-----------:|:----------:|:----:|:--------:|-----------|
| LFM2.5-VL-1.6B | 1.6B | 29% | 100% | 93% | 0.37 s | llama.cpp (Mac) |
| LFM2.5-8B-A1B | 8.3B MoE (1.5B active) | 36% | 100% | 93% | 0.62 s | llama.cpp (Mac) |
| **LFM2-24B-A2B** | **24B MoE (2.3B active)** | **79%** | **100%** | 93% | 1.0 s | llama.cpp (Mac) |
| Qwen3-30B-A3B-2507 | 30B MoE (3.3B active) | 79% | 100% | 100% | 0.62 s | llama.cpp (Mac) |
| Qwen3.6-35B-A3B (thinking, Ollama) | 35B MoE (3B active) | 71%¹ | 93% | 100% | 1.6 s | Ollama (RTX 3090) |
| **Qwen3.6-35B-A3B (grammar, llama.cpp)** | **35B MoE (3B active)** | **64%²** | **100%** | **100%** | **0.94 s** | llama.cpp (RTX 3090) |
| Qwen3.6-35B-A3B (thinking, llama.cpp) | 35B MoE (3B active) | 71%³ | 100% | 100% | 1.0 s | llama.cpp (RTX 3090) |

¹ Served via Ollama with thinking-mode on (one case leaked reasoning instead of clean JSON).

² **Fair re-run (2026-07), unsloth GGUF on llama.cpp, grammar-constrained — same harness as
the 24B.** Result went *down*, not up: Qwen3.6 is a **thinking model**, and forcing the JSON
action from the first token suppresses its reasoning, so it defaults to "click the field"
instead of "type into it" (4 of its 5 misses). "Thinking-off + grammar" is a handicap for
this model, not a fair boost — and even its thinking-on best (71%) trails the 24B. On this
AX-selection task the **24B wins outright (79% vs 64%) and is ~5× faster (177 ms vs 941 ms
p50) at ⅔ the VRAM.** (The earlier *non-thinking* Qwen3-30B-A3B-2507 tied the 24B at 79% —
so the gap here is specific to the thinking-model variant under output constraints.)

³ Thinking-ON, unconstrained (llama.cpp, `bench --no-grammar`) — reconfirms the Ollama 71%
exactly. Thinking is worth **+7 pts** over grammar-constrained (64% → 71%) for this model, but
still **trails the 24B's 79%** and is **~6× slower** (p50 1026 ms vs 177 ms). Even with thinking,
3 of its 4 misses are "click the field" instead of typing into it.

**Honesty check (don't overclaim this):** on inspection, those 3 misses are Qwen selecting the
*correct* element and choosing `click` (focus) instead of the demo's one-shot `type` — a valid
click-then-type strategy that this single-turn harness doesn't credit, not a wrong selection.
On target selection the models are ~equivalent; with 14 cases a 1–2 case gap is within noise;
and Qwen was actually *safer* (100% vs the 24B's 93% — the 24B made the suite's only unsafe
move, a hallucinated click on an empty list). The 24B's genuine, robust edge for THIS demo is
**latency (~6×) and matching the demo's `type` convention (fewer turns)** — a better *fit* for a
fast single-action loop, NOT a better model. In general capability the newer, larger Qwen 3.6 is
stronger (see the tool-chain result and published benchmarks). The harness was built for the LFM,
which plausibly biases the convention in its favor.

## Findings

1. **Edge models are not enough.** The 1.6B (29%) and 8B (36%) over-default to `open_app`
   and mishandle the element list. They confirm the original complaint.
2. **The cliff is at ~24B, not above it.** Accuracy jumps 36% → **79%** at the 24B tier and
   then **plateaus** — the 30B and the SOTA 35B don't beat it. For this task (select an
   element/keystroke from an AX list), capability saturates; the residual misses are mostly
   "clicked the right field instead of typing into it," which more parameters don't fix.
3. **SOTA doesn't win here.** Qwen3.6-35B-A3B ties the 24B at best, at higher VRAM cost.

## Recommendation → `LFM2-24B-A2B`

On-brand (keeps the "local Liquid AI LFM" story), ties SOTA, and at **~14 GB Q4_K_M** it
runs on a 24 GB GPU or a 32 GB+ Mac (the 35B's ~19–23 GB crashes a 32 GB Mac). The
default `setup.sh` now serves it:

```bash
llama-server -hf LiquidAI/LFM2-24B-A2B-GGUF:Q4_K_M -c 8192 --port 8080 -ngl 99 --jinja
# then drive it (text/AX-only — the 24B has no vision tower):
cargo run -- run --verify-file ~/Desktop/zeroclaw-demo.txt
```

### Caveats
- 14 cases is **directional, not definitive** — expand `bench/fixtures.json` for a stronger signal.
- The 35B got a slightly unfair shake (Ollama + thinking-on vs llama.cpp + grammar). A fair
  re-run (thinking disabled, grammar-constrained) is a TODO.
- Raw scores live in the workspace at `.context/bench-results.tsv`.
