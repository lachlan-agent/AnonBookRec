"use strict";

/* ---------- Small helpers ---------- */

const $ = (id) => document.getElementById(id);

function el(tag, className, textContent) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (textContent !== undefined) node.textContent = textContent;
  return node;
}

const bearer = (secret) => ({ Authorization: `Bearer ${secret}` });

const byline = (book) => [book.author, book.year].filter(Boolean).join(", ");

const looksLikeIsbn = (text) => /^[0-9\-\s]{10,17}[0-9Xx]?$/.test(text);

const coverUrl = (book, size) =>
  book.cover_id
    ? `https://covers.openlibrary.org/b/id/${book.cover_id}-${size}.jpg`
    : `https://covers.openlibrary.org/b/isbn/${book.isbn13}-${size}.jpg`;

async function api(path, options = {}) {
  const headers = { ...(options.headers || {}) };
  if (options.body) headers["Content-Type"] = "application/json";
  const response = await fetch(path, { ...options, headers });
  let data = null;
  try { data = await response.json(); } catch { /* non-JSON error body */ }
  if (!response.ok) {
    const error = new Error((data && data.error) || `Request failed (${response.status})`);
    error.status = response.status;
    throw error;
  }
  return data;
}

function setStatus(message, isError = false) {
  const status = $("form-status");
  status.textContent = message;
  status.classList.toggle("error", isError);
}

const showError = (err) => setStatus(err.message, true);

/* Shared wiring for elements that exist on some pages but not others:
   every hookup is conditional, so any page can call this safely. */
function wireCopyButtons() {
  $("open-login")?.addEventListener("click", () => $("login-dialog").showModal());
  $("close-login")?.addEventListener("click", () => $("login-dialog").close());
  $("login-form")?.addEventListener("submit", async (event) => {
    event.preventDefault();
    $("login-status").textContent = "";
    try {
      const result = await api("/api/login", {
        method: "POST",
        body: JSON.stringify({
          username: $("login-username").value.trim(),
          password: $("login-password").value,
        }),
      });
      localStorage.setItem(`abr_secret_${result.handle}`, result.secret);
      localStorage.setItem("abr_last_slug", result.handle);
      window.location.href = `/u/${encodeURIComponent(result.handle)}`;
    } catch (err) {
      $("login-status").textContent =
        err.message === "unauthorized" ? "Wrong username or password." : err.message;
    }
  });

  document.querySelectorAll("[data-copy]").forEach((button) => {
    button.addEventListener("click", async () => {
      await navigator.clipboard.writeText($(button.dataset.copy).textContent);
      const original = button.textContent;
      button.textContent = "Copied";
      setTimeout(() => { button.textContent = original; }, 1200);
    });
  });
}

/* ---------- Canvas themes ----------

   Some themes render their backdrop on a fullscreen canvas instead of CSS.
   One rAF loop, DPR-aware, torn down on theme switch; reduced-motion users
   get a single static frame. */

const CANVAS_THEMES = {
  rain: {
    make: (w, h) => Array.from({ length: 130 }, () => ({
      x: Math.random() * w, y: Math.random() * h,
      len: 8 + Math.random() * 14, speed: 320 + Math.random() * 380,
      alpha: 0.06 + Math.random() * 0.12,
    })),
    draw(ctx, drops, w, h, dt) {
      ctx.lineWidth = 1;
      for (const d of drops) {
        d.y += d.speed * dt;
        d.x += d.speed * dt * 0.12; // a touch of wind
        if (d.y > h + d.len) { d.y = -d.len; d.x = Math.random() * w; }
        ctx.strokeStyle = `rgba(143, 198, 224, ${d.alpha})`;
        ctx.beginPath();
        ctx.moveTo(d.x, d.y);
        ctx.lineTo(d.x - d.len * 0.12, d.y - d.len);
        ctx.stroke();
      }
    },
  },
  fireflies: {
    make: (w, h) => Array.from({ length: 24 }, () => ({
      x: Math.random() * w, y: Math.random() * h,
      vx: (Math.random() - 0.5) * 14, vy: (Math.random() - 0.5) * 10,
      phase: Math.random() * Math.PI * 2, rate: 0.4 + Math.random() * 0.8,
    })),
    draw(ctx, flies, w, h, dt, t) {
      for (const f of flies) {
        f.vx += (Math.random() - 0.5) * 6 * dt;
        f.vy += (Math.random() - 0.5) * 6 * dt;
        f.vx = Math.max(-18, Math.min(18, f.vx));
        f.vy = Math.max(-14, Math.min(14, f.vy));
        f.x = (f.x + f.vx * dt + w) % w;
        f.y = (f.y + f.vy * dt + h) % h;
        const glow = 0.25 + 0.75 * (0.5 + Math.sin(t * f.rate + f.phase) / 2);
        const g = ctx.createRadialGradient(f.x, f.y, 0, f.x, f.y, 26);
        g.addColorStop(0, `rgba(214, 236, 140, ${0.5 * glow})`);
        g.addColorStop(1, "rgba(214, 236, 140, 0)");
        ctx.fillStyle = g;
        ctx.fillRect(f.x - 26, f.y - 26, 52, 52);
        ctx.fillStyle = `rgba(240, 250, 200, ${0.85 * glow})`;
        ctx.beginPath();
        ctx.arc(f.x, f.y, 1.4, 0, Math.PI * 2);
        ctx.fill();
      }
    },
  },
  snow: {
    make: (w, h) => Array.from({ length: 110 }, () => ({
      x: Math.random() * w, y: Math.random() * h,
      r: 0.8 + Math.random() * 2.2, speed: 18 + Math.random() * 40,
      phase: Math.random() * Math.PI * 2, sway: 10 + Math.random() * 24,
    })),
    draw(ctx, flakes, w, h, dt, t) {
      for (const f of flakes) {
        f.y += f.speed * dt;
        const x = f.x + Math.sin(t * 0.6 + f.phase) * f.sway;
        if (f.y > h + 4) { f.y = -4; f.x = Math.random() * w; }
        ctx.fillStyle = `rgba(232, 237, 245, ${0.16 + f.r * 0.14})`;
        ctx.beginPath();
        ctx.arc(x, f.y, f.r, 0, Math.PI * 2);
        ctx.fill();
      }
    },
  },
  constellation: {
    make: (w, h) => Array.from({ length: 42 }, () => ({
      x: Math.random() * w, y: Math.random() * h,
      vx: (Math.random() - 0.5) * 9, vy: (Math.random() - 0.5) * 9,
    })),
    draw(ctx, stars, w, h, dt) {
      for (const s of stars) {
        s.x = (s.x + s.vx * dt + w) % w;
        s.y = (s.y + s.vy * dt + h) % h;
      }
      const reach = 130;
      for (let i = 0; i < stars.length; i++) {
        for (let j = i + 1; j < stars.length; j++) {
          const dx = stars[i].x - stars[j].x;
          const dy = stars[i].y - stars[j].y;
          const dist = Math.hypot(dx, dy);
          if (dist < reach) {
            ctx.strokeStyle = `rgba(169, 158, 240, ${0.14 * (1 - dist / reach)})`;
            ctx.lineWidth = 1;
            ctx.beginPath();
            ctx.moveTo(stars[i].x, stars[i].y);
            ctx.lineTo(stars[j].x, stars[j].y);
            ctx.stroke();
          }
        }
      }
      ctx.fillStyle = "rgba(230, 233, 245, 0.7)";
      for (const s of stars) {
        ctx.beginPath();
        ctx.arc(s.x, s.y, 1.3, 0, Math.PI * 2);
        ctx.fill();
      }
    },
  },
};

/* Matrix: columns of glyph rain with fading trails */
CANVAS_THEMES.matrix = {
  make: (w) => Array.from({ length: Math.ceil(w / 16) }, (_, i) => ({
    x: i * 16 + 8, y: Math.random() * -800,
    speed: 60 + Math.random() * 140, trail: 10 + Math.floor(Math.random() * 10),
  })),
  draw(ctx, cols, w, h, dt) {
    const glyphs = "アイウエオカキクケコサシスセソ0123456789ネノABCDEF";
    ctx.font = "14px monospace";
    ctx.textAlign = "center";
    for (const c of cols) {
      c.y += c.speed * dt;
      if (c.y - c.trail * 16 > h) { c.y = Math.random() * -300; c.speed = 60 + Math.random() * 140; }
      for (let i = 0; i < c.trail; i++) {
        const y = c.y - i * 16;
        if (y < -16 || y > h + 16) continue;
        const alpha = i === 0 ? 0.8 : 0.35 * (1 - i / c.trail);
        ctx.fillStyle = i === 0 ? `rgba(200, 255, 200, ${alpha})` : `rgba(60, 210, 110, ${alpha})`;
        ctx.fillText(glyphs[(Math.floor(c.y / 16) * 7 + i * 13) % glyphs.length], c.x, y);
      }
    }
  },
};

/* Koi pond: fish steering around the page, occasional ripples */
CANVAS_THEMES.koi = {
  make: (w, h) => ({
    fish: Array.from({ length: 5 }, (_, i) => ({
      x: Math.random() * w, y: Math.random() * h,
      angle: Math.random() * Math.PI * 2, speed: 34 + Math.random() * 26,
      phase: Math.random() * Math.PI * 2, size: 11 + Math.random() * 7,
      calico: i % 2 === 0,
    })),
    ripples: [],
    nextRipple: 1,
  }),
  draw(ctx, pond, w, h, dt, t) {
    pond.nextRipple -= dt;
    if (pond.nextRipple <= 0) {
      pond.ripples.push({ x: Math.random() * w, y: Math.random() * h, r: 0, alpha: 0.3 });
      pond.nextRipple = 2 + Math.random() * 3;
    }
    for (const r of pond.ripples) {
      r.r += 26 * dt;
      r.alpha -= 0.07 * dt;
      ctx.strokeStyle = `rgba(180, 220, 225, ${Math.max(r.alpha, 0)})`;
      ctx.lineWidth = 1.2;
      ctx.beginPath();
      ctx.arc(r.x, r.y, r.r, 0, Math.PI * 2);
      ctx.stroke();
    }
    pond.ripples = pond.ripples.filter((r) => r.alpha > 0.01);

    for (const k of pond.fish) {
      // Wander, plus a steer back toward the middle near the edges.
      k.angle += Math.sin(t * 0.7 + k.phase) * 0.5 * dt;
      const margin = 70;
      if (k.x < margin || k.x > w - margin || k.y < margin || k.y > h - margin) {
        const toCenter = Math.atan2(h / 2 - k.y, w / 2 - k.x);
        let delta = toCenter - k.angle;
        while (delta > Math.PI) delta -= Math.PI * 2;
        while (delta < -Math.PI) delta += Math.PI * 2;
        k.angle += delta * 0.9 * dt;
      }
      k.x += Math.cos(k.angle) * k.speed * dt;
      k.y += Math.sin(k.angle) * k.speed * dt;

      const wag = Math.sin(t * 5 + k.phase) * 0.4;
      ctx.save();
      ctx.translate(k.x, k.y);
      ctx.rotate(k.angle);
      // tail
      ctx.fillStyle = k.calico ? "rgba(224, 130, 70, 0.5)" : "rgba(235, 235, 230, 0.42)";
      ctx.save();
      ctx.translate(-k.size * 1.2, 0);
      ctx.rotate(wag);
      ctx.beginPath();
      ctx.moveTo(0, 0);
      ctx.lineTo(-k.size * 0.9, -k.size * 0.5);
      ctx.lineTo(-k.size * 0.9, k.size * 0.5);
      ctx.closePath();
      ctx.fill();
      ctx.restore();
      // body
      ctx.beginPath();
      ctx.ellipse(0, 0, k.size * 1.35, k.size * 0.55, 0, 0, Math.PI * 2);
      ctx.fillStyle = k.calico ? "rgba(238, 238, 232, 0.55)" : "rgba(224, 130, 70, 0.55)";
      ctx.fill();
      if (k.calico) {
        ctx.clip();
        ctx.fillStyle = "rgba(224, 130, 70, 0.6)";
        ctx.beginPath();
        ctx.arc(k.size * 0.5, -k.size * 0.15, k.size * 0.45, 0, Math.PI * 2);
        ctx.arc(-k.size * 0.4, k.size * 0.2, k.size * 0.38, 0, Math.PI * 2);
        ctx.fill();
      }
      ctx.restore();
    }
  },
};

/* Lava lamp: additive metaball blobs drifting up and down */
CANVAS_THEMES.lava = {
  make: (w, h) => Array.from({ length: 7 }, (_, i) => ({
    x: (i + 0.5) * (w / 7), y: Math.random() * h,
    r: 44 + Math.random() * 50, vy: 12 + Math.random() * 16,
    dir: i % 2 ? 1 : -1, phase: Math.random() * Math.PI * 2,
  })),
  draw(ctx, blobs, w, h, dt, t) {
    ctx.globalCompositeOperation = "lighter";
    for (const b of blobs) {
      b.y += b.vy * b.dir * dt;
      if (b.y < -b.r) { b.dir = 1; b.x = Math.random() * w; }
      if (b.y > h + b.r) { b.dir = -1; b.x = Math.random() * w; }
      const x = b.x + Math.sin(t * 0.4 + b.phase) * 30;
      const squish = 1 + Math.sin(t * 0.9 + b.phase) * 0.12;
      const g = ctx.createRadialGradient(x, b.y, 0, x, b.y, b.r * squish);
      g.addColorStop(0, "rgba(200, 70, 150, 0.16)");
      g.addColorStop(0.6, "rgba(160, 60, 140, 0.10)");
      g.addColorStop(1, "rgba(120, 40, 120, 0)");
      ctx.fillStyle = g;
      ctx.beginPath();
      ctx.arc(x, b.y, b.r * squish, 0, Math.PI * 2);
      ctx.fill();
    }
    ctx.globalCompositeOperation = "source-over";
  },
};

/* Paper birds: a boids flock in ink on paper */
CANVAS_THEMES.flock = {
  make: (w, h) => Array.from({ length: 32 }, () => ({
    x: Math.random() * w, y: Math.random() * h,
    vx: (Math.random() - 0.5) * 60, vy: (Math.random() - 0.5) * 60,
  })),
  draw(ctx, birds, w, h, dt) {
    const reach = 62, maxSpeed = 70;
    for (const b of birds) {
      let cx = 0, cy = 0, ax = 0, ay = 0, sx = 0, sy = 0, n = 0;
      for (const o of birds) {
        if (o === b) continue;
        const dx = o.x - b.x, dy = o.y - b.y;
        const dist = Math.hypot(dx, dy);
        if (dist < reach) {
          cx += o.x; cy += o.y; ax += o.vx; ay += o.vy; n++;
          if (dist < 22 && dist > 0) { sx -= dx / dist; sy -= dy / dist; }
        }
      }
      if (n) {
        b.vx += ((cx / n - b.x) * 0.4 + (ax / n - b.vx) * 0.9) * dt;
        b.vy += ((cy / n - b.y) * 0.4 + (ay / n - b.vy) * 0.9) * dt;
      }
      b.vx += sx * 42 * dt;
      b.vy += sy * 42 * dt;
      // stay on the page
      if (b.x < 60) b.vx += 46 * dt;
      if (b.x > w - 60) b.vx -= 46 * dt;
      if (b.y < 60) b.vy += 46 * dt;
      if (b.y > h - 60) b.vy -= 46 * dt;
      const speed = Math.hypot(b.vx, b.vy) || 1;
      if (speed > maxSpeed) { b.vx *= maxSpeed / speed; b.vy *= maxSpeed / speed; }
      b.x += b.vx * dt;
      b.y += b.vy * dt;

      const angle = Math.atan2(b.vy, b.vx);
      ctx.save();
      ctx.translate(b.x, b.y);
      ctx.rotate(angle);
      ctx.fillStyle = "rgba(42, 42, 40, 0.34)";
      ctx.beginPath();
      ctx.moveTo(6, 0);
      ctx.lineTo(-5, -4);
      ctx.lineTo(-2.5, 0);
      ctx.lineTo(-5, 4);
      ctx.closePath();
      ctx.fill();
      ctx.restore();
    }
  },
};

/* Glass variant flies the same flock behind frosted panels. */
CANVAS_THEMES["flock-glass"] = CANVAS_THEMES.flock;

let canvasThemeCleanup = null;

function applyCanvasTheme(name) {
  if (canvasThemeCleanup) { canvasThemeCleanup(); canvasThemeCleanup = null; }
  const spec = CANVAS_THEMES[name];
  if (!spec) return;

  const canvas = document.createElement("canvas");
  canvas.id = "theme-canvas";
  canvas.setAttribute("aria-hidden", "true");
  document.body.prepend(canvas);
  const ctx = canvas.getContext("2d");
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  let particles, raf = 0, last = performance.now(), elapsed = 0;

  const resize = () => {
    canvas.width = window.innerWidth * dpr;
    canvas.height = window.innerHeight * dpr;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    particles = spec.make(window.innerWidth, window.innerHeight);
  };
  resize();
  window.addEventListener("resize", resize);

  const frame = (now) => {
    const dt = Math.min((now - last) / 1000, 0.05);
    last = now;
    elapsed += dt;
    ctx.clearRect(0, 0, window.innerWidth, window.innerHeight);
    spec.draw(ctx, particles, window.innerWidth, window.innerHeight, dt, elapsed);
    raf = requestAnimationFrame(frame);
  };

  if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
    spec.draw(ctx, particles, window.innerWidth, window.innerHeight, 0.016, 1);
  } else {
    raf = requestAnimationFrame(frame);
  }

  canvasThemeCleanup = () => {
    cancelAnimationFrame(raf);
    window.removeEventListener("resize", resize);
    canvas.remove();
  };
}

/* ---------- Recommendation list ---------- */

const STATUS_TEXT = {
  created: (book) => `“${book.title}” added to the card.`,
  counted: (book) => `“${book.title}” was already here. Vote added.`,
  already_recommended: (book) => `You already recommended “${book.title}”.`,
};

function renderList(recommendations, { owner, secret, slug } = {}) {
  const list = $("rec-list");
  list.replaceChildren();
  $("empty-shelf").hidden = recommendations.length > 0;

  for (const rec of recommendations) {
    const item = el("li");
    item.append(el("span", "rec-title" + (rec.read ? " read" : ""), rec.book_name));
    if (rec.author) item.append(el("span", "rec-author", rec.author));
    item.append(el("span", "rec-isbn", `ISBN ${rec.isbn}`));
    if (owner && !rec.is_public) {
      item.append(el("span", "rec-private", "sealed"));
    }
    item.append(el("span", "rec-count", `× ${rec.recommendation_count}`));

    for (const note of rec.notes || []) {
      item.append(el("blockquote", "rec-note", note));
    }

    if (owner) item.append(ownerControls(rec, secret, slug));
    list.append(item);
  }
}

function ownerControls(rec, secret, slug) {
  const patch = async (body) => {
    await api(`/api/recommendations/${rec.id}`, {
      method: "PATCH",
      headers: bearer(secret),
      body: JSON.stringify(body),
    });
    await loadUserPage(slug);
  };

  const button = (label, className, onClick) => {
    const node = el("button", className, label);
    node.type = "button";
    node.addEventListener("click", () => onClick().catch(showError));
    return node;
  };

  const controls = el("div", "rec-controls");
  controls.append(
    button(rec.read ? "Mark unread" : "Mark read", "button small",
      () => patch({ read: !rec.read })),
    button(rec.is_public ? "Hide" : "Reveal", "button small",
      () => patch({ is_public: !rec.is_public })),
    button("Remove", "button small danger", async () => {
      await api(`/api/recommendations/${rec.id}`, {
        method: "DELETE",
        headers: bearer(secret),
      });
      await loadUserPage(slug);
    }),
  );
  return controls;
}

/* ---------- Submit form (typeahead + confirmation) ----------

   Nothing is submitted until the person confirms a specific edition
   (title, author, cover), so the wrong book can't slip in. */

function wireSubmitForm(submitUrl, reload) {
  const input = $("book-input");
  const dropdown = $("suggestions");
  const card = $("confirm-card");
  let debounceTimer = null;
  let suggestions = [];
  let activeIndex = -1;
  let selected = null;

  function hideDropdown() {
    dropdown.hidden = true;
    dropdown.replaceChildren();
    input.setAttribute("aria-expanded", "false");
    activeIndex = -1;
  }

  function hideConfirm() {
    card.hidden = true;
    selected = null;
    $("book-note").value = "";
  }

  function showConfirm(book) {
    selected = book;
    hideDropdown();
    $("confirm-title").textContent = book.title;
    $("confirm-author").textContent = byline(book);
    $("confirm-isbn").textContent = `ISBN ${book.isbn13}`;
    const cover = $("confirm-cover");
    cover.hidden = true;
    cover.onload = () => { cover.hidden = cover.naturalWidth <= 1; };
    cover.src = coverUrl(book, "M");
    card.hidden = false;
    setStatus("");
  }

  function renderDropdown() {
    dropdown.replaceChildren();
    for (const [index, book] of suggestions.entries()) {
      const item = el("li", "suggestion" + (index === activeIndex ? " active" : ""));
      item.role = "option";

      const cover = el("img", "suggestion-cover");
      cover.alt = "";
      cover.loading = "lazy";
      cover.src = coverUrl(book, "S");
      cover.onerror = () => cover.removeAttribute("src");

      const text = el("div", "suggestion-text");
      text.append(
        el("span", "suggestion-title", book.title),
        el("span", "suggestion-author", byline(book)),
      );
      item.append(cover, text);

      // mousedown, not click: fires before the input loses focus.
      item.addEventListener("mousedown", (event) => {
        event.preventDefault();
        showConfirm(book);
      });
      dropdown.append(item);
    }
    dropdown.hidden = suggestions.length === 0;
    input.setAttribute("aria-expanded", String(suggestions.length > 0));
  }

  async function fetchSuggestions(query) {
    try {
      const results = await api(`/api/search?q=${encodeURIComponent(query)}`);
      if (input.value.trim() !== query) return; // stale response
      suggestions = results;
      activeIndex = -1;
      renderDropdown();
    } catch { /* typeahead is best-effort; submit still verifies */ }
  }

  async function submitBook(bookName) {
    setStatus("Adding to the card…");
    const note = $("book-note").value.trim();
    // Send the owner key when we hold one: on a hidden page the canonical
    // URL rejects anonymous submissions, and the key is what lets the owner
    // recommend on their own card.
    const ownSecret = localStorage.getItem(`abr_secret_${pageSlug()}`);
    try {
      const result = await api(submitUrl, {
        method: "POST",
        headers: ownSecret ? bearer(ownSecret) : {},
        body: JSON.stringify({ book_name: bookName, note: note || null }),
      });
      const describe = STATUS_TEXT[result.status] || ((book) => `“${book.title}” recorded.`);
      setStatus(describe(result.book));
      input.value = "";
      hideConfirm();
      hideDropdown();
      await reload();
    } catch (err) { showError(err); }
  }

  input.addEventListener("input", () => {
    clearTimeout(debounceTimer);
    hideConfirm();
    const query = input.value.trim();
    if (query.length < 3 || looksLikeIsbn(query)) {
      hideDropdown();
      return;
    }
    debounceTimer = setTimeout(() => fetchSuggestions(query), 300);
  });

  input.addEventListener("keydown", (event) => {
    if (dropdown.hidden) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const step = event.key === "ArrowDown" ? 1 : -1;
      activeIndex = (activeIndex + step + suggestions.length) % suggestions.length;
      renderDropdown();
    } else if (event.key === "Enter" && activeIndex >= 0) {
      event.preventDefault();
      showConfirm(suggestions[activeIndex]);
    } else if (event.key === "Escape") {
      hideDropdown();
    }
  });
  input.addEventListener("blur", () => setTimeout(hideDropdown, 150));

  $("confirm-submit").addEventListener("click", () => {
    if (selected) submitBook(selected.isbn13);
  });
  $("confirm-cancel").addEventListener("click", () => {
    hideConfirm();
    input.focus();
  });

  $("recommend-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    if (selected) return submitBook(selected.isbn13);
    const query = input.value.trim();
    if (!query) return;

    // Search and ask for confirmation of the top match (with a note field).
    setStatus("Searching the catalogue…");
    try {
      const results = await api(`/api/search?q=${encodeURIComponent(query)}`);
      if (results.length === 0) {
        // Exact ISBNs sometimes miss the search index; submit directly.
        if (looksLikeIsbn(query)) return submitBook(query);
        setStatus("No matching book found — try the exact title or an ISBN.", true);
        return;
      }
      suggestions = results;
      showConfirm(results[0]);
      if (results.length > 1) renderDropdown(); // alternatives stay visible
    } catch (err) { showError(err); }
  });
}

/* ---------- Landing page ---------- */

function initIndex() {
  const lastSlug = localStorage.getItem("abr_last_slug");
  if (lastSlug && localStorage.getItem(`abr_secret_${lastSlug}`)) {
    $("returning").hidden = false;
    $("returning-link").href = `/u/${lastSlug}`;
  }

  let createdPage = null;

  $("create-page").addEventListener("click", async () => {
    try {
      createdPage = await api("/api/users", { method: "POST" });
      localStorage.setItem(`abr_secret_${createdPage.slug}`, createdPage.secret);
      localStorage.setItem("abr_last_slug", createdPage.slug);
      $("page-url").textContent = createdPage.url;
      $("secret-key").textContent = createdPage.secret;
      $("key-dialog").showModal();
    } catch (err) { showError(err); }
  });

  $("download-key").addEventListener("click", () => {
    if (!createdPage) return;
    const blob = new Blob(
      [`Anonymous Book Recommendation page: ${createdPage.url}\nSecret key: ${createdPage.secret}\n\nKeep this file private. The key cannot be recovered.\n`],
      { type: "text/plain" });
    const link = document.createElement("a");
    link.href = URL.createObjectURL(blob);
    link.download = `anonbookrec-key-${createdPage.slug}.txt`;
    link.click();
    URL.revokeObjectURL(link.href);
  });
  $("key-saved").addEventListener("change", (event) => {
    $("go-to-page").disabled = !event.target.checked;
  });
  $("go-to-page").addEventListener("click", () => {
    if (createdPage) window.location.href = `/u/${createdPage.slug}`;
  });

  wireCopyButtons();
}

/* ---------- User page ---------- */

function pageSlug() {
  return decodeURIComponent(window.location.pathname.split("/").pop());
}

/* Latest page state, for handlers that need more than the slug. */
let currentPage = null;

/* Apply a style bundle (theme, font, corners, density, accent) to the page.
   Used both for saved state and for live preview while the Theme tab is
   being fiddled with. */
function applyStyle(style) {
  document.body.dataset.theme = style.theme || "cloth";
  applyCanvasTheme(style.theme);
  document.body.dataset.font = style.font || "";
  document.body.dataset.radius = style.radius || "";
  document.body.dataset.density = style.density || "";
  applyAccent(style.accent);
}

/* Custom accent: sets --cloth and derives --cloth-dark (hover fill + accent
   text) by darkening on light themes and lightening on dark ones. */
function applyAccent(hex) {
  const root = document.body.style;
  if (!hex || !/^#[0-9a-f]{6}$/i.test(hex)) {
    root.removeProperty("--cloth");
    root.removeProperty("--cloth-dark");
    return;
  }
  const rgb = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
  const bg = getComputedStyle(document.body).backgroundColor.match(/\d+/g) || [245, 245, 240];
  const bgLum = (0.299 * bg[0] + 0.587 * bg[1] + 0.114 * bg[2]) / 255;
  const shade = (c, f) => Math.round(Math.min(255, Math.max(0, c * f + (f > 1 ? 20 : 0))));
  const factor = bgLum > 0.5 ? 0.72 : 1.45;
  const dark = rgb.map((c) => shade(c, factor));
  root.setProperty("--cloth", hex);
  root.setProperty("--cloth-dark", `rgb(${dark.join(",")})`);
}

async function refreshQr(slug, secret, page) {
  try {
    const response = await fetch(`/api/pages/${encodeURIComponent(slug)}/qr.svg`, {
      headers: secret ? bearer(secret) : {},
      cache: "no-store",
    });
    if (!response.ok) throw new Error();
    const svg = await response.text();
    $("qr-image").src = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
    $("qr-note").textContent = page.is_hidden
      ? (page.share_token
          ? "QR points to your friend link."
          : "QR points to your page's normal address — while hidden, only you can open it. Create a friend link to make the QR shareable.")
      : "QR points to your public page link.";
  } catch {
    $("qr-note").textContent = "Couldn't load the QR code.";
  }
}

async function loadUserPage(slug) {
  const secret = localStorage.getItem(`abr_secret_${slug}`);
  const page = await api(`/api/pages/${encodeURIComponent(slug)}`, {
    headers: secret ? bearer(secret) : {},
  });

  currentPage = page;
  $("unlock-panel").hidden = page.owner || Boolean(secret);
  applyStyle(page);
  $("page-bio").hidden = !page.bio;
  $("page-bio").textContent = page.bio || "";
  $("page-fav").hidden = !page.favourite_book;
  $("page-fav").replaceChildren();
  if (page.favourite_book) {
    $("page-fav").append(
      el("span", "fav-label", "Favourite book"),
      el("span", "fav-title", page.favourite_book),
    );
  }

  if (page.owner) {
    $("page-title").textContent = page.display_name
      ? `${page.display_name}'s card` : "Your card";
    $("open-settings").hidden = false;
    $("default-public").checked = page.default_public;
    $("profile-fields").disabled = !page.default_public;
    const fields = {
      "display-name": page.display_name,
      "theme": page.theme || "cloth",
      "font": page.font || "",
      "radius": page.radius || "",
      "density": page.density || "",
      "bio": page.bio,
      "favourite-book": page.favourite_book,
    };
    $("theme-fields").disabled = !page.default_public;
    $("accent-on").checked = Boolean(page.accent);
    if (page.accent) $("accent-color").value = page.accent;
    for (const [id, value] of Object.entries(fields)) {
      if (document.activeElement !== $(id)) $(id).value = value || "";
    }
    $("hide-page").checked = page.is_hidden;
    $("share-zone").hidden = !page.is_hidden;
    const hasLink = Boolean(page.share_token);
    $("create-share-link").textContent = hasLink ? "New friend link" : "Create friend link";
    $("copy-share-link").hidden = !hasLink;
    $("revoke-share-link").hidden = !hasLink;
    $("share-link-status").textContent = hasLink
      ? `Friend link: ${window.location.origin}/u/${page.share_token}`
      : "No friend link — only you can open this page right now.";
    // Keep localStorage keyed under whatever handle the server prefers, so
    // sign-ins and redirects always find the secret.
    if (page.handle && page.handle !== slug) {
      localStorage.setItem(`abr_secret_${page.handle}`, secret);
      localStorage.setItem("abr_last_slug", page.handle);
    }
    // The QR always shows in settings and tracks whatever the server would
    // share right now: friend link while hidden (with one minted), the
    // normal page link otherwise. Bearer fetch: img tags can't authenticate
    // against a hidden page's canonical URL.
    refreshQr(slug, secret, page);
    $("account-current").textContent = page.username
      ? `Current username: ${page.username}.` : "No username set yet.";
    if (document.activeElement !== $("account-username")) {
      $("account-username").value = page.username || "";
    }
    $("remove-credentials").hidden = !page.username;
    $("shelf-note").textContent = "your recommendations";
  } else {
    $("page-title").textContent = page.display_name
      ? `${page.display_name}'s card`
      : "Recommend this reader a book";
    $("shelf-note").textContent = "what others have recommended";
  }
  renderList(page.recommendations, { owner: page.owner, secret, slug });
  return page;
}

function initUser() {
  const slug = pageSlug();
  const secret = localStorage.getItem(`abr_secret_${slug}`);

  loadUserPage(slug).catch((err) => {
    if (err.status === 404) {
      // The page is gone (or hidden and our key is stale): any stored key is
      // useless now. Drop it and let the server's real 404 page answer —
      // removing the key also breaks the 404→unlock redirect loop.
      localStorage.removeItem(`abr_secret_${slug}`);
      if (localStorage.getItem("abr_last_slug") === slug) {
        localStorage.removeItem("abr_last_slug");
      }
      window.location.replace(`/u/${encodeURIComponent(slug)}`);
      return;
    }
    $("page-title").textContent = "Page not found";
    $("recommend-form").hidden = true;
    showError(err);
  });

  wireSubmitForm(`/api/pages/${encodeURIComponent(slug)}/recommendations`,
    () => loadUserPage(slug));

  wireCopyButtons();

  $("open-settings").addEventListener("click", () => {
    $("settings-dialog").showModal();
  });

  const showTab = (name) => {
    $("tab-general").hidden = name !== "general";
    $("tab-theme").hidden = name !== "theme";
    $("tab-btn-general").classList.toggle("active", name === "general");
    $("tab-btn-theme").classList.toggle("active", name === "theme");
  };
  $("tab-btn-general").addEventListener("click", () => showTab("general"));
  $("tab-btn-theme").addEventListener("click", () => showTab("theme"));

  // Live preview: every Theme-tab change restyles the page immediately;
  // nothing persists until Save. Closing the dialog reverts to saved state.
  const previewStyle = () => applyStyle({
    theme: $("theme").value,
    font: $("font").value,
    radius: $("radius").value,
    density: $("density").value,
    accent: $("accent-on").checked ? $("accent-color").value : null,
  });
  for (const id of ["theme", "font", "radius", "density", "accent-on"]) {
    $(id).addEventListener("change", previewStyle);
  }
  $("accent-color").addEventListener("input", () => {
    $("accent-on").checked = true;
    previewStyle();
  });
  $("settings-dialog").addEventListener("close", () => {
    if (currentPage) applyStyle(currentPage);
  });
  $("close-settings").addEventListener("click", () => {
    $("settings-dialog").close();
  });

  $("copy-share").addEventListener("click", async (event) => {
    const button = event.currentTarget;
    // A hidden page's canonical URL is dead to visitors — share the friend
    // link instead, or say why there is nothing to share yet.
    if (currentPage?.is_hidden) {
      if (!currentPage.share_token) {
        button.textContent = "Create a friend link first";
        setTimeout(() => { button.textContent = "Copy share link"; }, 1800);
        return;
      }
      await navigator.clipboard.writeText(
        `${window.location.origin}/u/${currentPage.share_token}`);
    } else {
      await navigator.clipboard.writeText(
        `${window.location.origin}/u/${currentPage?.handle || slug}`);
    }
    button.textContent = "Link copied";
    setTimeout(() => { button.textContent = "Copy share link"; }, 1200);
  });

  $("create-share-link").addEventListener("click", async () => {
    try {
      // The hide checkbox may be ticked but not yet saved; persist it first
      // so the reload doesn't revert the checkbox and the QR switches to the
      // friend link.
      if ($("hide-page").checked !== Boolean(currentPage?.is_hidden)) {
        await api(`/api/pages/${encodeURIComponent(slug)}`, {
          method: "PATCH",
          headers: bearer(secret),
          body: JSON.stringify({ is_hidden: $("hide-page").checked }),
        });
      }
      await api(`/api/pages/${encodeURIComponent(slug)}/share-link`, {
        method: "POST",
        headers: bearer(secret),
      });
      await loadUserPage(slug);
    } catch (err) { showError(err); }
  });

  $("copy-share-link").addEventListener("click", async (event) => {
    if (!currentPage?.share_token) return;
    await navigator.clipboard.writeText(
      `${window.location.origin}/u/${currentPage.share_token}`);
    const button = event.currentTarget;
    button.textContent = "Copied";
    setTimeout(() => { button.textContent = "Copy friend link"; }, 1200);
  });

  $("revoke-share-link").addEventListener("click", async () => {
    try {
      const result = await api(`/api/pages/${encodeURIComponent(slug)}/share-link`, {
        method: "DELETE",
        headers: bearer(secret),
      });
      // If the browser sits on the now-dead token URL, move to the owner door.
      if (slug === currentPage?.share_token) {
        localStorage.setItem(`abr_secret_${result.handle}`, secret);
        localStorage.setItem("abr_last_slug", result.handle);
        window.location.replace(`/u/${encodeURIComponent(result.handle)}?unlock`);
        return;
      }
      await loadUserPage(slug);
    } catch (err) { showError(err); }
  });

  // QR: fetch the server-rendered SVG, rasterize to PNG on a canvas.
  // data: URI keeps the CSP happy (blob: isn't in img-src).
  async function qrPngBlob() {
    const ownSecret = localStorage.getItem(`abr_secret_${pageSlug()}`);
    const response = await fetch(`/api/pages/${encodeURIComponent(slug)}/qr.svg`, {
      headers: ownSecret ? bearer(ownSecret) : {},
      cache: "no-store",
    });
    if (!response.ok) throw new Error("could not fetch the QR code");
    const svg = await response.text();
    const img = new Image();
    await new Promise((resolve, reject) => {
      img.onload = resolve;
      img.onerror = () => reject(new Error("could not render the QR code"));
      img.src = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
    });
    const canvas = document.createElement("canvas");
    canvas.width = canvas.height = 512;
    const ctx = canvas.getContext("2d");
    ctx.fillStyle = "#ffffff";
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
    return new Promise((resolve, reject) =>
      canvas.toBlob((blob) => blob ? resolve(blob) : reject(new Error("PNG encoding failed")),
        "image/png"));
  }

  function downloadBlob(blob) {
    const link = document.createElement("a");
    link.href = URL.createObjectURL(blob);
    link.download = `${location.hostname}-${slug}-qr.png`;
    link.click();
    URL.revokeObjectURL(link.href);
  }

  const flashLabel = (button, text, original) => {
    button.textContent = text;
    setTimeout(() => { button.textContent = original; }, 1500);
  };

  $("copy-qr").addEventListener("click", async (event) => {
    const button = event.currentTarget;
    try {
      const blob = await qrPngBlob();
      // Safari requires the ClipboardItem promise form; fall back to download.
      await navigator.clipboard.write([new ClipboardItem({ "image/png": blob })]);
      flashLabel(button, "QR copied", "Copy QR code");
    } catch {
      try {
        downloadBlob(await qrPngBlob());
        flashLabel(button, "Downloaded instead", "Copy QR code");
      } catch (err) {
        $("settings-dialog").close();
        showError(err);
      }
    }
  });

  $("download-qr").addEventListener("click", async (event) => {
    const button = event.currentTarget;
    try {
      downloadBlob(await qrPngBlob());
      flashLabel(button, "Downloaded", "Download QR");
    } catch (err) {
      $("settings-dialog").close();
      showError(err);
    }
  });


  // Key rotation: two-step like deletion. The server invalidates the old key
  // atomically; the new one is stored locally and shown exactly once.
  let rotateArmTimer = null;
  let rotatedSecret = null;
  $("rotate-key").addEventListener("click", async (event) => {
    const button = event.currentTarget;
    if (!button.dataset.armed) {
      button.dataset.armed = "1";
      button.textContent = "Click again to invalidate the old key";
      rotateArmTimer = setTimeout(() => {
        delete button.dataset.armed;
        button.textContent = "Generate new key";
      }, 5000);
      return;
    }
    clearTimeout(rotateArmTimer);
    delete button.dataset.armed;
    button.textContent = "Generate new key";
    try {
      const result = await api(`/api/pages/${encodeURIComponent(slug)}/rotate-key`, {
        method: "POST",
        headers: bearer(secret),
      });
      rotatedSecret = result.secret;
      localStorage.setItem(`abr_secret_${slug}`, rotatedSecret);
      $("new-secret-key").textContent = rotatedSecret;
      $("settings-dialog").close();
      $("new-key-dialog").showModal();
    } catch (err) {
      $("settings-dialog").close();
      showError(err);
    }
  });

  $("download-new-key").addEventListener("click", () => {
    if (!rotatedSecret) return;
    const blob = new Blob(
      [`Anonymous Book Recommendation page: ${window.location.origin}/u/${slug}\nSecret key: ${rotatedSecret}\n\nKeep this file private. The key cannot be recovered.\n`],
      { type: "text/plain" });
    const link = document.createElement("a");
    link.href = URL.createObjectURL(blob);
    link.download = `anonbookrec-key-${slug}.txt`;
    link.click();
    URL.revokeObjectURL(link.href);
  });
  $("new-key-saved").addEventListener("change", (event) => {
    $("new-key-done").disabled = !event.target.checked;
  });
  $("new-key-done").addEventListener("click", () => $("new-key-dialog").close());
  // However the dialog closes (Done or Esc), reload so every handler picks up
  // the new key; it is already saved in localStorage.
  $("new-key-dialog").addEventListener("close", () => window.location.reload());

  $("unlock-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const key = $("unlock-key").value.trim();
    if (!key) return;
    try {
      // Verify the key against the API before trusting it.
      await api(`/api/pages/${encodeURIComponent(slug)}`, { headers: bearer(key) });
      localStorage.setItem(`abr_secret_${slug}`, key);
      localStorage.setItem("abr_last_slug", slug);
      window.location.reload();
    } catch {
      $("unlock-status").textContent = "Invalid key.";
    }
  });

  $("logout").addEventListener("click", () => {
    localStorage.removeItem(`abr_secret_${slug}`);
    if (localStorage.getItem("abr_last_slug") === slug) {
      localStorage.removeItem("abr_last_slug");
    }
    window.location.reload();
  });

  // Account feedback stays inside the dialog: closing it on error would
  // swallow the message, and silent success feels like a dead button.
  const accountStatus = (message, isError = false) => {
    $("account-status").textContent = message;
    $("account-status").classList.toggle("error", isError);
  };

  $("set-credentials").addEventListener("click", async (event) => {
    const button = event.currentTarget;
    const username = $("account-username").value.trim();
    const password = $("account-password").value;
    if (!username || !password) {
      accountStatus("Enter both a username and a password.", true);
      return;
    }
    button.disabled = true;
    button.textContent = "Saving…";
    accountStatus("");
    try {
      const result = await api(`/api/pages/${encodeURIComponent(slug)}/credentials`, {
        method: "POST",
        headers: bearer(secret),
        body: JSON.stringify({ username, password }),
      });
      // The username is now the page's URL: move the stored secret to the new
      // key (keep the old one so slug URLs still unlock) and switch the URL.
      localStorage.setItem(`abr_secret_${result.handle}`, secret);
      localStorage.setItem("abr_last_slug", result.handle);
      history.replaceState(null, "", `/u/${encodeURIComponent(result.handle)}`);
      $("account-password").value = "";
      button.textContent = "Saved ✓";
      accountStatus(`You can now sign in as “${result.handle}”.  Your page lives at /u/${result.handle}.`);
      setStatus(`Password login enabled for ${result.handle}.`);
      await loadUserPage(pageSlug());
      setTimeout(() => { button.textContent = "Set username & password"; }, 2000);
    } catch (err) {
      button.textContent = "Set username & password";
      accountStatus(err.message, true);
    } finally {
      button.disabled = false;
    }
  });

  $("remove-credentials").addEventListener("click", async (event) => {
    const button = event.currentTarget;
    button.disabled = true;
    try {
      await api(`/api/pages/${encodeURIComponent(slug)}/credentials`, {
        method: "DELETE",
        headers: bearer(secret),
      });
      accountStatus("Password login removed. Your key still works, and your page URL is unchanged.");
      await loadUserPage(pageSlug());
    } catch (err) {
      accountStatus(err.message, true);
    } finally {
      button.disabled = false;
    }
  });

  // Deleting is two-step: first click arms the button, second click within
  // 5 seconds actually deletes.
  let deleteArmTimer = null;
  $("delete-account").addEventListener("click", async (event) => {
    const button = event.currentTarget;
    if (!button.dataset.armed) {
      button.dataset.armed = "1";
      button.textContent = "Click again to permanently delete";
      deleteArmTimer = setTimeout(() => {
        delete button.dataset.armed;
        button.textContent = "Delete page & all data";
      }, 5000);
      return;
    }
    clearTimeout(deleteArmTimer);
    try {
      await api(`/api/pages/${encodeURIComponent(slug)}`, {
        method: "DELETE",
        headers: bearer(secret),
      });
      localStorage.removeItem(`abr_secret_${slug}`);
      if (localStorage.getItem("abr_last_slug") === slug) {
        localStorage.removeItem("abr_last_slug");
      }
      window.location.href = "/";
    } catch (err) {
      $("settings-dialog").close();
      showError(err);
    }
  });

  // The public profile only exists for public pages.
  $("default-public").addEventListener("change", (event) => {
    $("profile-fields").disabled = !event.target.checked;
  });

  // Reveal the friend-link controls as soon as hiding is ticked — no need to
  // save and reopen the dialog first.
  $("hide-page").addEventListener("change", (event) => {
    $("share-zone").hidden = !event.target.checked;
  });

  $("save-settings").addEventListener("click", async () => {
    try {
      await api(`/api/pages/${encodeURIComponent(slug)}`, {
        method: "PATCH",
        headers: bearer(secret),
        body: JSON.stringify({
          default_public: $("default-public").checked,
          is_hidden: $("hide-page").checked,
          display_name: $("display-name").value,
          theme: $("theme").value,
          font: $("font").value,
          radius: $("radius").value,
          density: $("density").value,
          accent: $("accent-on").checked ? $("accent-color").value : "",
          bio: $("bio").value,
          favourite_book: $("favourite-book").value,
        }),
      });
      // Refresh currentPage BEFORE closing: the dialog's close handler
      // reverts styling to currentPage, which must be the just-saved state.
      await loadUserPage(slug);
      $("settings-dialog").close();
      setStatus("Settings saved.");
    } catch (err) { showError(err); }
  });
}

/* On the 404 page: if this browser holds a key for the requested card, it is
   probably the owner of a hidden page — go through the owner door, which
   serves the shell for any slug and lets the API decide. */
function initNotFound() {
  const path = window.location.pathname;
  if (!path.startsWith("/u/") || window.location.search.includes("unlock")) return;
  const slug = decodeURIComponent(path.split("/").pop());
  if (localStorage.getItem(`abr_secret_${slug}`)) {
    window.location.replace(`/u/${encodeURIComponent(slug)}?unlock`);
  }
}

if (document.body.dataset.page === "index") initIndex();
else if (document.body.dataset.page === "user") initUser();
else if (document.body.dataset.page === "not-found") initNotFound();
