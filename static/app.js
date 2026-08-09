"use strict";

const $ = (id) => document.getElementById(id);

async function api(path, options = {}) {
  const headers = { ...(options.headers || {}) };
  if (options.body) headers["Content-Type"] = "application/json";
  const response = await fetch(path, { ...options, headers });
  let data = null;
  try { data = await response.json(); } catch { /* non-JSON error body */ }
  if (!response.ok) {
    const message = (data && data.error) || `Request failed (${response.status})`;
    throw new Error(message);
  }
  return data;
}

function setStatus(message, isError = false) {
  const status = $("form-status");
  status.textContent = message;
  status.classList.toggle("error", isError);
}

const STATUS_TEXT = {
  created: (book) => `“${book.title}” added to the card.`,
  counted: (book) => `“${book.title}” was already here — count bumped.`,
  already_recommended: (book) => `You already recommended “${book.title}”.`,
};

function renderList(recommendations, { owner, secret, slug } = {}) {
  const list = $("rec-list");
  list.replaceChildren();
  $("empty-shelf").hidden = recommendations.length > 0;

  for (const rec of recommendations) {
    const item = document.createElement("li");

    const title = document.createElement("span");
    title.className = "rec-title" + (rec.read ? " read" : "");
    title.textContent = rec.book_name;
    item.append(title);

    const isbn = document.createElement("span");
    isbn.className = "rec-isbn";
    isbn.textContent = `ISBN ${rec.isbn}`;
    item.append(isbn);

    if (owner && !rec.is_public) {
      const badge = document.createElement("span");
      badge.className = "rec-private";
      badge.textContent = "sealed";
      item.append(badge);
    }

    const count = document.createElement("span");
    count.className = "rec-count";
    count.textContent = `× ${rec.recommendation_count}`;
    item.append(count);

    if (owner) item.append(ownerControls(rec, secret, slug));
    list.append(item);
  }
}

function ownerControls(rec, secret, slug) {
  const controls = document.createElement("div");
  controls.className = "rec-controls";

  const patch = async (body) => {
    await api(`/api/recommendations/${rec.id}`, {
      method: "PATCH",
      headers: { Authorization: `Bearer ${secret}` },
      body: JSON.stringify(body),
    });
    await loadUserPage(slug);
  };

  const readButton = document.createElement("button");
  readButton.className = "button small";
  readButton.type = "button";
  readButton.textContent = rec.read ? "Mark unread" : "Mark read";
  readButton.addEventListener("click", () => patch({ read: !rec.read }).catch(showError));
  controls.append(readButton);

  const visibilityButton = document.createElement("button");
  visibilityButton.className = "button small";
  visibilityButton.type = "button";
  visibilityButton.textContent = rec.is_public ? "Seal" : "Reveal";
  visibilityButton.addEventListener("click", () =>
    patch({ is_public: !rec.is_public }).catch(showError));
  controls.append(visibilityButton);

  const deleteButton = document.createElement("button");
  deleteButton.className = "button small danger";
  deleteButton.type = "button";
  deleteButton.textContent = "Remove";
  deleteButton.addEventListener("click", async () => {
    try {
      await api(`/api/recommendations/${rec.id}`, {
        method: "DELETE",
        headers: { Authorization: `Bearer ${secret}` },
      });
      await loadUserPage(slug);
    } catch (err) { showError(err); }
  });
  controls.append(deleteButton);

  return controls;
}

const showError = (err) => setStatus(err.message, true);

function wireSubmitForm(submitUrl, reload) {
  $("recommend-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const input = $("book-input");
    const name = input.value.trim();
    if (!name) return;
    setStatus("Checking the catalogue…");
    try {
      const result = await api(submitUrl, {
        method: "POST",
        body: JSON.stringify({ book_name: name }),
      });
      setStatus(STATUS_TEXT[result.status](result.book));
      input.value = "";
      await reload();
    } catch (err) { showError(err); }
  });
}

/* ---------- Landing page ---------- */

async function loadGlobal() {
  const recommendations = await api("/api/global");
  renderList(recommendations, {});
}

function initIndex() {
  loadGlobal().catch(showError);
  wireSubmitForm("/api/global/recommendations", loadGlobal);

  const lastSlug = localStorage.getItem("abr_last_slug");
  if (lastSlug && localStorage.getItem(`abr_secret_${lastSlug}`)) {
    $("returning").hidden = false;
    $("returning-link").href = `/u/${lastSlug}`;
  }

  $("create-page").addEventListener("click", async () => {
    try {
      const page = await api("/api/users", { method: "POST" });
      localStorage.setItem(`abr_secret_${page.slug}`, page.secret);
      localStorage.setItem("abr_last_slug", page.slug);
      $("page-url").textContent = page.url;
      $("secret-key").textContent = page.secret;
      const dialog = $("key-dialog");
      dialog.showModal();

      $("download-key").addEventListener("click", () => {
        const blob = new Blob(
          [`AnonBookRec page: ${page.url}\nSecret key: ${page.secret}\n\nKeep this file private. The key cannot be recovered.\n`],
          { type: "text/plain" });
        const link = document.createElement("a");
        link.href = URL.createObjectURL(blob);
        link.download = `anonbookrec-key-${page.slug}.txt`;
        link.click();
        URL.revokeObjectURL(link.href);
      });
      $("key-saved").addEventListener("change", (event) => {
        $("go-to-page").disabled = !event.target.checked;
      });
      $("go-to-page").addEventListener("click", () => {
        window.location.href = `/u/${page.slug}`;
      });
    } catch (err) { showError(err); }
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

/* ---------- User page ---------- */

function pageSlug() {
  return decodeURIComponent(window.location.pathname.split("/").pop());
}

async function loadUserPage(slug) {
  const secret = localStorage.getItem(`abr_secret_${slug}`);
  const headers = secret ? { Authorization: `Bearer ${secret}` } : {};
  const page = await api(`/api/pages/${encodeURIComponent(slug)}`, { headers });

  if (page.owner) {
    $("page-title").textContent = "Your card";
    $("page-sub").textContent =
      "Share your link and books will appear here. Only you can see sealed ones.";
    $("owner-panel").hidden = false;
    $("default-public").checked = page.default_public;
    $("shelf-note").textContent = "your recommendations";
  } else {
    $("shelf-note").textContent = "what others have recommended";
  }
  renderList(page.recommendations, { owner: page.owner, secret, slug });
  return page;
}

function initUser() {
  const slug = pageSlug();
  const secret = localStorage.getItem(`abr_secret_${slug}`);

  loadUserPage(slug).catch((err) => {
    $("page-title").textContent = "Page not found";
    $("page-sub").textContent = "This link doesn't lead to a card. Check the URL.";
    $("recommend-form").hidden = true;
    showError(err);
  });

  wireSubmitForm(`/api/pages/${encodeURIComponent(slug)}/recommendations`,
    () => loadUserPage(slug));

  $("copy-share").addEventListener("click", async () => {
    await navigator.clipboard.writeText(`${window.location.origin}/u/${slug}`);
    setStatus("Share link copied.");
  });

  $("default-public").addEventListener("change", async (event) => {
    try {
      await api(`/api/pages/${encodeURIComponent(slug)}`, {
        method: "PATCH",
        headers: { Authorization: `Bearer ${secret}` },
        body: JSON.stringify({ default_public: event.target.checked }),
      });
      setStatus(event.target.checked
        ? "New recommendations will be public."
        : "New recommendations will arrive sealed.");
    } catch (err) { showError(err); }
  });
}

if (document.body.dataset.page === "index") initIndex();
else if (document.body.dataset.page === "user") initUser();
