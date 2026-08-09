# Security of the AnonBookRec Encryption & Login System

**A formal analysis.** Math renders via KaTeX/MathJax (GitHub renders `$…$` natively).

This document models the system as implemented in `src/crypto.rs`,
`src/auth.rs`, and `src/routes/users.rs`, states precise security
definitions, and proves — by standard game-hopping reductions — that the
at-rest encryption and login system are secure under well-studied
assumptions. Honest limitations are stated at the end.

---

## 1. Notation and primitives

Let $\lambda = 128$ denote the security level, and let
$\mathsf{negl}(\lambda)$ be any function smaller than every inverse
polynomial. $x \xleftarrow{\$} S$ denotes uniform sampling.

The system composes the following primitives:

| Symbol | Instantiation | Assumption |
|---|---|---|
| $H$ | SHA-256 | collision resistance; modeled as a random oracle (RO) where noted |
| $\mathsf{KDF}$ | HKDF-SHA256 | PRF security of HMAC-SHA256 (extract-then-expand, [Kra10]) |
| $\Pi = (\mathsf{E}, \mathsf{D})$ | ChaCha20-Poly1305 | IND-CPA $\wedge$ INT-CTXT AEAD |
| $\mathsf{Seal}, \mathsf{Unseal}$ | libsodium sealed box (X25519 + XSalsa20-Poly1305) | IND-CCA2 in the RO model under ODH/CDH on Curve25519 [BDPR98, ABR01] |
| $F$ | HMAC-SHA256 | PRF |
| $A$ | Argon2id | memory-hard function; modeled as RO for extraction bounds [AB16] |

### System state (what the database stores)

For a page with secret $s \xleftarrow{\$} \{0,1\}^{256}$ and X25519 keypair
$(\mathsf{pk}, \mathsf{sk})$:

$$
\mathsf{DB} \;=\;
\Bigl(
\underbrace{H(s)}_{\text{auth hash}},\;
\mathsf{pk},\;
\underbrace{\Pi.\mathsf{E}_{K_w}(\mathsf{sk})}_{\text{wrapped key}},\;
\{\underbrace{\mathsf{Seal}(\mathsf{pk}, m_i)}_{\text{private recs/notes}}\}_{i=1}^{n},\;
\{\underbrace{F_{K_b}(u \,\|\, \mathsf{isbn}_i)}_{\text{blind indexes}}\}
\Bigr)
$$

where $K_w = \mathsf{KDF}(s;\, \text{salt}=\sigma_w)$ with a fixed
domain-separation salt $\sigma_w$ (`anonbookrec.keywrap.v1`), and
$K_b = \mathsf{pepper}$ is a 256-bit key stored **outside** the database.

If a password $\pi$ is attached (username login), the database additionally
stores

$$
\mathsf{PHC} = A(\pi; \rho_1), \qquad
E_s = \Pi.\mathsf{E}_{K_\pi}(s), \quad K_\pi = A(\pi; \rho_2),
$$

with independent per-user salts $\rho_1, \rho_2 \xleftarrow{\$} \{0,1\}^{128}$.

---

## 2. Threat model

The adversary $\mathcal{A}$ obtains a **full copy of the database** (and may
additionally interact with the live API subject to rate limits). $\mathcal{A}$
does **not** hold: the owner secret $s$, the owner password $\pi$, the server
pepper $K_b$, or TLS session keys. Transport security (TLS 1.3 via Caddy) and
the server's runtime memory are assumed honest; see §7 for what breaks
otherwise.

---

## 3. Theorem 1 (Authentication soundness)

*No adversary given $\mathsf{DB}$ authenticates as the owner except with
negligible probability.*

**Claim.** For any $\mathcal{A}$ making $q_H$ random-oracle queries and $q_o$
online authentication attempts,

$$
\Pr[\mathcal{A} \text{ authenticates}] \;\le\; \frac{q_H + q_o}{2^{256}} .
$$

**Proof.** Authentication succeeds iff $\mathcal{A}$ presents $s'$ with
$H(s') = H(s)$, compared in constant time (crate `subtle`), so no timing
side-channel narrows the search. In the RO model, $H(s)$ for uniform
$s \in \{0,1\}^{256}$ is a uniformly random value revealing nothing about $s$
beyond oracle queries: each query $s'$ hits $H(s') = H(s)$ either because
$s' = s$ (probability $2^{-256}$ per fresh query, union-bounded over
$q_H + q_o$ attempts) or by finding a second preimage, which for a random
function has the same per-query probability. Summing gives the bound. Without
the RO idealization the statement follows from preimage resistance of SHA-256
on 256-bit uniform inputs. $\blacksquare$

*Remark.* The bound is astronomically small ($2^{-256}$ per guess); the
per-IP rate limit is defense in depth, not load-bearing here.

---

## 4. Theorem 2 (At-rest confidentiality, key-only accounts)

*The database reveals nothing about the contents of private recommendations.*

Define the standard real-or-random game
$\mathbf{G}^{\mathsf{ror}}_{\mathcal{A}}$: the challenger builds
$\mathsf{DB}$ either over the real private payloads $m_1,\dots,m_n$ (chosen by
$\mathcal{A}$) or over $0^{|m_1|},\dots,0^{|m_n|}$; $\mathcal{A}$ outputs a
guess. Advantage is
$\mathsf{Adv}^{\mathsf{ror}}(\mathcal{A}) = \bigl|\Pr[\text{guess right}] - \tfrac12\bigr|$.

**Claim.**

$$
\mathsf{Adv}^{\mathsf{ror}}(\mathcal{A})
\;\le\;
\mathsf{Adv}^{\mathsf{prf}}_{\mathsf{HKDF}}(\mathcal{B}_1)
+ \mathsf{Adv}^{\mathsf{ind\text{-}cpa}}_{\Pi}(\mathcal{B}_2)
+ n \cdot \mathsf{Adv}^{\mathsf{ind\text{-}cca}}_{\mathsf{Seal}}(\mathcal{B}_3)
+ \mathsf{Adv}^{\mathsf{prf}}_{F}(\mathcal{B}_4)
+ \frac{q_H}{2^{256}}
$$

for reduction algorithms $\mathcal{B}_i$ of comparable running time.

**Proof** (sequence of games; $\Delta_i$ denotes the distinguishing gap
between consecutive games).

**Game 0.** The real game.

**Game 1** (*decouple the wrap key*). Replace
$K_w = \mathsf{KDF}(s; \sigma_w)$ by $K_w \xleftarrow{\$} \{0,1\}^{256}$.
The only other value derived from $s$ in $\mathsf{DB}$ is $H(s)$; by domain
separation ($H$ applied to $s$ directly vs. HMAC-SHA256 keyed extraction with
salt $\sigma_w$) these are computed by distinct oracles in the ROM, and $s$
has full 256-bit entropy. A distinguisher yields a PRF adversary
$\mathcal{B}_1$ against HKDF (or, in the ROM, succeeds only by querying $s$
itself, absorbed in the $q_H/2^{256}$ term):
$\Delta_1 \le \mathsf{Adv}^{\mathsf{prf}}_{\mathsf{HKDF}} + q_H/2^{256}$.

**Game 2** (*erase the private key*). With $K_w$ now uniform and used only
once, replace $\Pi.\mathsf{E}_{K_w}(\mathsf{sk})$ by
$\Pi.\mathsf{E}_{K_w}(0^{|\mathsf{sk}|})$. Any distinguisher is an IND-CPA
adversary $\mathcal{B}_2$ against ChaCha20-Poly1305:
$\Delta_2 \le \mathsf{Adv}^{\mathsf{ind\text{-}cpa}}_{\Pi}$.
After this hop, $\mathsf{DB}$ contains **no information about
$\mathsf{sk}$** beyond $\mathsf{pk}$ itself.

**Game 3** (*empty the sealed boxes*). Replace each
$\mathsf{Seal}(\mathsf{pk}, m_i)$ by $\mathsf{Seal}(\mathsf{pk}, 0^{|m_i|})$,
one index at a time (hybrid argument over $n$ ciphertexts). Sealed boxes are
IND-CCA secure in the ROM under the Oracle Diffie–Hellman assumption on
Curve25519 — the construction is DHIES/ECIES with an ephemeral sender key and
a nonce derived as $H(\mathsf{epk} \| \mathsf{pk})$ [ABR01]. Since Game 2
removed every trace of $\mathsf{sk}$, the reduction $\mathcal{B}_3$ embeds
its challenge public key as $\mathsf{pk}$:
$\Delta_3 \le n \cdot \mathsf{Adv}^{\mathsf{ind\text{-}cca}}_{\mathsf{Seal}}$.

**Game 4** (*randomize the blind indexes*). Replace each
$F_{K_b}(u \| \mathsf{isbn}_i)$ by an independent uniform string (consistently
for repeats). $K_b$ is uniform, used nowhere else, and **absent from
$\mathsf{DB}$**, so a distinguisher is a PRF adversary $\mathcal{B}_4$
against HMAC-SHA256: $\Delta_4 \le \mathsf{Adv}^{\mathsf{prf}}_{F}$.

In Game 4 the database is distributed identically for real and zeroed
payloads — every payload-dependent value has been replaced by one independent
of the $m_i$. Collecting the gaps proves the claim. $\blacksquare$

**Corollary (per-recommendation privacy).** Titles, ISBNs, authors, and
submitter notes of private recommendations are computationally hidden.
Equality patterns of blind indexes reveal only *whether two submissions to
the same page concern the same ISBN* — exactly the information needed for
dedupe, and nothing else (see §7 for the pepper caveat).

---

## 5. Theorem 3 (Password path: security degrades exactly to password strength)

Attaching a password adds $\mathsf{PHC} = A(\pi;\rho_1)$ and
$E_s = \Pi.\mathsf{E}_{A(\pi;\rho_2)}(s)$ to $\mathsf{DB}$. We show the
adversary gains nothing beyond an unavoidable offline dictionary attack whose
every guess costs one Argon2id evaluation.

Model $A$ as a random oracle whose evaluation costs $c$ (memory-hard cost
$\approx$ 19 MiB, 2 passes at our parameters). Let the password be drawn
from a distribution with guessing curve $\gamma(q)$ (probability the best
$q$-guess dictionary contains $\pi$; for min-entropy $\mu$,
$\gamma(q) \le q\,2^{-\mu}$).

**Claim.** For any $\mathcal{A}$ spending total computation $T$,

$$
\mathsf{Adv}^{\mathsf{recover\ } s}(\mathcal{A})
\;\le\;
\gamma\!\left(\lfloor T/c \rfloor\right)
+ \mathsf{Adv}^{\mathsf{ind\text{-}cpa}}_{\Pi}(\mathcal{B})
+ \mathsf{negl}(\lambda).
$$

**Proof sketch.** In the ROM, the only way to obtain information about
$K_\pi = A(\pi;\rho_2)$ is to query $A$ on $(\pi', \rho_2)$ for candidate
passwords $\pi'$; each query costs $c$, so at most $\lfloor T/c\rfloor$
candidates are tested, succeeding with probability at most
$\gamma(\lfloor T/c \rfloor)$. The salts $\rho_1 \ne \rho_2$ are unique per
user and per purpose, so (i) $\mathsf{PHC}$ evaluations are useless for key
derivation and vice versa (domain separation), and (ii) rainbow-table /
multi-target amortization across users is impossible — each user forces a
fresh $c$-cost per guess. Conditioned on never querying $(\pi, \rho_2)$,
$K_\pi$ is uniform and unused elsewhere, so $E_s$ hides $s$ by IND-CPA of
$\Pi$; and $\mathsf{PHC}$ is an independent RO output revealing nothing about
$K_\pi$. $\blacksquare$

**Interpretation.** A database thief must pay one full Argon2id evaluation
per password guess per user, with no precomputation and no shortcut through
the PHC hash. Key-only accounts (no password attached, $E_s$ absent) retain
the full Theorem 2 guarantee with **no** dictionary-attack surface.

### Online login guessing

The login endpoint returns a uniform `401` for unknown usernames, wrong
passwords, and stale wraps — the responses are indistinguishable, so no
username enumeration oracle exists. With the dedicated per-IP token bucket
(burst 5, refill $1/10\text{s}$), an online attacker controlling $k$ IPs for
time $t$ seconds makes $q_{\mathsf{on}} \le k(5 + t/10)$ guesses, succeeding
with probability at most $\gamma(q_{\mathsf{on}})$ — e.g. one IP for a full
day yields $q_{\mathsf{on}} \le 8645$ guesses, each additionally costing the
server-side Argon2id delay.

---

## 6. Theorem 4 (Key rotation preserves confidentiality)

Rotation replaces $(H(s), \Pi.\mathsf{E}_{K_w}(\mathsf{sk}))$ with
$(H(s'), \Pi.\mathsf{E}_{K'_w}(\mathsf{sk}))$ for fresh
$s' \xleftarrow{\$} \{0,1\}^{256}$, guarded by an atomic compare-and-swap on
$H(s)$, and sets $E_s \leftarrow \bot$ (password login disabled until
re-attached).

**Claim.** (i) After rotation, knowledge of the *old* secret $s$ gives an
adversary holding the *current* database no advantage: every $s$-derived
value has been deleted, so Theorem 2 applies verbatim with $s'$ in place of
$s$. (ii) The CAS guard makes rotation atomic: of two concurrent rotations
presenting the same old hash, exactly one row-update matches
$\texttt{WHERE secret\_hash} = H(s)$; the loser matches zero rows and is
rejected, so the stored wrap and hash always correspond to the same secret.
(iii) Sealed payloads need no re-encryption: $(\mathsf{pk}, \mathsf{sk})$ is
unchanged, only its wrapping key rotates — decryptability is preserved
identically.

**Caveat.** An adversary holding a **pre-rotation database snapshot**
*and* the old secret $s$ decrypts that snapshot (rotation is not
retroactive); this is inherent to any at-rest scheme without re-encrypting
history. $\blacksquare$

---

## 7. Explicit limitations (what is *not* claimed)

1. **Transient server plaintext.** Submissions arrive over TLS in plaintext
   and are sealed server-side; owner reads decrypt server-side. A live
   memory-compromised server sees plaintexts in flight. The guarantee is
   *at rest* (database + backups), not end-to-end.
2. **Password transits to the server at login.** The scheme is not a PAKE;
   the server (honest but curious at runtime) momentarily sees $\pi$.
   An OPAQUE-style upgrade would remove this.
3. **Metadata leaks.** Row counts, timestamps, vote counts, and blind-index
   equality patterns are visible in the database by design (dedupe).
4. **Pepper compromise.** If $K_b$ leaks *with* the database, blind indexes
   admit an ISBN dictionary attack per page ($\sim 10^9$ HMACs per page —
   feasible). $K_b$ therefore lives outside the DB (env/file, 0600) and must
   be excluded from database backups' threat scope or stored separately.
5. **Client storage.** Secrets in `localStorage` fall to XSS; mitigated by
   `default-src 'self'` CSP, `textContent`-only rendering, and no inline
   scripts — but a successful CSP bypass defeats it.
6. **Unproven-but-standard primitives.** Security is conditional on SHA-256,
   HMAC, HKDF, ChaCha20-Poly1305, X25519/XSalsa20 sealed boxes, and Argon2id
   meeting their stated assumptions; all are current best practice with no
   known practical attacks.

## References

- [ABR01] Abdalla, Bellare, Rogaway. *DHIES: An encryption scheme based on
  the Diffie–Hellman problem.* CT-RSA 2001.
- [BDPR98] Bellare, Desai, Pointcheval, Rogaway. *Relations among notions of
  security for public-key encryption schemes.* CRYPTO 1998.
- [Kra10] Krawczyk. *Cryptographic extraction and key derivation: the HKDF
  scheme.* CRYPTO 2010.
- [AB16] Alwen, Blocki. *Efficiently computing data-independent memory-hard
  functions.* CRYPTO 2016; and RFC 9106 (Argon2).
- libsodium documentation, *Sealed boxes*.
