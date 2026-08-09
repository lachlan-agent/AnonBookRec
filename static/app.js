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

async function loadUserPage(slug) {
  const secret = localStorage.getItem(`abr_secret_${slug}`);
  const page = await api(`/api/pages/${encodeURIComponent(slug)}`, {
    headers: secret ? bearer(secret) : {},
  });

  currentPage = page;
  $("unlock-panel").hidden = page.owner || Boolean(secret);
  document.body.dataset.theme = page.theme || "cloth";
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
      "bio": page.bio,
      "favourite-book": page.favourite_book,
    };
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
    const response = await fetch(`/api/pages/${encodeURIComponent(slug)}/qr.svg`);
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

  // Server-rendered QR of the share link, loaded on first use.
  $("show-qr").addEventListener("click", () => {
    const wrap = $("qr-wrap");
    const image = $("qr-image");
    if (!image.src) image.src = `/api/pages/${encodeURIComponent(slug)}/qr.svg`;
    wrap.hidden = !wrap.hidden;
    $("show-qr").textContent = wrap.hidden ? "Show QR code" : "Hide QR code";
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
          bio: $("bio").value,
          favourite_book: $("favourite-book").value,
        }),
      });
      $("settings-dialog").close();
      setStatus("Settings saved.");
      await loadUserPage(slug);
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
