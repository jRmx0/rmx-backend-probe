# rmx-backend-probe

A small learning and diagnostic project for **Blade of Grass**, an open-source
robotic lawn mower. It rehearses the full production workflow of the future
`rmx-backend` (Rust) and `rmx-client` (React + Bun) on a toy problem:

> A web page with a live table `records(time, connection_type, value)` and full
> CRUD over a selectable transport (**HTTPS REST**, **WSS**, or **MQTT** over
> WebSockets). Every change is logged to a TimescaleDB hypertable and pushed
> live to every open client, whatever transport made it.

The same stack as production: a Rust workspace (axum, sqlx, rumqttc, tokio), a
TypeScript + React client on Bun's bundler, TimescaleDB, Mosquitto, GitHub
Actions, images on GHCR.

## Contents

- [How it works](#how-it-works)
- [Repository layout](#repository-layout)
- [Local development](#local-development)
- [Tests and checks](#tests-and-checks)
- [CI/CD](#cicd)
- [Deployment](#deployment) (contract for the VPS side)
- [License](#license)

## How it works

```
browser ──HTTPS──▶ Caddy ──▶ probe-server :8080 ──▶ TimescaleDB
   │      /probe/api, /probe/ws        │  ▲
   │                                   ▼  │ mqtt:1883 (plain, internal)
   └──MQTT over WSS──▶ Caddy ──▶ Mosquitto
```

- **One records service** (`crates/probe-server/src/records.rs`) does every
  create/update/delete in a transaction that also appends to `record_events`,
  then broadcasts a `ChangeEvent` in-process (a tokio `broadcast` channel).
- **HTTPS**: REST under `/probe/api/records`.
- **WSS**: `/probe/ws`, JSON request/response with a `request_id`, plus
  server-pushed change events.
- **MQTT**: the server subscribes to `probe/req/+` and answers on
  `probe/res/<client-id>`; change events go to `probe/events/records`.
- **Live updates**: every change goes to every WebSocket and to the MQTT
  events topic. In the UI, events come over MQTT while MQTT is the selected
  (and connected) transport, and over the WebSocket otherwise, so each path is
  visibly tested.

### Database

Migrations: `crates/probe-server/migrations/` (sqlx, run at server start).

| Table | Kind | Columns |
|---|---|---|
| `records` | plain table (rows change in place) | `id`, `time` (created), `connection_type` (`https`/`wss`/`mqtt`, who created or last changed it), `value` (nullable), `updated_at` |
| `record_events` | hypertable, 1-day chunks | `time`, `connection_type`, `op` (`create`/`update`/`delete`), `record_id`, `value` |

`record_events` is compressed after 1 day and dropped after 7 days
(TimescaleDB background jobs). It has no foreign key, so deleted records keep
their history.

### Wire protocol

All types are defined once in Rust (`crates/probe-protocol`) and exported to
TypeScript with [ts-rs](https://github.com/Aleph-Alpha/ts-rs) into
`client/src/generated/`.

**Why ts-rs, and why committed:** the Rust types are the single source of truth,
and the compiler checks the client against them. The generated files are
committed so the client builds without a Rust toolchain (the Bun stage in the
Dockerfile, the client CI job). CI regenerates them and fails if the committed
copy is stale. After changing a type: `cargo test -p probe-protocol`, then
commit the changed files.

REST:

| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/probe/api/records` | | `RecordRow[]` (newest 500) |
| POST | `/probe/api/records` | `{"value": string \| null}` | 201 `RecordRow` |
| PUT | `/probe/api/records/{id}` | `{"value": string \| null}` | 200 `RecordRow` / 404 |
| DELETE | `/probe/api/records/{id}` | | 204 / 404 |

Errors: `{"code": "bad_request" | "not_found" | "internal", "message": "..."}`.
`value` is at most 1000 characters.

WSS and MQTT share one envelope:

```jsonc
// client → server
{ "request_id": "uuid", "request": { "op": "list" } }
{ "request_id": "uuid", "request": { "op": "create", "value": "hi" } }
{ "request_id": "uuid", "request": { "op": "update", "id": 7, "value": null } }
{ "request_id": "uuid", "request": { "op": "delete", "id": 7 } }

// server → client (MQTT: this object; WSS: wrapped as {"type":"response","response":{...}})
{ "request_id": "uuid", "result": { "status": "ok", "data": { "kind": "record", "record": { ... } } } }
{ "request_id": "uuid", "result": { "status": "error", "error": { "code": "not_found", "message": "..." } } }

// change event (MQTT events topic: this object; WSS: {"type":"event","event":{...}})
{ "op": "create", "record": { ... } }
{ "op": "delete", "id": 7, "connection_type": "mqtt" }
```

MQTT topics (`<prefix>` is `probe`):

| Topic | Direction | Payload |
|---|---|---|
| `<prefix>/req/<client-id>` | client → server | request envelope |
| `<prefix>/res/<client-id>` | server → client | response envelope |
| `<prefix>/events/records` | server → all | change event |
| `<prefix>/status/server` | server → all, **retained** | `online` / `offline` (last will) |

The client id travels in the topic (like the planned `mower/<id>/cmd`), not in
MQTT 5 properties, so the scheme works on MQTT 3.1.1 and on small device
clients, and a broker ACL can scope each login to its own topics. The server
answers only on `<prefix>/res/<id>` for the id it got the request on.

## Repository layout

```
Cargo.toml, rust-toolchain.toml   Rust workspace, pinned stable toolchain (edition 2024)
crates/probe-protocol/            wire types (serde + ts-rs), MQTT topic helpers
crates/probe-server/              the server: config, records service, http, ws, mqtt
  migrations/                     sqlx migrations
  tests/integration.rs            tests against real TimescaleDB + Mosquitto
.sqlx/                            offline query data for sqlx (no DB needed to build)
client/                           React 19 + TS on Bun (same toolchain as blade-terminal)
  public/index.html               HTML entry (Bun HTML import)
  server/index.ts                 dev server: HMR + proxy to probe-server
  scripts/build.ts                production build → dist/
  src/generated/                  ts-rs output (do not edit)
compose.dev.yaml, dev/mosquitto/  local/CI stack: TimescaleDB + Mosquitto
Dockerfile                        multi-stage: bun build + cargo build → distroless
.github/                          CI workflow, Dependabot
```

## Local development

You need Docker Desktop, Rust (rustup), and Bun. No Node or npm.

```powershell
# 1. Settings: dev-only passwords for the local stack.
Copy-Item .env.example .env

# 2. TimescaleDB + Mosquitto (localhost only).
docker compose -f compose.dev.yaml up -d --wait

# 3. The server (reads .env, runs migrations, listens on 127.0.0.1:8080).
cargo run -p probe-server

# 4. The client dev server, in a second terminal.
cd client
bun install
bun run dev
```

Open <http://localhost:3000/probe/>. For MQTT, log in as `probe-web` with the
`PROBE_WEB_PASSWORD` from `.env` (`probe-web-dev`).

Local ports (chosen so they don't clash with a Postgres or Mosquitto
installed on the PC):

| Port | What |
|---|---|
| 3000 | Bun dev server (client with hot reload) |
| 8080 | probe-server |
| 55432 | TimescaleDB (`postgres://probe:probe-dev@127.0.0.1:55432/probe`) |
| 11883 | Mosquitto, plain MQTT (the server) |
| 19001 | Mosquitto, MQTT over WebSockets (the browser) |

**How the dev server works.** `bun run dev` runs `client/server/index.ts`:
`Bun.serve` with an HTML import of `public/index.html`, so Bun bundles the app
on the fly with hot module reloading and Tailwind (`bun-plugin-tailwind` in
`bunfig.toml`). The same server proxies `/probe/api/*`, `/probe/health`,
`/probe/config.json` and the `/probe/ws` WebSocket to probe-server
(`PROBE_BACKEND_URL`, default `http://127.0.0.1:8080`). The browser sees a
single origin with the same paths as production, so there is no CORS and no
environment-specific URL in the client code. Only MQTT goes elsewhere: its URL
comes from `/probe/config.json`.

Stop the stack with `docker compose -f compose.dev.yaml down` (add `-v` to
also delete the database volume).

Run the production image against the dev stack:

```powershell
docker build -t rmx-backend-probe:local .
docker run --rm -p 8080:8080 `
  -e DATABASE_URL=postgres://probe:probe-dev@host.docker.internal:55432/probe `
  -e MQTT_HOST=host.docker.internal -e MQTT_PORT=11883 `
  -e MQTT_USERNAME=probe-app -e MQTT_PASSWORD=probe-app-dev `
  -e MQTT_WS_URL=ws://127.0.0.1:19001 `
  rmx-backend-probe:local
```

Then open <http://localhost:8080/probe/>.

## Tests and checks

| What | Command |
|---|---|
| Format | `cargo fmt --all --check` |
| Lint | `cargo clippy --workspace --all-targets --all-features -- -D warnings` |
| Unit tests (+ regenerates TS types) | `cargo test --workspace` |
| Integration tests (needs the compose stack and `.env`) | `cargo test -p probe-server --features integration-tests --test integration` |
| Client lint / types / tests / build | `cd client; bun run lint; bun run typecheck; bun test; bun run build` |

The integration tests start the real server on a random port; `#[sqlx::test]`
gives each test a fresh migrated database, and each test uses its own MQTT
topic prefix, so they run in parallel. They cover REST, WSS and MQTT CRUD,
cross-transport live events, health, `config.json`, the `record_events` log,
the retained server status and graceful shutdown.

**After changing a SQL query**, refresh the offline data (needs the stack):

```powershell
cargo install sqlx-cli --no-default-features --features postgres,rustls   # once
cargo sqlx prepare --workspace -- --all-targets --all-features
```

and commit `.sqlx/`. Docker and CI build with `SQLX_OFFLINE=true` and fail if
it is stale.

## CI/CD

`.github/workflows/ci.yml`, on every pull request and push:

1. **rust**: fmt, clippy (`-D warnings`), unit tests, TS types up to date.
2. **client**: `bun install --frozen-lockfile`, eslint, tsc, `bun test`, build.
3. **integration**: `compose.dev.yaml` up, integration tests.
4. **image**: after 1–3 pass, builds the Docker image (cached in the GitHub
   Actions cache). On pull requests it is only built. On `main` it is pushed
   as `sha-<short>` and `main`; on a `v*` tag as `X.Y.Z` and `X.Y`.
5. **deploy**: a disabled placeholder (see below).

Dependabot (`.github/dependabot.yml`) opens weekly updates for cargo, the
client (npm ecosystem: `package.json` + `bun.lock`), GitHub Actions and the
Dockerfile's base images.

## Deployment

This is the contract the VPS side (repo `rmx-vps`) can rely on.

**Image:** `ghcr.io/jrmx0/rmx-backend-probe`, public, linux/amd64.

| Tag | Meaning |
|---|---|
| `sha-<short>` | every push to `main` (deploy this one: immutable) |
| `main` | latest `main` |
| `X.Y.Z`, `X.Y` | release tags `vX.Y.Z` |

**Container:**

- Listens on **`0.0.0.0:8080`** (HTTP; TLS is Caddy's job). Publish no port;
  Caddy reaches it over the Docker network.
- Runs as uid **65532** (distroless `nonroot`), no shell. Needs no volumes.
- `HEALTHCHECK` built in (`/app/probe-server healthcheck` → `GET /probe/health`).
- Stops cleanly on SIGTERM (closes WebSockets, publishes MQTT `offline`).
- Memory: a few MB at idle; `mem_limit: 64m` is plenty.
- Networks: `backend` (database) and `edge` (Mosquitto, Caddy).

**Environment:**

| Variable | Required | Example (VPS) | Notes |
|---|---|---|---|
| `DATABASE_URL` | yes | `postgres://probe:…@probe-db:5432/probe` | migrations run at start. No TLS (internal network). |
| `MQTT_HOST` | yes | `mosquitto` | plain MQTT on the internal network |
| `MQTT_PORT` | no | `1883` | default 1883 |
| `MQTT_USERNAME` | yes | `probe-app` | ACL: `probe/#` |
| `MQTT_PASSWORD` | yes | (secret, server-side `.env`) | never logged |
| `MQTT_WS_URL` | yes for MQTT in the UI | `wss://mqtt.rmx.lt/mqtt` | the broker's WebSocket URL **as the browser sees it**, served in `/probe/config.json` |
| `RUST_LOG` | no | `info` | default `info` |
| `BIND_ADDR` | no | `0.0.0.0:8080` | already set in the image |
| `MQTT_CLIENT_ID` | no | `probe-server` | must be unique on the broker |
| `MQTT_TOPIC_PREFIX` | no | `probe` | |
| `DB_MAX_CONNECTIONS` | no | `5` | |
| `STATIC_DIR` | no | `/app/static` | already set in the image |

**Database:** TimescaleDB on Postgres 17 (`timescale/timescaledb:2.30.2-pg17`
or newer 2.x). The first migration runs `CREATE EXTENSION IF NOT EXISTS
timescaledb`, so the database user must own the database and either be a
superuser (the image's `POSTGRES_USER` is) or the extension must already exist.

**Routes the app serves itself** (Caddy proxies `api.rmx.lt/probe/*` as-is and
must **not** strip the `/probe` prefix):

| Path | What |
|---|---|
| `/probe` | 308 redirect to `/probe/` |
| `/probe/` and other unknown `/probe/*` | the client (static files, `index.html` fallback) |
| `/probe/api/records[/{id}]` | REST |
| `/probe/ws` | WebSocket (Caddy proxies upgrades by default) |
| `/probe/health` | `{"status":"ok","db":true,"mqtt":true,"version":"0.1.0"}`, 200 or 503 |
| `/probe/config.json` | `{"mqtt_ws_url":"wss://mqtt.rmx.lt/mqtt","mqtt_topic_prefix":"probe"}` |

**Browser MQTT:** the client connects with mqtt.js to `MQTT_WS_URL`
(production `wss://mqtt.rmx.lt/mqtt`: Caddy terminates TLS and proxies to a
Mosquitto `websockets` listener) as `probe-web`, typing the password into the
UI. It uses client ids `web-<12 hex>`, so the broker must not force
`use_username_as_clientid` on that listener if several tabs should work at once.
It needs read/write on `probe/#` (narrowest: write `probe/req/+`, read
`probe/res/+`, `probe/events/#`, `probe/status/#`).

**Deploy job:** the `deploy` job in `ci.yml` is a disabled placeholder. The VPS
session adds a restricted SSH key (forced command running one deploy script),
the repository secrets, and then enables the job.

## License

[Apache-2.0](LICENSE).
