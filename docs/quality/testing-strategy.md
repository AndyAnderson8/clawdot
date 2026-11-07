# Testing Strategy

Quality is verified at several levels, and every requirement is traced to at least one test (or an explicit inspection/analysis record).

## 1. Principles

1. **Requirements-driven.** Tests cite requirement IDs (`REQ-xxx`). CI produces a traceability report and fails on Must-requirements in *Implemented* specs without coverage.
2. **Deterministic by default.** The default suite uses fake providers, fake clocks, and recorded audio; no network, no real time.
3. **Fast feedback.** `just check` < 60 s locally; the full suite in CI < 10 min.
4. **Test the budget.** Resource limits are tests, not documentation.
5. **Hostile input is normal.** Every parser of external data is fuzzed.

## 2. Levels

| Level | Scope | Tooling | Runs |
|---|---|---|---|
| **Unit** | Functions, state machines, parsers | `cargo test`, `proptest` | every commit |
| **Component** | A crate with fakes (scheduler with a fake clock; session with fake providers) | `cargo test` | every commit |
| **Contract / conformance** | Satellite protocol: any satellite vs. a fake core and vice versa | `conformance` harness + JSON Schemas | every commit; published for porters |
| **Integration** | Full core + desktop satellite loopback with scripted providers | `cargo test` (integration) | every PR |
| **End-to-end** | Real binary, fake provider servers, recorded audio, HTTP calls | scripted harness | every PR |
| **Budget** | RSS, binary size, CPU on armv7 under qemu-user | `just budget` | every PR (gate) |
| **Soak** | 24 h+ idle and cycled-load run; heap growth check | scheduled CI | nightly/weekly |
| **Fuzz** | Protocol frames, webhook bodies, config, LLM tool-call JSON | `cargo-fuzz` | scheduled CI + on parser changes |
| **Real hardware** | Reference device or nearest equivalent | manual checklist | per release |

## 3. Voice-specific evaluation

Voice quality isn't binary, so it uses metrics against a versioned fixture set (kept small; large sets are fetched, not committed).

| Metric | Method | Target (provisional) |
|---|---|---|
| Wake-word false rejects | Positive utterance set across speakers/distances | ≤ 5% |
| Wake-word false accepts | ≥ 24 h of negative audio (TV, speech) | ≤ 1 per 10 h |
| Endpointing accuracy | Labelled utterances; clipped-speech rate | ≤ 2% clipped |
| End-to-end latency | Timestamped scripted runs against fake providers with injected latency; real providers measured separately, batch vs streaming STT | See REQ-NFR-002 |
| Fast-path matcher | Utterance corpus: correct slot extraction, clean no-match fallthrough, time bound | ≥ 98% on in-grammar set; ≤ 20 ms (NFR-111) |
| Tool-calling accuracy | Scripted prompts per candidate model via a provider; results recorded in the tested-models list | See REQ-AST-012 |

## 4. Test data and fixtures

- Audio fixtures are short, licensed for redistribution, and listed with provenance in `tests/fixtures/README.md`.
- Never commit real user recordings or real API keys.
- Provider fakes replay recorded HTTP exchanges (sanitised).

## 5. Quality gates (CI)

| Job | Check |
|---|---|
| `fmt` | `cargo fmt --check` |
| `clippy` | `cargo clippy --all-targets -- -D warnings` |
| `test` | unit, component, integration, conformance |
| `deny` | `cargo-deny` (licenses, bans, advisories) and `cargo-audit` |
| `cross-armv7` | build `armv7-unknown-linux-musleabihf` release |
| `budget` | size and qemu RSS/CPU gate |
| `trace` | requirement coverage report |
| `commitlint` | PR title follows Conventional Commits |
| `docs` | link check and spec template lint |
| `fuzz-smoke` | short fuzz run on changed parsers |

## 6. Code review checklist (reviewers)

- Does it match an Accepted spec? Requirement IDs referenced?
- Tests cover the behaviour and the failure paths?
- Any unbounded growth (queues, strings, history)?
- Any panic reachable from external input?
- New dependency justified, with size impact stated?
- Secrets, audio, or PII in logs?
- Docs, changelog, and ADR updated where needed?

## 7. Test-cost policy

1. **CI and default tests never call real providers.** They use fakes and recorded exchanges, so builds are deterministic, free, and safe on forks (no secrets available).
2. Real-provider tests are a separate, **opt-in** job (nightly or manual) that uses a dedicated key with a hard spend cap, short prompts, and a fixed budget per run.
3. Tool-calling evaluations (REQ-AST-012) run on demand and their results are committed to the tested-models list, not re-run on every PR.
4. Latency measurements against real providers are recorded as artifacts with the provider, region, and STT mode, never as pass/fail gates on PRs.
5. Real API keys never appear in fixtures, logs, or CI output.

## 8. Defect handling

- A bug is a deviation from an Accepted spec. If the spec is silent or wrong, fix the spec first.
- Every bug fix includes a regression test.
- Severity: **S1** (security, data loss, bricked device), **S2** (core feature broken), **S3** (degraded), **S4** (cosmetic). S1 uses the Expedite class.
