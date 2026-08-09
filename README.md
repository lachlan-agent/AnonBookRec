# AnonBookRec

AnonBookRec gives anonymous book recommendations. You create a page. You share
the link. Then each person can recommend a book to you. The system does not use
accounts, emails, or personal information.

- **Rust + Axum + SQLite**: The system is one binary and one database file.
- **ISBN cross-reference**: The system compares each submission with
  [Open Library](https://openlibrary.org). It finds the book by its title or its
  ISBN. It validates the ISBN-10/13 checksum locally. Thus the system stores
  only real books. A duplicate submission does not make a new row. It increases
  a counter. Each anonymous submitter increases the counter one time only.
- **Keys without email**: when you create a page, the system makes a random
  secret. The system shows the secret one time only. The server stores only the
  SHA-256 hash of the secret. The page URL is the public half. You can share it.
- **Encryption at rest**: the system seals each private recommendation with the
  X25519 public key of the page. The system stores the private key only in a
  wrapped condition, under the secret of the owner. Thus a copy of the database
  shows nothing. Only the owner, with the correct key, can read the private
  recommendations. Public recommendations are plaintext, because the system
  shows them to all persons.
- **Spam protection**: the system limits the request rate for each IP address.
  It validates each ISBN. It finds duplicates with blind indexes. It limits the
  request body to 4 KB. It applies a strict CSP. It uses parameterized queries
  in all locations.

## Operation

```sh
cargo run --release
# → http://127.0.0.1:8080
```

At the first start, the system makes the file `anonbookrec.db` (SQLite, WAL
mode) and the file `.pepper`. The `.pepper` file holds the blind-index key.
Make a backup of this file together with the database.

### Configuration (environment)

| Variable | Default | Purpose |
|---|---|---|
| `BIND` | `127.0.0.1:8080` | The listen address |
| `DATABASE_URL` | `sqlite://anonbookrec.db` | The SQLite database |
| `PUBLIC_BASE_URL` | `http://$BIND` | The base for share links, for example `https://books.example.com` |
| `SECURE_COOKIES` | `0` | Set the value to `1` if you operate behind HTTPS |
| `PEPPER` / `PEPPER_FILE` | `.pepper` | The blind-index secret, 64 hex characters |
| `STATIC_DIR` | `static` | The frontend assets |
| `RUST_LOG` | — | For example `info` |

## Test

```sh
cargo test
cargo clippy --all-targets
```

The tests are unit tests and end-to-end API tests. The tests do not use the
network. A stub replaces the resolver.

## Deployment (HTTPS)

Operate the binary behind a reverse proxy that terminates TLS. This is an
example with [Caddy](https://caddyserver.com). Caddy gets the certificates
automatically:

```caddyfile
books.example.com {
    reverse_proxy 127.0.0.1:8080
}
```

Set `PUBLIC_BASE_URL=https://books.example.com`. Set `SECURE_COOKIES=1`.
The rate limiter obeys the `X-Forwarded-For` header from the proxy.

This is an example of a systemd unit:

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
| POST | `/api/users` | — | Make a page. The result is `{slug, secret, url}`. The system shows the secret one time only |
| GET | `/api/pages/{slug}` | optional bearer | Get the public recommendations. The owner also gets the sealed recommendations, in decrypted form |
| PATCH | `/api/pages/{slug}` | bearer | Set `default_public` and/or the optional profile fields (`display_name`, `bio`, `favourite_book`, `theme`). The name is not a part of the URL |
| GET | `/api/search?q=` | — | Get typeahead suggestions (title, author, year, ISBN, cover id) |
| POST | `/api/pages/{slug}/recommendations` | — | Send `{book_name}` (a title or an ISBN). The system validates the book. Then it inserts a row or increases the counter |
| POST | `/api/pages/{slug}/rotate-key` | bearer | Make a new secret. The result is `{secret}`, shown one time only. The old secret stops working at once. The sealed recommendations stay readable, because the page keypair does not change |
| GET | `/api/pages/{slug}/qr.svg` | — | Get a QR code (SVG) of the share URL of the page |
| PATCH | `/api/recommendations/{id}` | bearer | Change `read` or `is_public`. This seals the recommendation or shows it |
| DELETE | `/api/recommendations/{id}` | bearer | Remove spam |
| DELETE | `/api/pages/{slug}` | bearer | Delete the page and all attached data permanently |

The owner authorization is `Authorization: Bearer <secret>`. The system compares
the secret with the stored hash in constant time.

## Anonymity and security model

- The system does not store emails, names, or passwords. Each submitter gets an
  HttpOnly random cookie. The system stores only the *hash* of the cookie. The
  system uses the hash only to prevent a double count.
- The server does not keep the owner secret. If you lose the secret, you cannot
  get the private recommendations again. This is intentional. At signup, the UI
  makes you confirm "I saved my key".
- The system stores each private recommendation as an `enc_payload` sealed box
  (X25519 + XSalsa20-Poly1305). The dedupe function uses HMAC blind indexes,
  with a server pepper as the key. Thus a plaintext ISBN of a private row does
  not go into the database.
- XSS: the system shows all user data with `textContent`. The CSP is
  `default-src 'self'`.
- SQL injection: sqlx bind parameters in all locations.
