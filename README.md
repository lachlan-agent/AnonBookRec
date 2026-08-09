# AnonBookRec

Anonymous book recommendations. Create a page, share the link anywhere — anyone can
recommend you a book. No accounts, no emails, no personal information.

- **Rust + Axum + SQLite** — a single binary and a single database file.
- **ISBN cross-referencing** — every submission is resolved against
  [Open Library](https://openlibrary.org) (by title or ISBN, with local ISBN-10/13
  checksum validation), so only real books are ever stored. Duplicates increment a
  counter instead of creating rows, one count per anonymous submitter.
- **Email-less keying** — creating a page mints a random secret (shown exactly once);
  the server stores only its SHA-256 hash. The page URL is the shareable public half.
- **Encrypted at rest** — private recommendations are sealed to the page's X25519
  public key. The private key is stored only wrapped under the owner's secret, so a
  database dump reveals nothing; only the owner, presenting their key, can read them.
  Public recommendations are plaintext (they're displayed anyway).
- **Spam resistance** — per-IP rate limiting, ISBN validation, dedupe via blind
  indexes, 4 KB body limit, strict CSP, parameterized queries throughout.

## Run

```sh
cargo run --release
# → http://127.0.0.1:8080
```

First run creates `anonbookrec.db` (SQLite, WAL) and a `.pepper` file (blind-index
key — back it up with the database).

### Configuration (environment)

| Variable | Default | Purpose |
|---|---|---|
| `BIND` | `127.0.0.1:8080` | Listen address |
| `DATABASE_URL` | `sqlite://anonbookrec.db` | SQLite database |
| `PUBLIC_BASE_URL` | `http://$BIND` | Base for share links, e.g. `https://books.example.com` |
| `SECURE_COOKIES` | `0` | Set `1` behind HTTPS |
| `PEPPER` / `PEPPER_FILE` | `.pepper` | 64-hex-char blind-index secret |
| `STATIC_DIR` | `static` | Frontend assets |
| `RUST_LOG` | — | e.g. `info` |

## Test

```sh
cargo test          # unit + end-to-end API tests (no network; stubbed resolver)
cargo clippy --all-targets
```

## Deploy (HTTPS)

Run the binary behind any TLS-terminating reverse proxy. With
[Caddy](https://caddyserver.com) (automatic certificates):

```caddyfile
books.example.com {
    reverse_proxy 127.0.0.1:8080
}
```

Set `PUBLIC_BASE_URL=https://books.example.com` and `SECURE_COOKIES=1`.
The rate limiter honours `X-Forwarded-For` from the proxy.

systemd unit example:

```ini
[Unit]
Description=AnonBookRec
After=network.target

[Service]
User=anonbookrec
WorkingDirectory=/var/lib/anonbookrec
Environment=BIND=127.0.0.1:8080
Environment=PUBLIC_BASE_URL=https://books.example.com
Environment=SECURE_COOKIES=1
Environment=STATIC_DIR=/var/lib/anonbookrec/static
Environment=RUST_LOG=info
ExecStart=/usr/local/bin/anonbookrec
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

## API

| Method | Path | Auth | Purpose |
|---|---|---|---|
| POST | `/api/users` | — | Create a page → `{slug, secret, url}` (secret shown once) |
| GET | `/api/pages/{slug}` | optional bearer | Public recommendations; owner also sees sealed ones, decrypted |
| PATCH | `/api/pages/{slug}` | bearer | Set `default_public` and/or optional `display_name` (never in the URL) |
| GET | `/api/search?q=` | — | Typeahead suggestions (title, author, year, ISBN, cover id) |
| POST | `/api/pages/{slug}/recommendations` | — | `{book_name}` (title or ISBN) → verify, insert or count |
| GET | `/api/global` | — | The open shelf (global public board) |
| POST | `/api/global/recommendations` | — | Recommend with no page at all |
| PATCH | `/api/recommendations/{id}` | bearer | Toggle `read` / `is_public` (seals or reveals) |
| DELETE | `/api/recommendations/{id}` | bearer | Remove spam |

Owner auth is `Authorization: Bearer <secret>`, verified in constant time against the
stored hash.

## Anonymity & security model

- No emails, names, or passwords anywhere. Submitters get an HttpOnly random cookie
  whose *hash* is stored, used only to stop double-counting.
- The owner secret never persists server-side. Losing it means private
  recommendations are unrecoverable **by design** — the UI forces an explicit
  "I saved my key" confirmation at signup.
- Private recommendations at rest: `enc_payload` sealed boxes
  (X25519 + XSalsa20-Poly1305); dedupe uses HMAC blind indexes keyed by a server
  pepper, so plaintext ISBNs never touch the database for private rows.
- XSS: all user data is rendered with `textContent`; CSP `default-src 'self'`.
- SQL injection: sqlx bind parameters everywhere.
