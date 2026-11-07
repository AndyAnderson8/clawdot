# Development Process

This document is the single source of truth for how work is planned, built, reviewed, and shipped. It is intentionally prescriptive. If the process is wrong, change it through a PR and an ADR (see [ADR-0004](../adr/0004-process-kanban-spec-driven.md)), not by quietly diverging.

## 1. Principles

1. **Specs before code.** Behaviour is defined in an accepted spec before implementation begins.
2. **Everything traces.** Requirement ID → spec → issue → PR → test. If you can't trace it, it's not done.
3. **Small and continuous.** Small changes, short-lived branches, `main` always releasable.
4. **The budget is a feature.** The resource budget ([spec 0002](../specs/0002-resource-budget.md)) is enforced by CI, not by good intentions.
5. **Decisions are written down.** Significant choices become ADRs. Chat and memory don't count.
6. **Flow over velocity.** We manage work-in-progress and cycle time, not story points.

## 2. Why Kanban + spec-driven

- **Kanban** suits a small team or solo maintainer with variable-sized work: no sprint ceremonies, continuous delivery, explicit WIP limits, and flow metrics that expose bottlenecks.
- **Spec-driven development** suits this project because it has a hard external contract (the satellite protocol, third-party ports, an enforced resource budget) and must be implementable by people who aren't us. A written spec is the product for porters.
- They combine cleanly: specs are the *what*, Kanban is the *flow of work that realises it*.

## 3. Work item hierarchy

| Type | Definition | Lives as |
|---|---|---|
| **Milestone** | A releasable increment with exit criteria (see roadmap) | GitHub Milestone |
| **Epic** | A capability spanning multiple features, tied to one or more specs | Issue, label `type:epic` |
| **Feature** | A user-visible behaviour that satisfies specific requirement IDs | Issue, label `type:feature` |
| **Task** | A technical unit of work ≤ 3 working days | Issue, label `type:task` |
| **Bug** | A deviation from an accepted spec | Issue, label `type:bug` |
| **Spike** | A time-boxed investigation producing knowledge, not product code | Issue, label `type:spike` |
| **Chore** | Maintenance with no behavioural change | Issue, label `type:chore` |

Anything larger than 3 working days must be split before it enters *In Progress*.

## 4. The board

Tool: **GitHub Projects** (board view), linked to the repo so issues, PRs, and milestones stay in one place.

| Column | Meaning | WIP limit | Exit policy |
|---|---|---|---|
| **Backlog** | Captured, not yet refined | none | Meets Definition of Ready |
| **Ready** | Refined and ordered; top is next | 5 | Pulled by someone with capacity |
| **In Progress** | Actively being built (branch exists) | **2 per person** | PR opened and CI green |
| **In Review** | PR open, awaiting review | 3 | Approved and merged |
| **Verify** | Merged; acceptance criteria checked against the build | 3 | Criteria demonstrably met |
| **Done** | Meets Definition of Done | none | none |
| **Blocked** | Waiting on an external dependency (flagged, with reason) | none | Unblocked, returns to the prior column |

Rules:
- **Pull, don't push.** Take work from the top of *Ready*.
- **Stop starting, start finishing.** If WIP is at limit, help finish something (review, unblock) before starting anything new.
- A *Blocked* item must state what it is waiting on and who owns the unblock.
- Reorder *Ready* by priority, never by who is available.

### Classes of service

| Class | Label | Policy |
|---|---|---|
| Expedite | `cos:expedite` | Security or data-loss issue. May exceed WIP limit. Max one at a time. |
| Standard | (default) | Normal pull order. |
| Spike | `type:spike` | Time-boxed (stated in the issue). Output is a written finding or ADR. |

### Metrics (reviewed monthly)
Cycle time (In Progress → Done), throughput (items/week), WIP age, and blocked-time. Used to adjust WIP limits and spot bottlenecks, not to evaluate people.

## 5. Spec-driven workflow

```
Idea → Spike (optional) → Spec (Draft) → Spec Review → Spec (Accepted)
     → Epics/Features/Tasks created → Build → Verify → Spec (Implemented)
```

1. **Capture.** Open an issue (idea or feature request).
2. **Spec.** For anything that changes externally observable behaviour, the protocol, the budget, or security posture, write or amend a spec in `docs/specs/` using the [template](../specs/_template.md). Specs use requirement IDs (`REQ-VOI-001`) and RFC 2119 keywords (MUST/SHOULD/MAY).
3. **Review.** Spec PRs are reviewed like code. A spec must be *Accepted* (merged with status `Accepted`) before feature work on it starts.
4. **Decompose.** Create issues referencing requirement IDs. Each feature lists its acceptance criteria, copied or derived from the spec.
5. **Build** (see §6).
6. **Verify.** Acceptance criteria are checked against the merged build. Tests cite the requirement IDs they cover.
7. **Close the loop.** When all requirements in a spec are covered by passing tests, the spec moves to `Implemented`. Spec changes after Acceptance go through a PR and, if they alter a contract, a version bump.

Changing code without a matching spec change (when behaviour changes) is a defect in the PR.

## 6. Build procedure (per work item)

1. Confirm the item meets the **Definition of Ready**.
2. Create a branch: `<type>/<issue#>-<slug>` (see [git workflow](git-workflow.md)).
3. **Write tests first or alongside**, tagged with requirement IDs.
4. Implement. Keep commits small and [Conventional](git-workflow.md#commit-messages).
5. Run the local gate: `just check` (fmt, clippy, tests, `cargo deny`). The budget check runs in CI.
6. Open a PR using the template. Link the issue and requirement IDs.
7. Respond to review; keep the branch rebased on `main`.
8. Squash-merge when approved and CI is green.
9. Move to *Verify*; confirm acceptance criteria; move to *Done*.

## 7. Definition of Ready

An item may enter *Ready* only if:
- [ ] It links to an **Accepted** spec and lists requirement IDs (or is a spike/chore with a stated reason).
- [ ] Acceptance criteria are written and testable.
- [ ] It is ≤ 3 working days (otherwise split).
- [ ] Dependencies are identified and not blocking.
- [ ] Budget impact is noted (RAM, binary size, CPU) if non-trivial.

## 8. Definition of Done

An item is *Done* only if:
- [ ] Acceptance criteria are met and verified.
- [ ] Tests added/updated, citing requirement IDs; all pass.
- [ ] CI is green, including lint, security audit, the **resource budget gate**, and the armv7 cross-build.
- [ ] No new `unsafe` without a documented safety comment and reviewer sign-off.
- [ ] Public APIs and protocol changes are documented; spec status updated.
- [ ] `CHANGELOG.md` updated under *Unreleased* for user-visible changes.
- [ ] ADR written if a significant decision was made.
- [ ] Reviewed and approved by someone other than the author (once there is more than one maintainer; see §10).
- [ ] Merged to `main` via squash; branch deleted.

## 9. Releases

- **Cadence:** release when a milestone's exit criteria are met, not on a calendar.
- **Versioning:** [SemVer](https://semver.org). Pre-1.0, minor versions may break. The **satellite protocol** is versioned independently (`protocol vMAJOR.MINOR`) and its compatibility rules are in its spec.
- **Process:** release branch is not used; tag `vX.Y.Z` on `main`. CI builds artifacts for all targets, generates checksums and an SBOM, and attaches them to a GitHub Release with notes derived from `CHANGELOG.md`.
- **Hotfix:** branch from the release tag, fix on `main` first, cherry-pick.

## 10. Roles

| Role | Responsibility |
|---|---|
| **Maintainer** | Owns specs and roadmap, merges PRs, cuts releases, triages. |
| **Contributor** | Proposes and implements via issues and PRs. |
| **Reviewer** | Any maintainer or invited contributor. |

While there is a single maintainer, PR self-approval is allowed **only** after CI is green and a 24-hour cooling-off period for non-trivial changes. The PR template still gets filled in. The goal is an audit trail and discipline, not ceremony. This rule is removed when a second maintainer joins.

## 11. Cadences (lightweight)

| Cadence | Activity | Output |
|---|---|---|
| Continuous | Pull from *Ready*, ship | Merged PRs |
| Weekly (30 min) | Triage Backlog, reorder Ready, review blocked items | Updated board |
| Per milestone | Retrospective: what slowed us, what to change | Notes in `docs/process/retros/` |
| Monthly | Flow-metric review, WIP-limit adjustment, dependency updates | Board and process changes via PR |

## 12. Process change control

This process evolves. Propose changes via PR to this file; record significant ones as ADRs.

## 13. Proportionality (solo-maintainer mode)

A process that is too heavy gets skipped, which is worse than a lighter one that is followed. While there is a single maintainer, these items are **dormant** and activate when the first external contributor joins:

| Dormant until first external contributor | Active from day one |
|---|---|
| DCO sign-off enforcement | Specs, requirement IDs, and ADRs |
| Monthly flow-metric review | Trunk-based flow with squash merge |
| Per-milestone retrospective files | CI gates (fmt, clippy, tests, deny/audit, cross build, budget) |
| Review cooling-off rule (§10) | Conventional Commits on PR titles |
| Required second approver | Definition of Done and the traceability report |

Scope control: once a spec is Accepted, new scope enters only via a spec amendment and a roadmap change (see [future-scope.md](../future-scope.md)), never directly into a milestone.
