## Summary
<!-- What changed and why. PR title must follow Conventional Commits: type(scope): summary -->

## Links
- Issue: Closes #
- Spec / requirement IDs: REQ-
- ADR (if any):

## Type
- [ ] feat  - [ ] fix  - [ ] spec  - [ ] docs  - [ ] refactor  - [ ] perf  - [ ] test  - [ ] build/ci  - [ ] chore

## Verification
<!-- How was this tested? Which tests cite which requirement IDs? -->

## Resource budget impact
- Binary size delta:
- Peak RSS delta:
- New dependencies (name, version, size, justification):

## Checklist
- [ ] Matches an **Accepted** spec (or this PR amends the spec)
- [ ] Tests added/updated and cite requirement IDs
- [ ] `just check` passes locally; CI is green (incl. `budget`, `cross-armv7`)
- [ ] No unbounded buffers/queues; no panics reachable from external input
- [ ] No secrets, raw audio, or PII in logs or fixtures
- [ ] Docs, `CHANGELOG.md` (Unreleased), and ADR updated where needed
- [ ] Commits signed and signed-off (DCO)
