# Deployment plan — Docker Compose + Caddy (auto-HTTPS)

Target: **DigitalOcean droplet** (any Docker host works the same).
Stack: the app container (single Rust binary + SQLite volume) behind a Caddy
container that terminates TLS with automatic Let's Encrypt certificates.

## Architecture

```
internet ──443/80──> caddy (auto-TLS, gzip, X-Forwarded-For)
                        │ reverse_proxy
                        ▼
                     app :8080 (axum)
                        │
                   app-data volume: anonbookrec.db (SQLite WAL) + .pepper
```

- Migrations are embedded in the binary and run on boot — no separate step.
- The rate limiter uses `X-Forwarded-For` (SmartIpKeyExtractor), which Caddy
  sets, so per-IP limits keep working behind the proxy.
- `SECURE_COOKIES=1` and `PUBLIC_BASE_URL=https://…` are set by compose.

## Files (in repo)

| File | Purpose |
|---|---|
| `Dockerfile` | Multi-stage: rust:1.96-slim build (dependency-layer cached) → debian-slim runtime, non-root user, static assets baked in |
| `compose.yaml` | `app` + `caddy` services, named volumes for DB and certificates |
| `deploy/Caddyfile` | Two-liner vhost: `{$DOMAIN} { reverse_proxy app:8080 }` |

## Rollout steps (DigitalOcean)

1. **Droplet**: create from the Marketplace **"Docker on Ubuntu"** image
   (Docker + compose preinstalled) — Basic / Regular, $6/mo (1 vCPU, 1 GB,
   25 GB) is plenty. Pick the region nearest your readers. Add your SSH key;
   disable password login.
2. **Firewall**: DigitalOcean Cloud Firewall (or `ufw`) allowing inbound
   22, 80, 443 only — 8080 stays internal to the compose network.
3. **DNS**: `A` record (and `AAAA` if the droplet has IPv6) for your domain →
   droplet IP. Either at your registrar or via DO's DNS (point nameservers at
   `ns1–3.digitalocean.com`). Low TTL (300) while setting up.
4. **Deploy** (as root or a docker-group user on the droplet):
   ```sh
   git clone <repo> && cd AnonBookRec
   echo "DOMAIN=books.example.com" > .env
   docker compose up -d --build
   ```
5. **Verify**:
   - `https://$DOMAIN` loads with a valid certificate (Caddy fetches it on
     first request; allow ~30 s).
   - Create a page, submit a book, check the sealed flow with the key.
   - `curl -s -o /dev/null -w '%{http_code}' http://$DOMAIN` → 308 redirect to HTTPS.
   - Burst 12 POSTs → 429 confirms rate limiting sees real client IPs.

## Operations

- **Upgrade**: `git pull && docker compose up -d --build` — migrations apply on
  restart; SQLite WAL tolerates the brief stop.
- **Backups** (the DB *and* the pepper are both required to restore):
  ```sh
  docker compose exec app sh -c \
    'sqlite3 /data/anonbookrec.db ".backup /data/backup.db"' 2>/dev/null \
    || docker run --rm -v anonbookrec_app-data:/data -v "$PWD:/out" debian:trixie-slim \
       cp /data/anonbookrec.db /data/.pepper /out/
  ```
  Cron that to off-site storage. (Optional later: Litestream sidecar for
  continuous S3 replication.)
- **Logs**: `docker compose logs -f app` (`RUST_LOG=info`).
- **Certificate renewal**: automatic (Caddy), state persisted in `caddy-data`.

## Security notes

- App container runs as a non-root system user; only Caddy touches the internet.
- Losing the `app-data` volume loses user pages irrecoverably (by design there
  is no account recovery) — treat backups as mandatory.
- `PEPPER` can be supplied via env/secret instead of the `.pepper` file if you
  prefer secrets management; both are 32-byte hex.

## Possible later steps (not in scope now)

- CI: GitHub Actions building/pushing the image, `docker compose pull` deploys.
- DO weekly droplet snapshots ($1.20/mo extra) as a coarse second backup layer.
- Litestream continuous DB replication to DO Spaces (S3-compatible).
- `fail2ban`-style abuse handling if the built-in rate limit proves too soft.
