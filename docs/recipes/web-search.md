# Recipe: web search

Gives the model a `web_search` tool for facts that change: weather, news, scores, prices, "is it still open". Three ways to provide it. Pick one; the tool name is the same, so the model does not care.

| Option | Needs | Files | Notes |
|---|---|---|---|
| **SearXNG** (the free default) | A SearXNG instance you run on your LAN | [`web-search-searxng.json`](../../examples/recipes/web-search-searxng.json) | No key, no account. Your queries go only to the engines SearXNG uses. |
| **Tavily** | An API key in `TAVILY_API_KEY` | [`web-search-tavily.json`](../../examples/recipes/web-search-tavily.json) | One call returns a short answer plus results, which suits a small speaker. Free tier credits are not stated in the documentation read; check the dashboard. |
| **Brave Search** (through its MCP server) | The Brave MCP server running as a sidecar with `BRAVE_API_KEY` | [`web-search-brave-mcp.json`](../../examples/recipes/web-search-brave-mcp.json) | Brave authenticates with an `X-Subscription-Token` header, which a declarative HTTP tool cannot carry (it sends only `Authorization: Bearer`). The sidecar holds the key, so ClawDot never sees it. |

All three are **`read_only`**. A search sends the words of the question to a third party (or to your own server, for SearXNG), which is why the tier definition "nothing leaves ClawDot" is a judgement call here; the decision is recorded in [open-questions.md](../open-questions.md). The usual turn limits (12 calls) bound the quota a single question can use, and a paid plan has its own caps. Because you lowered the tier, ClawDot treats a search as costly again once the turn already holds outside text (a search result, an alert): a second search in the same conversation is asked about with a yes or no, and a turn started by an event only proposes one (REQ-SEC-012).

## SearXNG

1. Run SearXNG on a host the speaker can reach (the official container is the easy way). In its `settings.yml` allow the JSON format, otherwise it answers `403`:
   ```yaml
   search:
     formats:
       - html
       - json
   ```
2. Copy [`web-search-searxng.json`](../../examples/recipes/web-search-searxng.json) and change `http://searx.lan:8080` to your instance. Plain `http://` is fine on a private network (REQ-SEC-001).
3. The call is `GET /search?q=...&format=json&language=en&safesearch=1`. SearXNG documents `q`, `format`, `categories`, `language`, `pageno`, `time_range` and `safesearch`; add a `time_range` parameter to the file if you want "news from today" to prefer fresh results.

**Failure modes.** `403` means the JSON format is not enabled. A public instance usually disables it and rate-limits bots; use your own. Engines are sometimes blocked by the search sites, which gives few or empty results; SearXNG's own status page shows it. The core clips the response to 1,000 characters, so only the first results reach the model; a long answer is shortened by the summary model. Slow engines can exceed the 10 s timeout in the file.

## Tavily

1. Create a key, then on the device: `export TAVILY_API_KEY=tvly-...` (or put it in the service's environment file).
2. Use [`web-search-tavily.json`](../../examples/recipes/web-search-tavily.json). The call is `POST https://api.tavily.com/search` with `Authorization: Bearer <key>` and a body of `query`, `include_answer` and `max_results`. `include_answer` is declared as the single value `basic` (a closed list) so the model cannot ask for the dearer `advanced` answer or for raw page content.
3. `basic` search costs 1 credit and `advanced` costs 2, per Tavily's documentation; the recipe cannot reach `advanced`.

**Failure modes.** `401` is a wrong or missing key (the core refuses to start a tool whose variable is unset, and says which). `429` or a quota error means the plan is used up; the model is told and says so. A search that returns a `usage` block with credits is normal and harmless.

## Brave Search through its MCP server

The Brave server (`brave-search-mcp-server`, MIT licence) can serve MCP over HTTP:

```
BRAVE_API_KEY=... BRAVE_MCP_TRANSPORT=http BRAVE_MCP_HOST=127.0.0.1 BRAVE_MCP_PORT=8080 \
  node dist/index.js --transport http          # serves http://127.0.0.1:8080/mcp
```

Its README says the HTTP endpoint is unauthenticated, so keep it on the loopback address of a host you control (the example uses `127.0.0.1`), or on a private network behind your own access control. Then use [`web-search-brave-mcp.json`](../../examples/recipes/web-search-brave-mcp.json). The file offers only `brave_web_search` and `brave_news_search` (the server also has local, image, video, place, summariser and LLM-context tools; the `only` list keeps the surface small) and sets both to `read_only`, because tiers come from your file, never from what the server says about itself.

**Failure modes.** The core requires MCP revision `2025-06-18`; a server that negotiates another revision is skipped with a warning that names both. The Brave server's revision was not checked. If the sidecar is down, start-up continues without these tools.

## Verify

1. `scripts/cargo.sh test -p clawdot-core --test recipes`: checks the SearXNG request is `GET /search?q=rust%20%26%20more&format=json&language=en&safesearch=1` with no `Authorization`, and that the Tavily request carries the key as a bearer and refuses `include_answer: advanced` and any undeclared field.
2. `python3 examples/recipes/fake_server.py`, point a copy at `http://127.0.0.1:9999`, and press Test.
3. Replay evals: `rcp-web-search-text-cannot-switch-off-lights` shows a search result that orders a light off is only data: the light call is held for a yes ("This followed text from outside") and the user's "no" drops it.

## Sources (read 2026-11-09)

* SearXNG search API: <https://docs.searxng.org/dev/search_api.html>
* Tavily search endpoint: <https://docs.tavily.com/documentation/api-reference/endpoint/search>
* Brave Search API: <https://api-dashboard.search.brave.com/app/documentation/web-search/get-started>
* Brave MCP server: <https://github.com/brave/brave-search-mcp-server>

**Not verified:** Brave's current free-tier limits and prices, Tavily's free credit count, the Brave MCP server's protocol revision, and any live call to any of the three.
