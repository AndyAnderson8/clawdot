# R-06: Name check ("ClawDot")

Date: 2026-11-07 · Feeds: ADR-0007 decision timing, README, repo name

## Findings

- A search for "clawdot" found **no repository by that exact name**.
- The closest collision is the **Clawdbot → Moltbot → OpenClaw** project. Press coverage reports that Anthropic's legal team asked for a rename because **"Clawd" is too close to "Claude"**; the project changed names in January 2026 ([Business Today](https://www.businesstoday.in/technology/news/story/the-lobster-sheds-its-shell-for-the-third-time-as-clawdbot-becomes-openclaw-513650-2026-01-30), [MorphLLM](https://www.morphllm.com/clawdbot)). Coverage also reports typosquatting and scam activity around the old names. Details of the project's current governance are conflicting and unverified.
- "ClawDot" begins with the string "Clawd", which was the specific point of that dispute, and it sits in the same search neighbourhood as a well-known project family.

## Assessment (not legal advice)

1. **Trademark risk:** an open-source voice assistant in an AI-adjacent space whose name begins with "Clawd" invites the same objection. It is cheap to avoid now and costly after a public release.
2. **Discoverability and confusion:** search results already show OpenClaw/Clawdbot, and users could mistake the project for part of that ecosystem or for an Anthropic product.
3. **Scam surface:** the Clawdbot rename produced impersonation domains and handles. A name near it inherits some of that noise.

## Recommendation

Choose a different name **before the first public commit, domain, or package registration**. Checklist for a candidate: no GitHub/crates.io/domain collision, no resemblance to a vendor's marks, pronounceable by voice (the product is spoken!), and not itself a wake word that triggers on its own name.

The repository directory name can stay as a working title; every doc already says "working title". Renaming later is a find-and-replace, but only before outsiders depend on it.
