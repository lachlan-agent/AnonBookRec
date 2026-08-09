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

    for (const note of rec.notes || []) {
      const quote = document.createElement("blockquote");
      quote.className = "rec-note";
      quote.textContent = note;
      item.append(quote);
    }

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

/* Typeahead + confirmation. Nothing is submitted until the person confirms a
   specific edition (title, author, cover), so the wrong book can't slip in. */
function wireSubmitForm(submitUrl, reload) {
  const input = $("book-input");
  const dropdown = $("suggestions");
  const card = $("confirm-card");
  let debounceTimer = null;
  let suggestions = [];
  let activeIndex = -1;
  let selected = null;

  const looksLikeIsbn = (text) => /^[0-9\-\s]{10,17}[0-9Xx]?$/.test(text);
  const coverUrl = (book, size) =>
    book.cover_id
      ? `https://covers.openlibrary.org/b/id/${book.cover_id}-${size}.jpg`
      : `https://covers.openlibrary.org/b/isbn/${book.isbn13}-${size}.jpg`;

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
    $("confirm-author").textContent =
      [book.author, book.year].filter(Boolean).join(", ");
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
      const item = document.createElement("li");
      item.role = "option";
      item.className = "suggestion" + (index === activeIndex ? " active" : "");

      const cover = document.createElement("img");
      cover.className = "suggestion-cover";
      cover.alt = "";
      cover.loading = "lazy";
      cover.src = coverUrl(book, "S");
      cover.onerror = () => cover.removeAttribute("src");
      item.append(cover);

      const text = document.createElement("div");
      text.className = "suggestion-text";
      const title = document.createElement("span");
      title.className = "suggestion-title";
      title.textContent = book.title;
      const author = document.createElement("span");
      author.className = "suggestion-author";
      author.textContent = [book.author, book.year].filter(Boolean).join(", ");
      text.append(title, author);
      item.append(text);

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

  async function submitBook(bookName) {
    setStatus("Adding to the card…");
    const note = $("book-note").value.trim();
    try {
      const result = await api(submitUrl, {
        method: "POST",
        body: JSON.stringify({ book_name: bookName, note: note || null }),
      });
      setStatus(STATUS_TEXT[result.status](result.book));
      input.value = "";
      hideConfirm();
      hideDropdown();
      await reload();
    } catch (err) { showError(err); }
  }

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
      [`AnonBookRec page: ${createdPage.url}\nSecret key: ${createdPage.secret}\n\nKeep this file private. The key cannot be recovered.\n`],
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

  $("unlock-panel").hidden = page.owner || Boolean(secret);
  document.body.dataset.theme = page.theme || "cloth";
  $("page-bio").hidden = !page.bio;
  $("page-bio").textContent = page.bio || "";
  $("page-fav").hidden = !page.favourite_book;
  $("page-fav").textContent = page.favourite_book
    ? `Favourite book: ${page.favourite_book}` : "";

  if (page.owner) {
    $("page-title").textContent = page.display_name
      ? `${page.display_name}'s card` : "Your card";
    $("page-sub").textContent =
      "Share your link and books will appear here. Only you can see sealed ones.";
    $("owner-panel").hidden = false;
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
    $("shelf-note").textContent = "your recommendations";
  } else {
    $("page-title").textContent = page.display_name
      ? `Recommend ${page.display_name} a book`
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

  $("unlock-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const key = $("unlock-key").value.trim();
    if (!key) return;
    try {
      // Verify the key against the API before trusting it.
      await api(`/api/pages/${encodeURIComponent(slug)}`, {
        headers: { Authorization: `Bearer ${key}` },
      });
      localStorage.setItem(`abr_secret_${slug}`, key);
      localStorage.setItem("abr_last_slug", slug);
      window.location.reload();
    } catch {
      $("unlock-status").textContent = "That key doesn't open this card.";
    }
  });

  $("logout").addEventListener("click", () => {
    localStorage.removeItem(`abr_secret_${slug}`);
    if (localStorage.getItem("abr_last_slug") === slug) {
      localStorage.removeItem("abr_last_slug");
    }
    window.location.reload();
  });

  // The public profile only exists for public pages.
  $("default-public").addEventListener("change", (event) => {
    $("profile-fields").disabled = !event.target.checked;
  });

  $("save-settings").addEventListener("click", async () => {
    try {
      await api(`/api/pages/${encodeURIComponent(slug)}`, {
        method: "PATCH",
        headers: { Authorization: `Bearer ${secret}` },
        body: JSON.stringify({
          default_public: $("default-public").checked,
          display_name: $("display-name").value,
          theme: $("theme").value,
          bio: $("bio").value,
          favourite_book: $("favourite-book").value,
        }),
      });
      setStatus("Settings saved.");
      await loadUserPage(slug);
    } catch (err) { showError(err); }
  });
}

if (document.body.dataset.page === "index") initIndex();
else if (document.body.dataset.page === "user") initUser();
