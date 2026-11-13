# Let an agent speak and ask you things (the ClawDot MCP server)

An agent harness that is an MCP client, such as Hermes Agent, can use ClawDot's speaker through two tools: `announce` (say a short message) and `ask_user` (ask you a question out loud and get your answer back). It is the "design 2" link for a harness: the harness stays the doer, ClawDot stays the voice and the safety checks. Requirements: REQ-EXT-011 to REQ-EXT-018 ([spec 0001](../specs/0001-product-requirements.md)); the protocol description is in [architecture/mcp.md](../architecture/mcp.md).

Status: built and tested in-process with a fake MCP client. **Not yet run against a live Hermes Agent**; the configuration below is written from its documentation and may need adjusting.

## What the server is

* One endpoint, `POST /mcp`, on the same port as the portal and the other webhooks (default `http://clawdot.local:3335/mcp`). Model Context Protocol over Streamable HTTP, revision `2025-06-18`, stateless, plain JSON replies. There is no stdio mode, no event stream and no login page.
* **Exactly two tools** and nothing else: no resources, no prompts, no way to read or change ClawDot's settings, notes, timers or tools.
* **An endpoint token and nothing else gets in.** Use the same `cdt_...` tokens as `/api/announce`. An administrator session is not accepted and there is no open mode. One endpoint per agent lets you mute or revoke one without the others (`clawdot --create-endpoint hermes:generic`, the token is printed once).

## Point a client at it

Create the endpoint and keep its token in an environment variable on the machine that runs the agent, never in a file you commit.

Hermes Agent reads MCP servers from `mcp_servers:` in its `config.yaml` (HTTP servers take a `url` and `headers`; see the MCP page of its documentation for how it expands environment variables, which is unverified here):

```yaml
mcp_servers:
  clawdot:
    url: "http://clawdot.local:3335/mcp"
    headers:
      Authorization: "Bearer cdt_REPLACE_ME"   # or your client's way to read it from the environment
    tools:
      include: [announce, ask_user]
```

Hermes names the tools `mcp_clawdot_announce` and `mcp_clawdot_ask_user`. Any other MCP client that supports Streamable HTTP and a custom header works the same way. A quick check without any client:

```sh
curl -sS -X POST "$CLAWDOT_URL/mcp" \
  -H "Authorization: Bearer $CLAWDOT_TOKEN" -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
```

## The tools

**`announce`**: `text` (required, at most 500 characters), `priority` (`low`, `normal` default, `urgent`). One way: nobody can reply to it. The result is `{"queued":true,"id":7}`, or `{"queued":false,"filtered":"muted"}` when the endpoint's filter dropped it.

**`ask_user`**: `question` (required, at most 500 characters), `timeout_secs` (1 to 300, default 60). ClawDot says the question when the speaker is free (as "From hermes: ..."), then opens the follow-up window so you can answer without the wake word. The call waits for your answer and returns:

| `status` | Meaning |
|---|---|
| `answered` | You answered (ClawDot reads your words back and waits for your yes before sending them, as it does for any reply to an agent; that spoken step has not been run on a real speaker yet). `answer` holds your words (speech-to-text, so it can be wrong), flattened to one line and clipped to 500 characters, with `untrusted: true`. |
| `acknowledged` | You said "got it" or similar, with no words. |
| `dismissed` | You dismissed the question. |
| `no_answer` | Nobody answered in time, or the question was never spoken (busy speaker, quiet hours). Not an error. The question is withdrawn and is not spoken late. |

A tool failure is a normal MCP result with `isError: true` and a one-line reason: bad arguments, rate limited, muted, no speaker connected, or a question already waiting (one per endpoint, four in all).

## What ClawDot guarantees, and what it does not

* **The answer is data, never a yes.** It goes to the asking call and to nobody else: not to a coding agent's reply stream, and it can never confirm something ClawDot is holding for your "yes". Your harness should do the same: **do not treat an `ask_user` answer as approval for a dangerous action.** Keep the harness's own approval system on for anything that acts. The answer comes back as speech-to-text, a guest or a television can say words too, and it is labelled `untrusted` for that reason.
* **The text is untrusted.** Over 500 characters is refused. Markup, links and control characters are removed, secrets are redacted, and the speaker named is always the endpoint's own name, so an agent cannot announce itself as someone else.
* **Your controls win.** Mute, minimum priority, quiet hours (only `urgent` is spoken; a question is not asked, so it ends as `no_answer`) and the endpoint's burst limit all apply, exactly as for `/api/announce`. `urgent` is the one way through quiet hours: give a harness an endpoint you are happy to be woken by, or filter it with a minimum priority.
* **It is audited.** Each call is in the audit log (`GET /api/audit`) with the trigger `mcp:<endpoint>` and the tool `announce` or `ask_user`. How a question ended is recorded; your answer and the token are not.
* **It covers what the agent says, not what it does.** ClawDot's ask-out-loud checks apply to ClawDot's own tools. An action the harness takes with its own tools is governed by the harness. The agent-harness research note (link designs 1 to 3) covers that gap.

## Failure modes

| You see | Cause |
|---|---|
| HTTP `401` | Missing, wrong or revoked token, or an administrator session token was used. |
| HTTP `429` | Too many failed attempts from anywhere (shared limit); wait a minute. |
| `isError` "rate limited" | The endpoint's burst limit; slow down. |
| `isError` "no speaker is connected" | No satellite or console is connected to ClawDot right now. |
| `isError` "the question was not asked: muted" | The endpoint is muted or filters this priority. |
| `no_answer` every time | The speaker is busy or in quiet hours, or the timeout is too short for the question to be spoken and answered. |
