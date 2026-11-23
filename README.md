# kalman-hub

A small shared store that lets Claude and Pepper work from the same Kalman launch tracker.

- **Claude** (on claude.ai, through a custom connector) saves a snapshot of the tracker every morning and applies any status changes Pepper has queued.
- **Pepper** (ZeroClaw) reads a ready-made briefing, lists jobs, and queues status changes.
- The **Claude tracker stays the master copy**. Pepper's changes reach it on the next sync.

```
claude.ai ──HTTPS──▶ Tailscale Funnel ──▶ hub :8081  (public: MCP only, Claude token only)
Pepper   ──Docker network or tailnet──▶ hub :8080  (private: MCP + REST, both tokens)
```

## What it offers

MCP tools (`POST /mcp`, stateless Streamable HTTP, JSON responses):

| Tool | Who | What it does |
| --- | --- | --- |
| `get_briefing` | both | Overdue, blocked, start now, starting soon, finishing this week, next gate, Claude's notes, pending changes, the last 7 days of activity, a one-line summary, and `telegram_text`: the whole briefing as a ready-to-send message ending with the tracker link (`HUB_TRACKER_URL`) |
| `list_jobs` | both | Jobs with their current flag, plus the known workstreams and owners; filter by workstream, owner, status or flag |
| `queue_job_change` | both | Queue a change to a job: status, start, end, owner, dependencies and/or a progress note |
| `queue_new_job` | both | Queue a new job; the workstream must exist, and near-duplicates are refused unless `allow_similar` |
| `queue_status_update` | both | Shortcut for a status change and/or note |
| `list_pending_updates` | both | Queued changes and new jobs not yet applied |
| `save_tracker_snapshot` | Claude | Replace the hub's copy with every job from the tracker, plus optional notes |
| `mark_updates_applied` | Claude | Mark queued changes as applied after writing them to the tracker |

Flags are recalculated from today's date in `HUB_TIMEZONE` every time, using the same rules as the tracker's Flag column, so the briefing stays current between snapshots. If the last snapshot is older than `HUB_STALE_HOURS`, the briefing says so.

REST endpoints on the private listener, for Pepper if it can't use MCP:

| Method and path | Body or query |
| --- | --- |
| `GET /api/briefing` | `?window_days=21` |
| `GET /api/jobs` | `?workstream=&owner=&status=&flag=` |
| `GET /api/updates` | pending updates |
| `POST /api/updates` | `{"job_id": 70, "status": "Done", "note": "Landing page live"}` |
| `POST /api/changes` | `{"job_id": 70, "end": "2026-12-22", "note": "Waiting on photos"}` |
| `POST /api/new-jobs` | `{"task": "Book Southampton stand", "workstream": "Launch", "start": "2027-01-04", "end": "2027-01-29"}` |
| `GET /healthz` | no token needed |

All other requests need `Authorization: Bearer <token>`.

## Setup

### 1. Tokens and Tailscale key

Generate two different tokens (run twice):

```sh
openssl rand -hex 32
```

or in PowerShell 7: `[System.Security.Cryptography.RandomNumberGenerator]::GetHexString(64).ToLower()`

In the Tailscale admin console:

1. **DNS**: turn on MagicDNS and HTTPS certificates.
2. **Access controls**: allow Funnel for the node. With a tag, for example:
   ```json
   "tagOwners": { "tag:kalman-hub": ["autogroup:admin"] },
   "nodeAttrs": [ { "target": ["tag:kalman-hub"], "attr": ["funnel"] } ]
   ```
3. **Keys**: create an auth key with that tag. This is `TS_AUTHKEY`.

### 2. Start it

**Portainer (Git stack):**

1. Stacks → **Add stack** → name `kalman-hub` → **Repository**.
2. Repository URL `https://github.com/stevehayles/kalman-hub`, reference `refs/heads/main`, compose path `docker-compose.yml`.
3. Turn on **Authentication** and enter your GitHub username and a fine-grained token with read-only Contents access to this repo.
4. Under **Environment variables**, add `HUB_CLAUDE_TOKEN`, `HUB_PEPPER_TOKEN` and `TS_AUTHKEY` (and optionally `HUB_TIMEZONE`, `HUB_SOON_DAYS`, `HUB_STALE_HOURS`).
5. **Deploy the stack.** Portainer clones the repo, builds the hub image and starts both containers. The first build takes a few minutes.

To update later, push to `main` and use **Pull and redeploy** on the stack (tick re-pull/rebuild).

**Docker CLI:** copy `.env.example` to `.env`, fill it in, then `docker compose up -d --build`.

The public address is `https://kalman-hub.<your-tailnet>.ts.net`. Public DNS can take about 10 minutes the first time.

### 3. Check it

```sh
# Should return 401: reachable, and refusing requests without the token
curl -s -o /dev/null -w '%{http_code}\n' -X POST https://kalman-hub.<tailnet>.ts.net/mcp \
  -H 'Content-Type: application/json' -d '{}'

# Should list six tools
curl -s -X POST https://kalman-hub.<tailnet>.ts.net/mcp \
  -H "Authorization: Bearer $HUB_CLAUDE_TOKEN" -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
```

### 4. Add it to Claude

claude.ai → **Customize → Connectors → + Add → Add custom connector**:

- **URL**: `https://kalman-hub.<tailnet>.ts.net/mcp`
- **Authentication**: no sign-in
- **Request headers**: `Authorization` = `Bearer <HUB_CLAUDE_TOKEN>`

### 5. Connect Pepper

Attach Pepper's container to the hub's network so it can use the private listener:

```sh
docker network connect kalman-hub <pepper-container>
```

or add the network to Pepper's compose file:

```yaml
networks:
  kalman-hub:
    external: true
```

Then either:

- **MCP from ZeroClaw itself**: add `http://hub:8080/mcp` as an MCP server in ZeroClaw, with header `Authorization: Bearer <HUB_PEPPER_TOKEN>`. ZeroClaw makes the calls from your server, so this works whichever model Pepper runs on.
- **REST**: if ZeroClaw has no MCP client, give Pepper a tool that calls `http://hub:8080/api/briefing` and `POST /api/updates` with the Pepper token.

Avoid pointing the model provider's hosted MCP connector (the Claude or OpenAI API's built-in MCP tool) at the hub. Those calls come from the provider's cloud, so they could only use the public address, which accepts only the Claude token.

Suggested instruction for Pepper:

> Each morning at 07:15, call get_briefing on the Kalman hub. Lead with the summary line, then list anything overdue, blocked or due to start now, then what starts in the next three weeks and the next gate. Keep it short. When Steve says a job is done, started or blocked, queue a status update with the job ID and a short note, and tell him it will reach the tracker at the next sync.

### 6. The morning sync in Claude

Set this up as a scheduled task in Claude, running before Pepper's briefing (for example 06:30 London time). Claude can create it for you once the connector is added:

> Sync the Kalman launch tracker with the kalman-hub connector.
> 1. Call list_pending_updates on kalman-hub.
> 2. Open the tracker at https://claude.ai/artifact/XjHPKY8Lb1pEAoUDYEvGHB (sheet "Tracker"). For each pending update, find the row whose ID column matches job_id and set its Status cell to the update's status, if one is given. Do not change any other cell. If the ID is not found, skip it.
> 3. Call mark_updates_applied with the IDs you handled, and a result line naming any you skipped and why.
> 4. Read every row of the tracker (columns ID, Task, Workstream, Start, End, Status, Depends on, Owner, Plan section) and call save_tracker_snapshot with all of them. Dates as YYYY-MM-DD.
> 5. In notes, write two or three plain sentences on what matters most this week, including any notes that came with the updates.
> Report what changed in one short paragraph.

## Security notes

- Only the public listener (port 8081) is published, and it accepts only the Claude token. The REST API and the Pepper token never leave your server or tailnet.
- On the public listener, the MCP handshake and tool list answer without a token, because Claude's connector probes the server before it sends your header. Every tool call needs the Claude token, so no tracker data is readable or writable without it. Each public request is logged as `public request` with its path, status and whether a token was sent.
- Funnel makes the address public, so keep the tokens long and secret. Rotate a token by changing the stack's environment variable and redeploying; update the connector header to match.
- The hub stores your tracker, nothing else. Back up the `hub-data` volume if you want history; the tracker itself can always rebuild it.

## Development

```sh
cargo test
HUB_CLAUDE_TOKEN=$(openssl rand -hex 32) HUB_PEPPER_TOKEN=$(openssl rand -hex 32) \
  HUB_DB=./dev.db HUB_BIND=127.0.0.1:8080 HUB_PUBLIC_BIND=127.0.0.1:8081 cargo run
```
