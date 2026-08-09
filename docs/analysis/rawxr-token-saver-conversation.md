# Analysis: the RAWXR / Teletubby / Token Saver conversation, from rtk's side

Source: a shared ChatGPT conversation ("Token Saver Git Description") in which a product
spec was drafted for a Cook Labs Inc. platform that would combine three existing tools —
JCode (an agent harness), jCodeMunch MCP (source-code retrieval), and rtk (terminal-output
compression) — into one "AI Context Operating System" sold on a percentage of verified
token savings.

This document records what that conversation actually proposes, checks its central factual
premise about rtk against this repository, and triages every rtk-facing ask into
already-shipped / worth-building / out-of-scope.

## 1. What the conversation contains

The chat starts with three plain questions — what do jcode, jCodeMunch and rtk each do,
how do they fit together, and who charges for what — and then escalates into a series of
progressively larger "paste this into Claude Code" master directives. The product is named
RAWXR in one pass, Teletubby in another, and Token Saver in a third; the content is
substantially the same each time.

The proposed platform has five components:

| Component | Role in the spec | Sourced from |
| --- | --- | --- |
| Agent Harness | plans, edits, runs tests, manages sessions | fork of JCode (MIT) |
| Source Intelligence Engine | AST indexing, symbol search, call graphs, test mapping | clean-room reimplementation, because jCodeMunch is not reusable |
| Terminal Intelligence Engine | compress command output before the model reads it | fork of rtk (Apache-2.0) |
| Token Optimization Engine | prompt dedup, exact + semantic cache, model routing | new |
| Wallet / Savings Ledger | verified-savings accounting and percentage billing | new |

The commercial model is the load-bearing idea: no subscription, no seat fee, a seven-day
free trial, then 15% / 10% / 5% of *verified* savings for Pro / Business / Enterprise, with
optimization halting when the prepaid wallet runs dry. The spec is emphatic and repeated
that savings must never be claimed without evidence, that money must not use floating
point, and that "up to 95%" must never be stated as a universal claim.

rtk appears in the spec only as section 9, "Terminal Intelligence Engine", plus scattered
integration notes. That section asks for three things rtk does not have today, discussed in
§4 below.

## 2. The central premise does not survive contact with `LICENSE`

The instruction accompanying this analysis was that rtk is our own, and that the
conversation's licensing caution about rtk therefore does not apply. That is only
half-true, and the half that is false is the expensive half.

`LICENSE` in this repository reads:

```
Copyright 2024 rtk-ai and rtk-ai Labs
```

`CLAUDE.md` describes the project as "a fork with critical fixes for git argument parsing
and modern JavaScript stack support". `Cargo.toml` still points `homepage` at
`https://www.rtk-ai.app`. This repository is a downstream fork of `rtk-ai/rtk`, Apache-2.0,
and the upstream copyright belongs to rtk-ai, not to us.

What is genuinely ours: this fork, its commit history, and the modifications in it. What is
not ours: the upstream copyright, and the "RTK" name and branding, which Apache-2.0 §6
explicitly declines to license.

So the conversation's treatment of rtk — fork it, use it commercially, keep the notices,
state your modifications, respect the trademark — was **correct**, and adopting the
"rtk is our own, the licensing section doesn't apply to us" framing would walk us into the
one part of the spec that was actually right. Apache-2.0 is permissive enough that this
costs us almost nothing to honour: we may fork, modify, sell, and embed rtk in a commercial
product. The obligations are notice-shaped, not permission-shaped.

### 2.1 The one real gap

Apache-2.0 §4(b) asks that modified files carry prominent notices stating that we changed
them. This repository redistributes binaries (releases, Homebrew) and does not state its
fork relationship anywhere a user will see it — `README.md` has no fork or attribution
line, only `CLAUDE.md` mentions it, and there is no `NOTICE` or `THIRD_PARTY_NOTICES.md`.

Recommended fix, in increasing order of cost:

1. Add a short provenance line to `README.md` and a `docs/PROVENANCE.md` recording the
   upstream repository, the license, the fork point, and the nature of our modifications.
2. Do **not** invent a `NOTICE` file. Upstream `rtk-ai/rtk` ships no `NOTICE` (verified: the
   path 404s), so §4(d) imposes no propagation duty on us; creating one would newly bind our
   own downstream redistributors for no benefit.
3. Leave the masthead alone. The branding question was already settled in PR #10
   ("rtk is just rtk"), and re-litigating it is not what a license obligation requires.

This is worth doing on its own merits, independent of whether any platform ever gets built.

## 3. What the conversation got wrong or thin

**The savings number cannot be verified the way the billing model needs.** The entire
commercial premise is charging a percentage of *verified* savings. rtk cannot supply that
figure. `src/core/tracking.rs` estimates tokens as `bytes / 4`; there is no tokenizer in
the binary. rtk measures bash-output bytes removed, which is a reliable ratio and an
approximate token count, and it has no visibility at all into what the model was actually
billed. A platform invoicing customers off that number would be charging against an
estimate while claiming verification. The spec's own rule — "never count unverified savings
as billable" — rules out using rtk as the billing meter. Real verification has to come from
provider usage reporting on the proxy side, not from the terminal filter.

**Percentage-of-savings billing needs a counterfactual that does not exist.** "Savings"
means the difference against what you would have spent otherwise. Nobody ran the
unoptimized branch. The spec gestures at this with a quality-validation stage but never
resolves the measurement problem.

**The three-name churn is a signal.** RAWXR, Teletubby, and Token Saver are the same
document with the branding swapped. The spec is a positioning exercise that has not yet
been narrowed to a build.

**Repo-facing history.** This repository already tried the adjacent version of this idea and
backed it out: `fd6c85f` added portal login and upload of savings aggregates "for metering",
`b92e3bd` rebranded the masthead to Cook Labs Token Saver, and PR #10 (`ae8b3ae`, `f522f5e`)
reverted both under the heading "rtk is just rtk". Anything that re-lands metering upload or
third-party branding in this repo is re-opening a closed decision.

## 4. Triage of the rtk-specific asks

Spec section 9 and the integration notes ask rtk for the following.

### Already shipped

- **Broad command coverage** (git, pytest, cargo, npm/pnpm, ruff, mypy, ESLint, docker,
  build tools, CI logs). rtk covers these and roughly a dozen more ecosystems.
- **Exit-code preservation.** Filters propagate the child's exit code via
  `std::process::exit`; this is a documented non-negotiable in `.claude/rules/rust-patterns.md`.
- **Graceful degradation.** The mandatory fallback pattern — emit the raw command output if
  the filter fails — already satisfies the spec's "never block the user" requirement.
- **Savings accounting.** `rtk gain` reports per-command reduction from the SQLite tracker,
  with the byte-estimate caveat in §3.
- **"Do not make rtk a required runtime dependency."** Nothing to do; rtk is a standalone
  binary that a consumer detects and shells out to.

### Genuinely missing, and worth building on rtk's own merits

- **A single global output mode** (`lossless` / `balanced` / `aggressive`). rtk has
  `--ultra-compact` as a global flag and a per-command `FilterLevel` on `rtk read`, but no
  one control that sets aggressiveness across every filter. This is a coherent feature
  request regardless of RAWXR and the cheapest of the three.
- **A documented "never hide" invariant.** The spec lists what compression must never drop:
  exit code, failed test names, error type and message, relevant stack frames, source paths,
  line/column numbers, expected-vs-actual values, compiler codes, changed files, security
  warnings, migration failures, dependency conflicts. rtk mostly honours this in practice,
  per filter, by convention. Writing it down as a shared contract with tests would make it a
  guarantee instead of a habit, and would catch regressions the current per-filter snapshot
  tests miss.
- **Structured output.** rtk emits filtered text; the spec wants typed events (failing test,
  file, line, expected, actual) that a model can consume in fewer tokens than prose. `rtk json`
  exists but compacts arbitrary JSON input — it is not a structured emitter for filter results.
  This is the largest of the three and the one most worth prototyping on a single filter
  (`cargo test` or `pytest`) before committing to a schema.

### Out of scope for this repository

Everything else in the spec — agent harness, source intelligence engine, semantic cache,
model router, OpenAI-compatible proxy, MCP server, wallet, savings ledger, dashboard,
Python/TypeScript SDKs, trial-abuse prevention, PostgreSQL migrations — belongs to a
different product. rtk is a single-binary, no-async, sub-10ms-startup terminal filter, and
those constraints are the reason it is fast. Building a billing platform inside it would
cost the properties that make it worth forking in the first place.

If that platform gets built, rtk's correct role is what the tail of the conversation
already concluded: an optional, detected-if-present external binary the platform shells
out to, not a vendored subsystem.

## 5. Recommendation

1. Close the attribution gap (§2.1). Cheap, correct, independent of everything else.
2. Do not re-land metering upload or third-party branding in this repo (§3).
3. If any part of the spec is to be built here, take the three items in §4 in order —
   global output mode, written never-hide contract, then structured output on one filter as
   a prototype. Each stands on its own without the platform.
4. Treat the savings-verification problem as unsolved before any percentage-of-savings
   pricing is promised to a customer (§3).
