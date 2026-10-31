# rmx-backend-probe

A small learning and diagnostic project for **Blade of Grass**, an open-source
robotic lawn mower. It rehearses the full production workflow of the future
`rmx-backend` (Rust) and `rmx-client` (React + Bun) on a toy problem:

> A web page with a live table `records(time, connection_type, value)` and full
> CRUD over a selectable transport (**HTTPS REST** or **WSS**), plus the same
> CRUD for devices over **MQTT** (like the LTE modem probe in
> `rmx-mower-modem-probe`). Every change is logged to a TimescaleDB hypertable
> and pushed live to every open page, whatever transport made it.

The same stack as production: a Rust workspace (axum, sqlx, rumqttc, tokio), a
TypeScript + React client on Bun's bundler, TimescaleDB, Mosquitto, GitHub
Actions, images on GHCR.

- **Docs:** [Backend-Probe](https://github.com/rmx-lt/blade-of-grass/wiki/Backend-Probe)
  on the wiki: how it works, wire protocol, login and sessions, tests, CI/CD and
  the deployment contract.
- **Work:** issues on the [Blade of Grass project](https://github.com/orgs/rmx-lt/projects/1).

## Local development

You need Docker Desktop, Rust (rustup), and Bun. No Node or npm.

```powershell
# 1. Settings (dev-only passwords), then TimescaleDB + Mosquitto.
Copy-Item .env.example .env
docker compose -f compose.dev.yaml up -d --wait

# 2. The server (runs migrations, listens on 127.0.0.1:8080).
cargo run -p probe-server

# 3. The client dev server, in a second terminal.
cd client
bun install
bun run dev
```

Create a login once (`cargo run -p probe-server -- create-user dev`, any
password of 12+ characters), then open <http://localhost:3000/probe/> and log in. Ports, MQTT testing and
the checks CI runs: see the wiki page.

## License

[Apache-2.0](LICENSE).
