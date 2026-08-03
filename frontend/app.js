const MAX_BLOCK_WEIGHT = 4_000_000;
const API_POLL_INTERVAL = 4_000;
const MEMPOOL_REFRESH_INTERVAL = 30_000;

const DEFAULT_SETTINGS = Object.freeze({
  auto_selection_enabled: false,
  selection_strategy: "maximizeFees",
  min_fee_rate: 1,
  max_size: 1_000_000,
  min_base_fee: 0,
  max_ancestor_count: 25,
  max_descendant_count: 25,
  exclude_bip125_replaceable: false,
  exclude_unbroadcast: false,
  max_transaction_count: 100,
  require_template: false,
  clear_existing_selections: true,
  periodic_enabled: false,
  periodic_interval: 30,
  auto_job_declaration: false,
  auto_scroll_to_table: true,
  show_notifications: true,
  pause_on_selection: false,
  clear_selection_on_job_declaration: false,
  preserve_existing_selections: true,
  auto_clean_invalid_transactions: true,
});

const THEME_OPTIONS = [
  ["default", "Default"],
  ["blue", "Blue"],
  ["green", "Green"],
  ["amber", "Amber"],
  ["default-scaled", "Default · Scaled"],
  ["blue-scaled", "Blue · Scaled"],
  ["mono-scaled", "Mono · Scaled"],
];

const ROUTES = {
  "/dashboard/overview": "Overview",
  "/dashboard/job-history": "Job History",
  "/dashboard/settings": "Settings",
};

const state = {
  route: normalizeRoute(window.location.pathname),
  theme: localStorage.getItem("demand-theme") || "default",
  mode: localStorage.getItem("demand-mode") || "light",
  sidebarExpanded: false,
  stats: {
    loading: true,
    health: null,
    pool: null,
    aggregate: null,
    system: null,
    miners: null,
    error: null,
    errors: {},
  },
  mempool: [],
  mempoolLoaded: false,
  mempoolError: null,
  selected: new Set(),
  search: "",
  filters: { feeRate: null, vsize: null, baseFee: null, depends: "" },
  sort: { key: "feeRate", direction: "desc" },
  tablePage: 1,
  tablePageSize: 10,
  tableView: "all",
  paused: false,
  templateId: null,
  declaring: false,
  autoSelecting: false,
  logs: [],
  settings: { ...DEFAULT_SETTINGS },
  settingsLoaded: false,
  jobs: [],
  jobPage: 1,
  jobPerPage: 10,
  jobTotal: 0,
  jobTotalPages: 0,
  jobsLoading: false,
  jobsError: null,
  sockets: {},
  reconnectTimers: {},
  periodicTimer: null,
};

const ICON_PATHS = {
  dashboard:
    '<rect width="6" height="6" x="3" y="3" rx="1"/><rect width="6" height="6" x="15" y="3" rx="1"/><rect width="6" height="6" x="3" y="15" rx="1"/><rect width="6" height="6" x="15" y="15" rx="1"/>',
  history:
    '<path d="M3 12a9 9 0 1 0 3-6.7L3 8"/><path d="M3 3v5h5"/><path d="M12 7v5l3 2"/>',
  settings:
    '<path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.38a2 2 0 0 0-.73-2.73l-.15-.09a2 2 0 0 1-1-1.74v-.51a2 2 0 0 1 1-1.74l.15-.08a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/>',
  panel: '<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M9 3v18"/>',
  theme:
    '<circle cx="12" cy="12" r="9"/><path d="M12 3v18M12 12l6.4-6.4M12 12l6.4 6.4"/>',
  chart: '<path d="M3 3v18h18"/><path d="M7 16v-5M12 16V7M17 16v-9"/>',
  globe:
    '<circle cx="12" cy="12" r="10"/><path d="M2 12h20M12 2a15.3 15.3 0 0 1 0 20M12 2a15.3 15.3 0 0 0 0 20"/><path d="M8 22h8"/>',
  monitor: '<rect width="20" height="14" x="2" y="3" rx="2"/><path d="M8 21h8M12 17v4"/>',
  server:
    '<rect width="20" height="8" x="2" y="2" rx="2"/><rect width="20" height="8" x="2" y="14" rx="2"/><path d="M6 6h.01M6 18h.01"/>',
  cpu:
    '<rect width="16" height="16" x="4" y="4" rx="2"/><rect width="6" height="6" x="9" y="9" rx="1"/><path d="M9 1v3M15 1v3M9 20v3M15 20v3M20 9h3M20 14h3M1 9h3M1 14h3"/>',
  activity: '<path d="M3 12h4l2-7 4 14 2-7h6"/>',
  trend: '<path d="m3 17 6-6 4 4 8-8"/><path d="M15 7h6v6"/>',
  check: '<path d="M20 6 9 17l-5-5"/>',
  checkCircle: '<circle cx="12" cy="12" r="10"/><path d="m8 12 3 3 5-6"/>',
  play: '<path d="m6 3 14 9-14 9z"/>',
  sliders: '<path d="M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3"/><path d="M1 14h6M9 8h6M17 16h6"/>',
  pause: '<rect width="4" height="16" x="6" y="4" rx="1"/><rect width="4" height="16" x="14" y="4" rx="1"/>',
  sort: '<path d="m3 8 4-4 4 4M7 4v16M21 16l-4 4-4-4M17 20V4"/>',
  plus: '<circle cx="12" cy="12" r="9"/><path d="M12 8v8M8 12h8"/>',
  x: '<path d="M18 6 6 18M6 6l12 12"/>',
  refresh: '<path d="M20 11a8 8 0 1 0 2 5.3"/><path d="M20 4v7h-7"/>',
  user: '<path d="M19 21v-2a7 7 0 0 0-14 0v2"/><circle cx="12" cy="7" r="4"/>',
  bell: '<path d="M10.3 21a2 2 0 0 0 3.4 0M18 8a6 6 0 0 0-12 0c0 7-3 7-3 9h18c0-2-3-2-3-9"/>',
  download: '<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4M7 10l5 5 5-5M12 15V3"/>',
  upload: '<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4M17 8l-5-5-5 5M12 3v12"/>',
  copy: '<rect width="14" height="14" x="8" y="8" rx="2"/><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2"/>',
  external: '<path d="M15 3h6v6M10 14 21 3"/><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/>',
  hash: '<path d="M4 9h16M4 15h16M10 3 8 21M16 3l-2 18"/>',
  chevronLeft: '<path d="m15 18-6-6 6-6"/>',
  chevronRight: '<path d="m9 18 6-6-6-6"/>',
  chevronsLeft: '<path d="m11 17-5-5 5-5M18 17l-5-5 5-5"/>',
  chevronsRight: '<path d="m13 17 5-5-5-5M6 17l5-5-5-5"/>',
  eye: '<path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12"/><circle cx="12" cy="12" r="3"/>',
  trash: '<path d="M3 6h18M8 6V4h8v2M19 6l-1 15H6L5 6M10 11v5M14 11v5"/>',
  undo: '<path d="M3 7v6h6"/><path d="M3 13a9 9 0 1 0 3-7.7L3 8"/>',
  info: '<circle cx="12" cy="12" r="10"/><path d="M12 16v-4M12 8h.01"/>',
};

function icon(name, size = 18, className = "") {
  const paths = ICON_PATHS[name] || ICON_PATHS.info;
  return `<svg class="${className}" width="${size}" height="${size}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${paths}</svg>`;
}

function normalizeRoute(path) {
  const clean = path.replace(/\.html$/, "").replace(/\/$/, "") || "/";
  if (clean === "/" || clean === "/dashboard") return "/dashboard/overview";
  if (clean === "/overview") return "/dashboard/overview";
  if (clean === "/history") return "/dashboard/job-history";
  if (clean === "/settings") return "/dashboard/settings";
  return ROUTES[clean] ? clean : "/dashboard/overview";
}

function escapeHtml(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;");
}

function formatNumber(value, maximumFractionDigits = 0) {
  const number = Number(value);
  if (!Number.isFinite(number)) return "N/A";
  return number.toLocaleString(undefined, { maximumFractionDigits });
}

function formatHashrate(value, decimals = 2) {
  const number = Number(value);
  if (!Number.isFinite(number)) return "N/A";
  if (number >= 1e18) return `${(number / 1e18).toFixed(decimals)} EH/s`;
  if (number >= 1e15) return `${(number / 1e15).toFixed(decimals)} PH/s`;
  if (number >= 1e12) return `${(number / 1e12).toFixed(decimals)} TH/s`;
  if (number >= 1e9) return `${(number / 1e9).toFixed(decimals)} GH/s`;
  if (number >= 1e6) return `${(number / 1e6).toFixed(decimals)} MH/s`;
  return `${number.toFixed(decimals)} H/s`;
}

function formatBytes(value) {
  const bytes = Number(value);
  if (!Number.isFinite(bytes)) return "N/A";
  const units = ["Bytes", "KB", "MB", "GB", "TB"];
  if (bytes === 0) return "0 Bytes";
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  return `${(bytes / 1024 ** index).toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
}

function formatDate(value) {
  if (!value) return "N/A";
  const raw = Number(value);
  const date = Number.isFinite(raw)
    ? new Date(raw < 10_000_000_000 ? raw * 1000 : raw)
    : new Date(value);
  if (Number.isNaN(date.getTime())) return "N/A";
  return new Intl.DateTimeFormat("en-US", {
    month: "long",
    day: "numeric",
    year: "numeric",
    hour: "numeric",
    minute: "2-digit",
  }).format(date);
}

function shortHash(value, front = 8, back = 8) {
  const text = String(value || "");
  return text.length > front + back + 3
    ? `${text.slice(0, front)}...${text.slice(-back)}`
    : text;
}

function currentPageElement() {
  return document.querySelector("#page-content");
}

function applyAppearance() {
  document.documentElement.dataset.theme = state.theme;
  document.documentElement.dataset.mode = state.mode;
  document.querySelector('meta[name="theme-color"]')?.setAttribute(
    "content",
    state.mode === "dark" ? "#09090b" : "#ffffff",
  );
}

function switchControl(key, checked, label) {
  return `<label class="switch" aria-label="${escapeHtml(label)}">
    <input type="checkbox" name="${escapeHtml(key)}" ${checked ? "checked" : ""} />
    <span class="switch-track"></span>
  </label>`;
}

function renderShell() {
  const app = document.querySelector("#app");
  app.innerHTML = `<div class="app-shell">
    <aside id="sidebar" class="sidebar">
      <nav class="sidebar-nav" aria-label="Dashboard navigation">
        ${sidebarLink("/dashboard/overview", "dashboard", "Dashboard")}
        ${sidebarLink("/dashboard/job-history", "history", "Job History")}
        ${sidebarLink("/dashboard/settings", "settings", "Settings")}
      </nav>
    </aside>
    <div class="main-shell">
      <header class="topbar">
        <div class="topbar-left">
          <button class="icon-btn btn ghost" type="button" data-action="toggle-sidebar" aria-label="Toggle sidebar">${icon("panel", 19)}</button>
          <nav class="breadcrumbs" aria-label="Breadcrumb">
            <a class="breadcrumb-parent" href="/dashboard/overview" data-nav="/dashboard/overview">Dashboard</a>
            <span class="breadcrumb-separator">/</span>
            <span id="breadcrumb-current" class="breadcrumb-current">${escapeHtml(ROUTES[state.route])}</span>
          </nav>
        </div>
        <div class="topbar-actions">
          <button class="icon-btn secondary" type="button" data-action="toggle-mode" aria-label="Toggle dark mode">${icon("theme", 18)}</button>
          <label class="theme-select-wrap">
            <span class="theme-select-label">Select a theme:</span>
            <select id="theme-select" class="theme-select" aria-label="Theme">
              ${THEME_OPTIONS.map(([value, name]) => `<option value="${value}" ${value === state.theme ? "selected" : ""}>${name}</option>`).join("")}
            </select>
          </label>
        </div>
      </header>
      <div class="page-scroll"><main id="page-content"></main></div>
    </div>
  </div>`;
  updateNavigation();
  renderRoute();
}

function sidebarLink(route, iconName, title) {
  return `<a class="sidebar-link" href="${route}" data-nav="${route}">
    ${icon(iconName, 18)}<span class="sidebar-label">${title}</span><span class="sidebar-tooltip">${title}</span>
  </a>`;
}

function updateNavigation() {
  document.querySelectorAll("[data-nav]").forEach((link) => {
    link.classList.toggle("active", link.dataset.nav === state.route);
  });
  const breadcrumb = document.querySelector("#breadcrumb-current");
  if (breadcrumb) breadcrumb.textContent = ROUTES[state.route] || "Overview";
  document.title = `${ROUTES[state.route] || "Dashboard"} · Demand Dashboard`;
}

function navigate(route, replace = false) {
  const normalized = normalizeRoute(route);
  if (replace) window.history.replaceState({}, "", normalized);
  else if (normalized !== state.route) window.history.pushState({}, "", normalized);
  state.route = normalized;
  updateNavigation();
  renderRoute();
  document.querySelector("#sidebar")?.classList.remove("mobile-open");
}

function renderRoute() {
  closeModal();
  if (state.route === "/dashboard/job-history") renderJobHistory();
  else if (state.route === "/dashboard/settings") renderSettings();
  else renderOverview();
}

async function apiRequest(path, options = {}) {
  const headers = new Headers(options.headers || {});
  if (options.body && !headers.has("Content-Type")) headers.set("Content-Type", "application/json");
  const response = await fetch(path, { ...options, headers });
  let payload = null;
  const contentType = response.headers.get("content-type") || "";
  if (contentType.includes("application/json")) payload = await response.json();
  if (!response.ok) {
    const message = payload?.message || `Request failed (${response.status})`;
    throw new Error(message);
  }
  return payload;
}

async function envelopeRequest(path, options = {}) {
  const payload = await apiRequest(path, options);
  if (!payload?.success) throw new Error(payload?.message || "Request failed");
  return payload.data;
}

function addLog(event, level, message) {
  state.logs.unshift({
    event,
    level,
    message,
    timestamp: new Date().toLocaleString(),
  });
  state.logs = state.logs.slice(0, 100);
  if (state.route === "/dashboard/overview") renderLogs();
}

function toast(title, description = "", type = "info", duration = 5_000) {
  if (!state.settings.show_notifications && type !== "error") return;
  const root = document.querySelector("#toast-root");
  const item = document.createElement("div");
  item.className = `toast ${type}`;
  item.innerHTML = `<div class="toast-title">${escapeHtml(title)}</div>${description ? `<div class="toast-description">${escapeHtml(description)}</div>` : ""}`;
  root.append(item);
  window.setTimeout(() => item.remove(), duration);
}

function openModal({ title, description = "", body = "", size = "large" }) {
  const root = document.querySelector("#modal-root");
  root.innerHTML = `<div class="modal-backdrop" data-action="close-modal">
    <section class="modal ${size}" role="dialog" aria-modal="true" aria-labelledby="modal-title" data-modal-panel>
      <header class="modal-header">
        <div><h3 id="modal-title" class="modal-heading">${escapeHtml(title)}</h3>${description ? `<p class="modal-description">${escapeHtml(description)}</p>` : ""}</div>
        <button class="icon-btn btn ghost" type="button" data-action="close-modal" aria-label="Close">${icon("x", 18)}</button>
      </header>
      <div class="modal-body">${body}</div>
    </section>
  </div>`;
  requestAnimationFrame(() => root.querySelector("button, input, select")?.focus());
}

function closeModal() {
  const root = document.querySelector("#modal-root");
  if (root) root.innerHTML = "";
}

async function copyText(text) {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text);
    return;
  }
  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.style.position = "fixed";
  textarea.style.opacity = "0";
  document.body.append(textarea);
  textarea.select();
  document.execCommand("copy");
  textarea.remove();
}

function downloadText(text, filename, type = "text/plain;charset=utf-8") {
  const url = URL.createObjectURL(new Blob([text], { type }));
  const link = document.createElement("a");
  link.href = url;
  link.download = filename;
  document.body.append(link);
  link.click();
  link.remove();
  URL.revokeObjectURL(url);
}

async function pollStats() {
  const requests = await Promise.allSettled([
    envelopeRequest("/api/health"),
    envelopeRequest("/api/pool/info"),
    envelopeRequest("/api/stats/aggregate"),
    envelopeRequest("/api/stats/system"),
  ]);
  state.stats.loading = false;
  state.stats.health = requests[0].status === "fulfilled" ? requests[0].value : null;
  state.stats.pool = requests[1].status === "fulfilled" ? requests[1].value : null;
  state.stats.aggregate = requests[2].status === "fulfilled" ? requests[2].value : null;
  state.stats.system = requests[3].status === "fulfilled" ? requests[3].value : null;
  state.stats.errors = {
    health: requests[0].status === "rejected" ? requests[0].reason?.message : null,
    pool: requests[1].status === "rejected" ? requests[1].reason?.message : null,
    aggregate: requests[2].status === "rejected" ? requests[2].reason?.message : null,
    system: requests[3].status === "rejected" ? requests[3].reason?.message : null,
  };
  state.stats.error = state.stats.errors.health;
  if (state.route === "/dashboard/overview") updateStatsUI();
}

async function loadMiners() {
  try {
    state.stats.miners = await envelopeRequest("/api/stats/miners");
  } catch (error) {
    state.stats.miners = null;
  }
}

function normalizeTransaction(transaction) {
  const vsize = Number(transaction.vsize) || 0;
  const baseRaw = Number(transaction.fees?.base) || 0;
  const baseSat = Math.abs(baseRaw) < 21_000_000 ? baseRaw * 100_000_000 : baseRaw;
  return {
    ...transaction,
    txid: String(transaction.txid || ""),
    vsize,
    weight: Number(transaction.weight) || vsize * 4,
    feeRate: Number(transaction.feeRate) || Number(transaction.fee_rate) || (vsize ? baseSat / vsize : 0),
    baseFee: baseSat,
    ancestor_count: Number(transaction.ancestor_count) || 0,
    ancestor_size: Number(transaction.ancestor_size) || 0,
    descendant_count: Number(transaction.descendant_count) || 0,
    descendant_size: Number(transaction.descendant_size) || 0,
    depends: Array.isArray(transaction.depends) ? transaction.depends.map(String) : [],
    spent_by: Array.isArray(transaction.spent_by) ? transaction.spent_by.map(String) : [],
    bip125_replaceable: Boolean(transaction.bip125_replaceable),
    unbroadcast: Boolean(transaction.unbroadcast),
  };
}

async function loadMempool(force = false) {
  if (state.paused && !force) return;
  try {
    const transactions = await apiRequest("/api/mempool");
    state.mempool = Array.isArray(transactions) ? transactions.map(normalizeTransaction) : [];
    state.mempoolLoaded = true;
    state.mempoolError = null;
  } catch (error) {
    state.mempoolLoaded = true;
    state.mempoolError = error.message;
  }
  if (state.route === "/dashboard/overview") {
    renderMempoolTable();
    updateSelectionUI();
  }
}

function socketUrl(path) {
  const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
  return `${protocol}//${window.location.host}${path}`;
}

function connectSocket(key, path, handlers) {
  if (state.sockets[key]?.readyState === WebSocket.OPEN || state.sockets[key]?.readyState === WebSocket.CONNECTING) return;
  clearTimeout(state.reconnectTimers[key]);
  let socket;
  try {
    socket = new WebSocket(socketUrl(path));
  } catch (error) {
    state.reconnectTimers[key] = setTimeout(() => connectSocket(key, path, handlers), 3_000);
    return;
  }
  state.sockets[key] = socket;
  socket.addEventListener("open", () => handlers.open?.());
  socket.addEventListener("message", (event) => handlers.message?.(event));
  socket.addEventListener("error", () => handlers.error?.());
  socket.addEventListener("close", () => {
    handlers.close?.();
    state.reconnectTimers[key] = setTimeout(() => connectSocket(key, path, handlers), 3_000);
  });
}

function connectWebSockets() {
  connectSocket("jd", "/ws/jd/stream", {
    open: () => addLog("WebSocketOpen", "INFO", "Connected to job declaration stream"),
    message: (event) => {
      try {
        const notification = JSON.parse(event.data);
        addLog(notification.event || "JobEvent", "INFO", notification.message || "Job declaration event received");
        if (notification.event === "NewTemplate") {
          const alreadySeen = state.templateId === notification.template_id;
          state.templateId = notification.template_id;
          if (!alreadySeen) {
            toast("NewTemplate", notification.message || `Template ${notification.template_id} is ready`);
            if (state.settings.auto_selection_enabled) runAutoSelection("template");
          }
        } else if (["RequestTransactionDataSuccess", "RequestTransactionDataTimeout"].includes(notification.event)) {
          state.templateId = null;
        }
      } catch (error) {
        addLog("WebSocketParseError", "ERROR", `Failed to parse WebSocket message: ${error.message}`);
      }
    },
    error: () => addLog("WebSocketError", "ERROR", "WebSocket connection error"),
    close: () => addLog("WebSocketClose", "WARNING", "Disconnected from job declaration stream; reconnecting…"),
  });

  connectSocket("mempool", "/ws/bitcoin/stream", {
    message: (event) => {
      if (state.paused) return;
      try {
        applyMempoolEvent(JSON.parse(event.data));
      } catch (error) {
        console.warn("Unable to process mempool event", error);
      }
    },
  });
}

function applyMempoolEvent(event) {
  if (event.event === "A" && event.transaction) {
    const transaction = normalizeTransaction(event.transaction);
    const index = state.mempool.findIndex((item) => item.txid === transaction.txid);
    if (index >= 0) state.mempool[index] = transaction;
    else state.mempool.unshift(transaction);
  } else if (event.event === "R" && event.txid) {
    state.mempool = state.mempool.filter((item) => item.txid !== String(event.txid));
  } else if (event.event === "C" && event.block?.txids) {
    const mined = new Set(event.block.txids.map(String));
    state.mempool = state.mempool.filter((item) => !mined.has(item.txid));
  } else if (event.event === "D" && Array.isArray(event.transactions)) {
    const current = new Map(state.mempool.map((item) => [item.txid, item]));
    event.transactions.map(normalizeTransaction).forEach((item) => current.set(item.txid, item));
    state.mempool = [...current.values()];
  }
  if (state.route === "/dashboard/overview") {
    renderMempoolTable();
    updateSelectionUI();
  }
}

function renderOverview() {
  const page = currentPageElement();
  page.className = "page compact-top";
  page.innerHTML = `<section class="page-header">
    <div><h1 class="page-title">Hi, Welcome back 👋</h1></div>
    <div class="page-actions">
      <button class="btn" type="button" data-action="open-stats">${icon("chart", 17)} Detailed Stats</button>
      <span id="health-badge" class="status-badge"><span class="status-dot"></span>Connecting...</span>
    </div>
  </section>
  <section class="stats-grid" aria-label="Mining statistics">
    ${statCard("pool-card", "globe", "Pool Address", "Loading…", "Latency:", "Loading…", "")}
    ${statCard("devices-card", "monitor", "Connected Devices", "—", "Mining devices online", "Total active miners", "Active")}
    ${statCard("hashrate-card", "server", "Total Hashrate", "—", "Combined mining power", "Aggregate hash performance", "Mining")}
    ${statCard("cpu-card", "cpu", "CPU Usage", "—", "System performance", "Memory: —", "Normal")}
  </section>
  <section class="card validation-card" id="validation-card">
    <div class="validation-head">
      <div class="validation-title"><span class="success-text">${icon("checkCircle", 17)}</span> Bitcoin Mining Validation</div>
      <span id="validation-badge" class="badge solid">Valid Selection</span>
    </div>
    <p id="validation-copy" class="validation-copy">No transactions selected</p>
    <div class="validation-label-row"><span>Block Weight Usage:</span><span id="validation-percent" class="validation-percent">0.0%</span></div>
    <div class="progress"><div id="validation-progress" class="progress-bar"></div></div>
    <div class="validation-foot"><span id="validation-weight">0 / 4,000,000 weight units</span><span id="validation-remaining">4,000,000 remaining</span></div>
  </section>
  <section class="card transactions-card data-table-container">
    <div class="toolbar-row">
      <button id="run-auto-button" class="btn" type="button" data-action="run-auto">${icon("play", 16)} Run Auto-Selection</button>
      <button class="btn" type="button" data-action="open-auto-settings">${icon("settings", 16)} Settings</button>
    </div>
    <div class="table-toolbar">
      <div class="table-toolbar-left">
        <input id="tx-search" class="input search-input" type="search" value="${escapeHtml(state.search)}" placeholder="Search by txid" autocomplete="off" />
        ${filterChip("feeRate", "Fee Rate")}${filterChip("vsize", "vsize")}${filterChip("baseFee", "Base Fee")}${filterChip("depends", "Depends On")}
      </div>
      <div class="table-toolbar-right">
        <select id="view-control" class="select-control" aria-label="Table view">
          <option value="all" ${state.tableView === "all" ? "selected" : ""}>View · All</option>
          <option value="compact" ${state.tableView === "compact" ? "selected" : ""}>View · Compact</option>
        </select>
        <button class="btn ${state.paused ? "primary" : "destructive"}" type="button" data-action="toggle-pause">${state.paused ? icon("play", 16) + " Resume" : icon("pause", 16) + " Pause"}</button>
        <select id="sort-control" class="select-control" aria-label="Sort transactions">
          ${sortOptions()}
        </select>
      </div>
    </div>
    <div id="mempool-table"></div>
    <div id="selection-action-bar" class="selection-action-bar" hidden></div>
  </section>
  <section class="card logs-card">
    <div class="validation-head"><div class="validation-title">${icon("history", 17)} Logs</div><span class="badge">Latest 100</span></div>
    <div id="logs-list" class="logs-list"></div>
  </section>`;
  updateStatsUI();
  renderMempoolTable();
  updateSelectionUI();
  renderLogs();
  if (!state.mempoolLoaded) loadMempool();
}

function statCard(id, iconName, label, value, footTitle, footMuted, badge) {
  return `<article id="${id}" class="card stat-card">
    <div><div class="stat-card-top"><div><div class="stat-label">${icon(iconName, 17)} ${label}</div><div class="stat-value" data-stat-value>${value}</div></div>${badge ? `<span class="badge">${badge === "Mining" ? icon("trend", 13) : icon("activity", 13)} ${badge}</span>` : ""}</div></div>
    <div><div class="stat-foot-title"><span data-stat-foot-title>${footTitle}</span> ${footTitle.includes("Latency") ? "" : icon("trend", 14)}</div><div class="stat-foot-muted" data-stat-foot-muted>${footMuted}</div></div>
  </article>`;
}

function updateStatsUI() {
  const healthBadge = document.querySelector("#health-badge");
  if (!healthBadge) return;
  if (state.stats.loading) {
    healthBadge.className = "status-badge";
    healthBadge.innerHTML = '<span class="status-dot"></span>Connecting...';
  } else if (state.stats.error) {
    healthBadge.className = "status-badge error";
    healthBadge.innerHTML = `<span class="status-dot"></span>${escapeHtml(state.stats.error)}`;
  } else {
    healthBadge.className = "status-badge";
    healthBadge.innerHTML = `<span class="status-dot"></span>${escapeHtml(typeof state.stats.health === "string" ? state.stats.health : state.stats.health?.status || "Proxy OK")}`;
  }

  const pool = state.stats.pool;
  updateStatCard(
    "pool-card",
    pool?.address || (state.stats.errors.pool ? `Error: ${state.stats.errors.pool}` : "N/A"),
    "Latency:",
    `${pool?.latency ?? "N/A"} ms`,
  );
  const aggregate = state.stats.aggregate;
  updateStatCard(
    "devices-card",
    aggregate?.total_connected_device ?? "N/A",
    "Mining devices online",
    "Total active miners",
  );
  updateStatCard(
    "hashrate-card",
    aggregate ? formatHashrate(aggregate.aggregate_hashrate) : "N/A",
    "Combined mining power",
    "Aggregate hash performance",
  );
  const system = state.stats.system;
  const cpu = Number(system?.["cpu_usage_%"] ?? system?.cpu_usage);
  updateStatCard(
    "cpu-card",
    Number.isFinite(cpu) ? `${cpu.toFixed(1)}%` : "N/A",
    "System performance",
    `Memory: ${system ? formatBytes(system.memory_usage_bytes ?? system.memory_usage) : "N/A"}`,
  );
}

function updateStatCard(id, value, footTitle, footMuted) {
  const card = document.querySelector(`#${id}`);
  if (!card) return;
  card.querySelector("[data-stat-value]").textContent = value;
  card.querySelector("[data-stat-foot-title]").textContent = footTitle;
  card.querySelector("[data-stat-foot-muted]").textContent = footMuted;
}

function filterChip(key, label) {
  const value = state.filters[key];
  const display = value !== null && value !== "" ? `${label}: ${value}` : label;
  return `<button class="filter-chip ${value !== null && value !== "" ? "active" : ""}" type="button" data-filter="${key}">${icon("plus", 15)} ${escapeHtml(display)}</button>`;
}

function sortOptions() {
  const options = [
    ["feeRate:desc", "Sort · Fee rate ↓"],
    ["feeRate:asc", "Sort · Fee rate ↑"],
    ["vsize:desc", "Sort · Size ↓"],
    ["vsize:asc", "Sort · Size ↑"],
    ["baseFee:desc", "Sort · Base fee ↓"],
    ["time:desc", "Sort · Newest"],
  ];
  const current = `${state.sort.key}:${state.sort.direction}`;
  return options.map(([value, label]) => `<option value="${value}" ${value === current ? "selected" : ""}>${label}</option>`).join("");
}

function filteredTransactions() {
  const search = state.search.trim().toLowerCase();
  const filtered = state.mempool.filter((transaction) => {
    if (search && !transaction.txid.toLowerCase().includes(search)) return false;
    if (state.filters.feeRate !== null && transaction.feeRate < Number(state.filters.feeRate)) return false;
    if (state.filters.vsize !== null && transaction.vsize > Number(state.filters.vsize)) return false;
    if (state.filters.baseFee !== null && transaction.baseFee < Number(state.filters.baseFee)) return false;
    if (state.filters.depends && !transaction.depends.some((item) => item.includes(state.filters.depends))) return false;
    return true;
  });
  const direction = state.sort.direction === "asc" ? 1 : -1;
  return filtered.sort((a, b) => {
    const left = a[state.sort.key] ?? 0;
    const right = b[state.sort.key] ?? 0;
    return (left > right ? 1 : left < right ? -1 : 0) * direction;
  });
}

function renderMempoolTable() {
  const root = document.querySelector("#mempool-table");
  if (!root) return;
  const rows = filteredTransactions();
  const totalPages = Math.max(1, Math.ceil(rows.length / state.tablePageSize));
  state.tablePage = Math.min(state.tablePage, totalPages);
  const start = (state.tablePage - 1) * state.tablePageSize;
  const visibleRows = rows.slice(start, start + state.tablePageSize);
  const allVisibleSelected = visibleRows.length > 0 && visibleRows.every((row) => state.selected.has(row.txid));
  const compact = state.tableView === "compact";
  root.innerHTML = `<div class="table-shell">
    <table class="data-table">
      <thead><tr>
        <th><input type="checkbox" data-action="select-page" aria-label="Select page" ${allVisibleSelected ? "checked" : ""} /></th>
        <th>Txid</th><th><span class="sort-header">FeeRate (sat/vB) ${icon("sort", 13)}</span></th><th><span class="sort-header">Size (vB) ${icon("sort", 13)}</span></th><th><span class="sort-header">Base Fee (sat) ${icon("sort", 13)}</span></th>
        ${compact ? "" : `<th>Depends On</th><th>Descendant Count</th><th>Descendant Size</th><th>Ancestor Count</th><th>Ancestor Size</th><th>Time</th>`}
      </tr></thead>
      <tbody>${renderTransactionRows(visibleRows, compact)}</tbody>
    </table>
  </div>
  <div class="table-pagination">
    <span>${state.selected.size ? `${state.selected.size} of ${rows.length} row(s) selected.` : `${rows.length} row(s) total.`}</span>
    <div class="pagination-controls">
      <label class="nowrap">Rows per page <select id="table-page-size" class="select-control small"><option>10</option><option>20</option><option>30</option><option>50</option></select></label>
      <span class="nowrap">Page ${state.tablePage} of ${totalPages}</span>
      <button class="icon-btn" type="button" data-table-page="first" ${state.tablePage <= 1 ? "disabled" : ""} aria-label="First page">${icon("chevronsLeft", 15)}</button>
      <button class="icon-btn" type="button" data-table-page="previous" ${state.tablePage <= 1 ? "disabled" : ""} aria-label="Previous page">${icon("chevronLeft", 15)}</button>
      <button class="icon-btn" type="button" data-table-page="next" ${state.tablePage >= totalPages ? "disabled" : ""} aria-label="Next page">${icon("chevronRight", 15)}</button>
      <button class="icon-btn" type="button" data-table-page="last" ${state.tablePage >= totalPages ? "disabled" : ""} aria-label="Last page">${icon("chevronsRight", 15)}</button>
    </div>
  </div>`;
  const pageSize = root.querySelector("#table-page-size");
  if (pageSize) pageSize.value = String(state.tablePageSize);
}

function renderTransactionRows(rows, compact) {
  if (!state.mempoolLoaded) return `<tr><td class="empty-cell" colspan="11">Loading transactions…</td></tr>`;
  if (state.mempoolError && rows.length === 0) return `<tr><td class="empty-cell destructive-text" colspan="11">${escapeHtml(state.mempoolError)}</td></tr>`;
  if (rows.length === 0) return `<tr><td class="empty-cell" colspan="11">No results.</td></tr>`;
  return rows.map((transaction) => {
    const selected = state.selected.has(transaction.txid);
    return `<tr class="${selected ? "selected" : ""}">
      <td><input type="checkbox" data-tx-select="${escapeHtml(transaction.txid)}" ${selected ? "checked" : ""} aria-label="Select transaction" /></td>
      <td title="${escapeHtml(transaction.txid)}"><code>${escapeHtml(shortHash(transaction.txid, 10, 8))}</code></td>
      <td>${formatNumber(transaction.feeRate, 2)}</td><td>${formatNumber(transaction.vsize)}</td><td>${formatNumber(transaction.baseFee)}</td>
      ${compact ? "" : `<td title="${escapeHtml(transaction.depends.join(", "))}">${transaction.depends.length ? escapeHtml(shortHash(transaction.depends[0], 7, 5)) : "—"}</td><td>${formatNumber(transaction.descendant_count)}</td><td>${formatNumber(transaction.descendant_size)}</td><td>${formatNumber(transaction.ancestor_count)}</td><td>${formatNumber(transaction.ancestor_size)}</td><td>${escapeHtml(formatDate(transaction.time))}</td>`}
    </tr>`;
  }).join("");
}

function selectedTransactions() {
  const selected = state.selected;
  return state.mempool.filter((transaction) => selected.has(transaction.txid));
}

function validateSelection() {
  const transactions = selectedTransactions();
  const selectedIds = state.selected;
  const mempoolIds = new Set(state.mempool.map((transaction) => transaction.txid));
  const missing = [...selectedIds].filter((txid) => !mempoolIds.has(txid));
  const dependencyIssues = [];
  for (const transaction of transactions) {
    for (const dependency of transaction.depends) {
      if (mempoolIds.has(dependency) && !selectedIds.has(dependency)) dependencyIssues.push(`${shortHash(transaction.txid)} needs ${shortHash(dependency)}`);
    }
  }
  const totalWeight = transactions.reduce((sum, transaction) => sum + transaction.weight, 0);
  const errors = [];
  if (totalWeight > MAX_BLOCK_WEIGHT) errors.push("Selection exceeds the Bitcoin block weight limit");
  if (missing.length) errors.push(`${missing.length} selected transaction(s) are no longer in the mempool`);
  if (dependencyIssues.length) errors.push(`${dependencyIssues.length} parent transaction dependency issue(s)`);
  return {
    transactions,
    totalWeight,
    totalFees: transactions.reduce((sum, transaction) => sum + transaction.baseFee, 0),
    errors,
    isValid: errors.length === 0,
  };
}

function updateSelectionUI() {
  const validation = validateSelection();
  const percentage = Math.min(100, (validation.totalWeight / MAX_BLOCK_WEIGHT) * 100);
  const copy = document.querySelector("#validation-copy");
  if (!copy) return;
  copy.textContent = state.selected.size
    ? validation.isValid
      ? `${state.selected.size} transaction${state.selected.size === 1 ? "" : "s"} selected`
      : validation.errors.join(" · ")
    : "No transactions selected";
  copy.classList.toggle("destructive-text", !validation.isValid);
  const badge = document.querySelector("#validation-badge");
  badge.textContent = validation.isValid ? "Valid Selection" : "Invalid Selection";
  badge.style.background = validation.isValid ? "var(--primary)" : "var(--destructive)";
  badge.style.borderColor = validation.isValid ? "var(--primary)" : "var(--destructive)";
  document.querySelector("#validation-percent").textContent = `${percentage.toFixed(1)}%`;
  document.querySelector("#validation-percent").classList.toggle("destructive-text", !validation.isValid);
  const progress = document.querySelector("#validation-progress");
  progress.style.width = `${percentage}%`;
  progress.classList.toggle("invalid", !validation.isValid);
  document.querySelector("#validation-weight").textContent = `${formatNumber(validation.totalWeight)} / 4,000,000 weight units`;
  document.querySelector("#validation-remaining").textContent = `${formatNumber(Math.max(0, MAX_BLOCK_WEIGHT - validation.totalWeight))} remaining`;
  const actionBar = document.querySelector("#selection-action-bar");
  actionBar.hidden = state.selected.size === 0;
  if (!actionBar.hidden) {
    actionBar.innerHTML = `<div><strong>${state.selected.size} selected</strong><div class="muted">${formatNumber(validation.totalWeight)} weight units · ${formatNumber(validation.totalFees)} sats</div></div>
      <div class="button-row" style="gap:.5rem"><button class="btn small" type="button" data-action="open-selection-summary">${icon("eye", 15)} Review</button><button class="btn small" type="button" data-action="clear-selection">Clear</button><button class="btn primary small" type="button" data-action="declare-job" ${!validation.isValid || state.declaring ? "disabled" : ""}>${state.declaring ? "Declaring…" : "Declare Job"}</button></div>`;
  }
}

function renderLogs() {
  const root = document.querySelector("#logs-list");
  if (!root) return;
  root.innerHTML = state.logs.length
    ? state.logs.map((log) => `<div class="log-row"><span>${escapeHtml(log.timestamp)}</span><strong class="log-level-${log.level.toLowerCase()}">${escapeHtml(log.level)}</strong><span><strong>${escapeHtml(log.event)}</strong> · ${escapeHtml(log.message)}</span></div>`).join("")
    : '<div class="empty-cell" style="display:grid;place-items:center">No logs available. Waiting for WebSocket connection...</div>';
}

function openFilterModal(key) {
  const definitions = {
    feeRate: ["Minimum Fee Rate", "Show transactions at or above this fee rate (sat/vB).", "number"],
    vsize: ["Maximum Virtual Size", "Show transactions at or below this virtual size.", "number"],
    baseFee: ["Minimum Base Fee", "Show transactions at or above this base fee in satoshis.", "number"],
    depends: ["Depends On", "Show transactions whose parent TXID contains this value.", "text"],
  };
  const [title, description, type] = definitions[key];
  openModal({
    title,
    description,
    size: "",
    body: `<form id="filter-form"><div class="form-field"><label for="filter-value">Value</label><input id="filter-value" class="input" type="${type}" min="0" value="${escapeHtml(state.filters[key] ?? "")}" placeholder="Leave empty to clear" /></div><div class="modal-actions"><button class="btn" type="button" data-action="clear-filter" data-filter-key="${key}">Clear</button><button class="btn primary" type="submit" data-filter-key="${key}">Apply Filter</button></div></form>`,
  });
}

function openSelectionSummary() {
  const validation = validateSelection();
  const count = validation.transactions.length;
  const totalVsize = validation.transactions.reduce((sum, item) => sum + item.vsize, 0);
  const averageFeeRate = count ? validation.transactions.reduce((sum, item) => sum + item.feeRate, 0) / count : 0;
  openModal({
    title: "Selected Transactions",
    description: "Review the selected transactions",
    size: "large",
    body: `<div class="selection-summary-grid">
      ${summaryMetric("Total Transactions", formatNumber(count))}
      ${summaryMetric("Total Fees", `${formatNumber(validation.totalFees)} sats`)}
      ${summaryMetric("Total Virtual Size", `${formatNumber(totalVsize)} vbytes`)}
      ${summaryMetric("Average Fee Rate", `${formatNumber(averageFeeRate, 2)} sat/vB`)}
    </div>
    <div class="form-section"><h4>Transaction Summary</h4><div class="txid-list">${validation.transactions.map((item, index) => `<div class="txid-row"><span class="badge">${index + 1}</span><span class="txid-value" title="${escapeHtml(item.txid)}">${escapeHtml(item.txid)}</span><button class="icon-btn btn ghost" type="button" data-copy="${escapeHtml(item.txid)}" aria-label="Copy TXID">${icon("copy", 14)}</button></div>`).join("")}</div></div>
    <div class="modal-actions"><button class="btn" type="button" data-action="close-modal">Close</button><button class="btn primary" type="button" data-action="declare-job" ${!validation.isValid ? "disabled" : ""}>Declare Job</button></div>`,
  });
}

function summaryMetric(label, value) {
  return `<div class="summary-metric"><div class="summary-metric-label">${label}</div><div class="summary-metric-value">${value}</div></div>`;
}

async function declareJob() {
  if (!state.templateId) {
    toast("No template available", "Please wait for a new template notification before declaring a job.", "error");
    return false;
  }
  const validation = validateSelection();
  if (!state.selected.size) {
    toast("No transactions selected", "Please select at least one transaction to declare a job.", "error");
    return false;
  }
  if (!validation.isValid) {
    toast("Invalid Selection", "Selected transactions violate Bitcoin mining criteria.", "error");
    return false;
  }
  state.declaring = true;
  updateSelectionUI();
  try {
    const data = await envelopeRequest("/api/job-declaration", {
      method: "POST",
      body: JSON.stringify({ template_id: state.templateId, txids: [...state.selected] }),
    });
    toast("Job Declaration Successful!", `Successfully declared job with ${state.selected.size} transactions for template ${state.templateId}.`, "success");
    addLog("JobDeclarationSuccess", "INFO", `Job declared successfully with ${state.selected.size} transactions (template_id: ${state.templateId})`);
    if (state.settings.clear_selection_on_job_declaration) state.selected.clear();
    closeModal();
    return data || true;
  } catch (error) {
    toast("Job Declaration Failed", error.message, "error");
    addLog("JobDeclarationError", "ERROR", `Job declaration failed: ${error.message}`);
    return false;
  } finally {
    state.declaring = false;
    if (state.route === "/dashboard/overview") {
      renderMempoolTable();
      updateSelectionUI();
    }
  }
}

function localAutoSelection() {
  const settings = state.settings;
  let transactions = state.mempool.filter((transaction) => {
    if (transaction.feeRate < settings.min_fee_rate) return false;
    if (settings.max_size && transaction.vsize > settings.max_size) return false;
    if (settings.min_base_fee && transaction.baseFee < settings.min_base_fee) return false;
    if (settings.max_ancestor_count && transaction.ancestor_count > settings.max_ancestor_count) return false;
    if (settings.max_descendant_count && transaction.descendant_count > settings.max_descendant_count) return false;
    if (settings.exclude_bip125_replaceable && transaction.bip125_replaceable) return false;
    if (settings.exclude_unbroadcast && transaction.unbroadcast) return false;
    return true;
  });
  if (settings.selection_strategy === "maximizeCount") transactions.sort((a, b) => a.vsize - b.vsize);
  else if (settings.selection_strategy === "balanced") transactions.sort((a, b) => b.feeRate / Math.max(1, b.vsize) - a.feeRate / Math.max(1, a.vsize));
  else transactions.sort((a, b) => b.baseFee - a.baseFee);
  const maximum = Math.max(1, Number(settings.max_transaction_count) || 100);
  return transactions.slice(0, maximum);
}

async function runAutoSelection(source = "manual") {
  if (state.autoSelecting) {
    toast("Auto-selection in progress", "Please wait for the current auto-selection to complete", "warning");
    return;
  }
  if (!state.settings.auto_selection_enabled) {
    toast("Auto-selection disabled", "Please enable auto-selection in settings first", "warning");
    return;
  }
  if (state.settings.require_template && !state.templateId) {
    toast("No template available", "Please wait for a new template notification before running auto-selection.", "warning");
    return;
  }
  state.autoSelecting = true;
  const button = document.querySelector("#run-auto-button");
  if (button) {
    button.disabled = true;
    button.textContent = "Selecting…";
  }
  let transactions;
  let local = false;
  try {
    const params = new URLSearchParams({
      selectionStrategy: state.settings.selection_strategy,
      minFeeRate: String(state.settings.min_fee_rate),
      maxSize: String(state.settings.max_size),
      minBaseFee: String(state.settings.min_base_fee),
      maxAncestorCount: String(state.settings.max_ancestor_count),
      maxDescendantCount: String(state.settings.max_descendant_count),
      excludeBip125Replaceable: String(state.settings.exclude_bip125_replaceable),
      excludeUnbroadcast: String(state.settings.exclude_unbroadcast),
      maxTransactionCount: String(state.settings.max_transaction_count),
    });
    const data = await envelopeRequest(`/api/auto-select?${params}`);
    if (!Array.isArray(data)) throw new Error("Invalid API response format");
    const selectedIds = new Set(data.map((item) => String(item.txid)));
    transactions = state.mempool.filter((item) => selectedIds.has(item.txid));
  } catch (error) {
    local = true;
    transactions = localAutoSelection();
    toast("Using local filtering", "API unavailable, using local transaction filtering", "warning", 3_000);
  }
  if (!transactions.length) {
    toast("No matching transactions", "No transactions match the current auto-selection criteria", "warning");
  } else {
    if (state.settings.clear_existing_selections || !state.settings.preserve_existing_selections) state.selected.clear();
    transactions.forEach((transaction) => state.selected.add(transaction.txid));
    if (state.settings.pause_on_selection) state.paused = true;
    toast(`Auto-selection ${local ? "(local) " : ""}completed`, `Selected ${transactions.length} transactions${state.templateId ? ` for template ${state.templateId}` : ""}.`, "success");
    if (state.settings.auto_scroll_to_table) document.querySelector(".data-table-container")?.scrollIntoView({ behavior: "smooth" });
    if (state.settings.auto_job_declaration && state.templateId) await declareJob();
  }
  state.autoSelecting = false;
  if (state.route === "/dashboard/overview") renderOverview();
  if (source === "template") addLog("AutoSelection", "INFO", `Auto-selection completed for template ${state.templateId ?? "unknown"}`);
}

function openAutoSelectionSettings() {
  const settings = state.settings;
  openModal({
    title: "Auto-Selection Settings",
    description: "Configure auto-selection criteria for transaction selection when new templates arrive.",
    size: "large",
    body: `<form id="auto-settings-form">
      <div class="setting-row"><div class="setting-copy"><strong>Enable Auto-Selection</strong><span>Automatically select transactions when new templates arrive</span></div>${switchControl("auto_selection_enabled", settings.auto_selection_enabled, "Enable auto-selection")}</div>
      <div class="form-section"><div class="form-grid">
        ${formSelect("selection_strategy", "Selection Strategy", settings.selection_strategy, [["maximizeFees", "Maximize Fees"], ["maximizeCount", "Maximize Count"], ["balanced", "Balanced"]])}
        ${formInput("min_fee_rate", "Min Fee Rate (sat/vB)", settings.min_fee_rate, 0, "number")}
        ${formInput("max_size", "Max Size (vBytes)", settings.max_size, 1, "number")}
        ${formInput("min_base_fee", "Min Base Fee (sats)", settings.min_base_fee, 0, "number")}
        ${formInput("max_transaction_count", "Max Transaction Count", settings.max_transaction_count, 1, "number")}
        ${formInput("max_ancestor_count", "Max Ancestor Count", settings.max_ancestor_count, 1, "number")}
        ${formInput("max_descendant_count", "Max Descendant Count", settings.max_descendant_count, 1, "number")}
      </div></div>
      <div class="form-section"><h4>Selection Preferences</h4>
        ${modalSettingRow("require_template", "Require Template", "Only run auto-selection when a template is available", settings.require_template)}
        ${modalSettingRow("clear_existing_selections", "Clear Existing Selections", "Remove current selections before auto-selecting new ones", settings.clear_existing_selections)}
        ${modalSettingRow("periodic_enabled", "Enable Periodic Auto-Selection", "Run auto-selection at regular intervals", settings.periodic_enabled)}
        <div id="periodic-interval-wrap" class="form-field" style="margin:.6rem 0;${settings.periodic_enabled ? "" : "display:none"}"><label>Interval (seconds)</label><input class="input" name="periodic_interval" type="number" min="5" max="300" value="${settings.periodic_interval}" /></div>
        ${modalSettingRow("auto_job_declaration", "Auto Job Declaration", "Automatically declare job after successful auto-selection", settings.auto_job_declaration)}
      </div>
      <div class="form-section"><h4>Exclusion Preferences</h4>
        ${modalSettingRow("exclude_bip125_replaceable", "Exclude BIP125 Replaceable", "Exclude replaceable transactions", settings.exclude_bip125_replaceable)}
        ${modalSettingRow("exclude_unbroadcast", "Exclude Unbroadcast", "Exclude transactions not broadcast to peers", settings.exclude_unbroadcast)}
      </div>
      <div class="modal-actions"><button class="btn" type="button" data-action="close-modal">Cancel</button><button class="btn primary" type="submit">Save Settings</button></div>
    </form>`,
  });
}

function formInput(name, label, value, min = 0, type = "text") {
  return `<div class="form-field"><label for="${name}">${label}</label><input id="${name}" class="input" name="${name}" type="${type}" min="${min}" value="${escapeHtml(value)}" /></div>`;
}

function formSelect(name, label, value, options) {
  return `<div class="form-field"><label for="${name}">${label}</label><select id="${name}" class="select-control" name="${name}">${options.map(([optionValue, optionLabel]) => `<option value="${optionValue}" ${optionValue === value ? "selected" : ""}>${optionLabel}</option>`).join("")}</select></div>`;
}

function modalSettingRow(name, label, description, checked) {
  return `<div class="setting-row"><div class="setting-copy"><strong>${label}</strong><span>${description}</span></div>${switchControl(name, checked, label)}</div>`;
}

function formSettings(form) {
  const data = new FormData(form);
  const next = { ...state.settings };
  const booleans = [
    "auto_selection_enabled", "require_template", "clear_existing_selections", "periodic_enabled",
    "auto_job_declaration", "exclude_bip125_replaceable", "exclude_unbroadcast",
  ];
  booleans.forEach((key) => { next[key] = data.has(key); });
  ["min_fee_rate", "max_size", "min_base_fee", "max_transaction_count", "max_ancestor_count", "max_descendant_count", "periodic_interval"].forEach((key) => {
    if (data.has(key)) next[key] = Number(data.get(key));
  });
  if (data.has("selection_strategy")) next.selection_strategy = String(data.get("selection_strategy"));
  return next;
}

async function saveSettings(settings, successMessage = "Settings saved") {
  state.settings = { ...state.settings, ...settings };
  try {
    const saved = await envelopeRequest("/api/settings", { method: "POST", body: JSON.stringify(state.settings) });
    if (saved) state.settings = { ...state.settings, ...saved };
    toast(successMessage, "Your dashboard preferences have been updated.", "success");
  } catch (error) {
    localStorage.setItem("demand-settings", JSON.stringify(state.settings));
    toast("Saved locally", `The settings API is unavailable: ${error.message}`, "warning");
  }
  schedulePeriodicAutoSelection();
}

function schedulePeriodicAutoSelection() {
  clearInterval(state.periodicTimer);
  state.periodicTimer = null;
  if (state.settings.auto_selection_enabled && state.settings.periodic_enabled) {
    state.periodicTimer = setInterval(() => runAutoSelection("periodic"), Math.max(5, state.settings.periodic_interval) * 1000);
  }
}

async function loadSettings() {
  const local = localStorage.getItem("demand-settings");
  if (local) {
    try { state.settings = { ...state.settings, ...JSON.parse(local) }; } catch (_) { /* ignore invalid local backup */ }
  }
  try {
    const settings = await envelopeRequest("/api/settings");
    if (settings) state.settings = { ...state.settings, ...settings };
  } catch (_) {
    // The dashboard remains usable with defaults when Bitcoin RPC/database is unavailable.
  }
  state.settingsLoaded = true;
  schedulePeriodicAutoSelection();
  if (state.route === "/dashboard/settings") renderSettings();
}

async function openDetailedStats() {
  await loadMiners();
  openModal({
    title: "Mining Pool Statistics",
    description: "Detailed view of mining pool performance and system metrics",
    size: "large",
    body: `<div class="tabs" role="tablist">
      ${["overview", "miners", "system", "pool"].map((tab) => `<button class="tab ${tab === "overview" ? "active" : ""}" type="button" data-stats-tab="${tab}">${tab[0].toUpperCase() + tab.slice(1)}${tab === "pool" ? " Info" : ""}</button>`).join("")}
    </div><div id="stats-tab-panel" class="tab-panel">${statsTabContent("overview")}</div>`,
  });
}

function statsTabContent(tab) {
  if (tab === "miners") {
    const miners = state.stats.miners ? Object.entries(state.stats.miners) : [];
    if (!miners.length) return '<div class="detail-card muted">No miner data available</div>';
    return miners.map(([id, miner]) => `<div class="detail-card" style="margin-bottom:.75rem"><h4>${icon("monitor", 16)} ${escapeHtml(miner.device_name || `Miner ${id}`)}</h4><div class="stats-details-grid">${detail("Hashrate", formatHashrate(miner.hashrate))}${detail("Difficulty", formatNumber(miner.current_difficulty, 4))}${detail("Accepted Shares", formatNumber(miner.accepted_shares))}${detail("Rejected Shares", formatNumber(miner.rejected_shares))}</div></div>`).join("");
  }
  if (tab === "system") {
    const system = state.stats.system;
    if (!system) return '<div class="detail-card muted">No system data available</div>';
    const cpu = Number(system["cpu_usage_%"] ?? system.cpu_usage);
    return `<div class="detail-card"><h4>${icon("cpu", 16)} System Performance</h4><div class="stats-details-grid">${detail("CPU Usage", Number.isFinite(cpu) ? `${cpu.toFixed(1)}%` : "N/A")}${detail("Memory Usage", formatBytes(system.memory_usage_bytes ?? system.memory_usage))}</div></div>`;
  }
  if (tab === "pool") {
    const pool = state.stats.pool;
    return pool ? `<div class="detail-card"><h4>${icon("globe", 16)} Pool Information</h4><div class="stats-details-grid">${detail("Pool Address", pool.address || "N/A")}${detail("Latency", `${pool.latency ?? "N/A"} ms`)}</div></div>` : '<div class="detail-card muted">No pool data available</div>';
  }
  const aggregate = state.stats.aggregate;
  return `<div class="detail-card"><h4>${icon("activity", 16)} Aggregate Statistics</h4><div class="stats-details-grid">${detail("Connected Devices", aggregate?.total_connected_device ?? "N/A")}${detail("Total Hashrate", aggregate ? formatHashrate(aggregate.aggregate_hashrate) : "N/A")}${detail("Accepted Shares", formatNumber(aggregate?.aggregate_accepted_shares))}${detail("Rejected Shares", formatNumber(aggregate?.aggregate_rejected_shares))}${detail("Current Difficulty", formatNumber(aggregate?.aggregate_diff, 4))}</div></div>`;
}

function detail(label, value) {
  return `<div><div class="detail-label">${label}</div><div class="detail-value">${escapeHtml(value)}</div></div>`;
}

function renderJobHistory() {
  const page = currentPageElement();
  page.className = "page compact-top";
  page.innerHTML = `<section class="page-header"><div><h1 class="page-title">Job Declaration History</h1><p class="page-description">View your submitted job declarations</p></div></section><div id="job-history-content"></div>`;
  renderJobHistoryContent();
  if (!state.jobsLoading && !state.jobs.length && !state.jobsError) loadJobHistory();
}

async function loadJobHistory() {
  state.jobsLoading = true;
  state.jobsError = null;
  renderJobHistoryContent();
  try {
    const data = await envelopeRequest(`/api/job-history?page=${state.jobPage}&per_page=${state.jobPerPage}`);
    state.jobs = data?.jobs || [];
    state.jobTotal = Number(data?.total) || 0;
    state.jobTotalPages = Number(data?.total_pages) || 0;
  } catch (error) {
    state.jobsError = error.message;
  } finally {
    state.jobsLoading = false;
    renderJobHistoryContent();
  }
}

function renderJobHistoryContent() {
  const root = document.querySelector("#job-history-content");
  if (!root) return;
  if (state.jobsError) {
    root.innerHTML = `<section class="card history-error"><div>Error loading job history: ${escapeHtml(state.jobsError)}</div><button class="btn" type="button" data-action="refresh-history">${icon("refresh", 16)} Try Again</button></section>`;
    return;
  }
  const totalPages = Math.max(1, state.jobTotalPages || 1);
  const start = state.jobTotal ? (state.jobPage - 1) * state.jobPerPage + 1 : 0;
  const end = Math.min(state.jobPage * state.jobPerPage, state.jobTotal);
  root.innerHTML = `<section class="card history-card">
    <div class="history-toolbar"><div class="button-row" style="gap:.5rem"><button class="btn small" type="button" data-action="refresh-history" ${state.jobsLoading ? "disabled" : ""}>${icon("refresh", 15)} Refresh</button></div><span class="muted">${state.jobsLoading ? "Loading…" : `${state.jobTotal} total job${state.jobTotal === 1 ? "" : "s"}`}</span></div>
    <div class="table-shell"><table class="data-table history-table"><thead><tr><th>JD No</th><th>Template ID</th><th>Channel</th><th>TX Count</th><th>Mining Job Token Hex</th><th>Created</th><th>Actions</th></tr></thead><tbody>${renderJobRows()}</tbody></table></div>
    <div class="history-pagination"><span>Showing ${start} to ${end} of ${state.jobTotal} entries</span><div class="pagination-controls"><label class="nowrap">Rows per page <select id="job-page-size" class="select-control"><option>10</option><option>20</option><option>30</option><option>50</option></select></label><span>Page ${state.jobPage} of ${totalPages}</span><button class="icon-btn" data-job-page="first" ${state.jobPage <= 1 ? "disabled" : ""}>${icon("chevronsLeft", 15)}</button><button class="icon-btn" data-job-page="previous" ${state.jobPage <= 1 ? "disabled" : ""}>${icon("chevronLeft", 15)}</button><button class="icon-btn" data-job-page="next" ${state.jobPage >= totalPages ? "disabled" : ""}>${icon("chevronRight", 15)}</button><button class="icon-btn" data-job-page="last" ${state.jobPage >= totalPages ? "disabled" : ""}>${icon("chevronsRight", 15)}</button></div></div>
  </section>`;
  const pageSize = root.querySelector("#job-page-size");
  if (pageSize) pageSize.value = String(state.jobPerPage);
}

function renderJobRows() {
  if (state.jobsLoading && !state.jobs.length) return '<tr><td colspan="7" class="empty-cell">Loading…</td></tr>';
  if (!state.jobs.length) return '<tr><td colspan="7" class="empty-cell">No results.</td></tr>';
  return state.jobs.map((job) => `<tr><td><strong>#${formatNumber(job.id)}</strong></td><td><code>${escapeHtml(job.template_id)}</code></td><td><span class="badge">CH-${escapeHtml(job.channel_id)}</span></td><td>${formatNumber(job.txid_count)}</td><td><span class="inline" style="gap:.35rem"><code>${escapeHtml(shortHash(job.mining_job_token))}</code><button class="icon-btn btn ghost" type="button" data-copy="${escapeHtml(job.mining_job_token)}" aria-label="Copy mining job token">${icon("copy", 13)}</button></span></td><td class="muted">${escapeHtml(formatDate(job.created_at))}</td><td><button class="btn ghost small" type="button" data-view-txids="${escapeHtml(job.template_id)}">${icon("eye", 14)} View TXIDs</button></td></tr>`).join("");
}

async function openJobTxids(templateId) {
  openModal({ title: `Job TXIDs - Template ${templateId}`, description: "Transaction IDs included in this job declaration", size: "xlarge", body: '<div class="empty-cell" style="display:grid;place-items:center">Loading transaction IDs...</div>' });
  try {
    const data = await envelopeRequest(`/api/job-txids/${encodeURIComponent(templateId)}`);
    const txids = data?.txids || [];
    openModal({
      title: `Job TXIDs - Template ${templateId}`,
      description: "Transaction IDs included in this job declaration",
      size: "xlarge",
      body: `<div class="detail-card"><h4>${icon("hash", 16)} Transaction Summary</h4><div class="details-grid">${detail("Template ID", data?.template_id ?? templateId)}${detail("Total TXIDs", data?.total ?? txids.length)}</div></div>
      <div class="validation-head" style="margin:1rem 0"><span class="badge">${txids.length} Transaction${txids.length === 1 ? "" : "s"}</span><div class="button-row" style="gap:.5rem"><button class="btn small" type="button" data-copy="${escapeHtml(txids.join("\n"))}">${icon("copy", 14)} Copy All</button><button class="btn small" type="button" data-export-txids="${escapeHtml(templateId)}">${icon("download", 14)} Export CSV</button></div></div>
      <h4>Transaction IDs</h4><div class="txid-list">${txids.map((txid, index) => `<div class="txid-row"><span class="badge">${index + 1}</span><span class="txid-value" title="${escapeHtml(txid)}">${escapeHtml(txid)}</span><div class="button-row"><button class="icon-btn btn ghost" data-copy="${escapeHtml(txid)}" aria-label="Copy transaction ID">${icon("copy", 14)}</button><a class="icon-btn btn ghost" href="https://mempool.space/tx/${encodeURIComponent(txid)}" target="_blank" rel="noopener noreferrer" aria-label="View on mempool.space">${icon("external", 14)}</a></div></div>`).join("")}</div>`,
    });
    document.querySelector("[data-export-txids]")?.addEventListener("click", () => downloadText(["txid", ...txids].join("\n"), `job-txids-${templateId}.csv`, "text/csv;charset=utf-8"), { once: true });
  } catch (error) {
    openModal({ title: `Job TXIDs - Template ${templateId}`, description: "Transaction IDs included in this job declaration", body: `<div class="history-error">Error: ${escapeHtml(error.message)}</div>` });
  }
}

function renderSettings() {
  const page = currentPageElement();
  const settings = state.settings;
  page.className = "page compact-top";
  page.innerHTML = `<section class="page-header"><div><h1 class="page-title-row">${icon("settings", 32)} Dashboard Settings</h1><p class="page-description">Configure general dashboard preferences and settings</p></div><button class="btn" type="button" data-action="reset-settings">${icon("undo", 16)} Reset All Settings</button></section>
  <section class="card settings-shell"><div class="settings-section-header"><h2 class="settings-section-title">${icon("user", 18)} General Preferences</h2><p class="settings-section-description">Customize dashboard behavior, notifications, and user interface preferences.</p></div>
    <form id="general-settings-form">
      <section class="card settings-group"><h3>UI Behavior</h3><p>Control how the dashboard behaves during transaction selection and job declarations</p>
        ${settingsRow("auto_scroll_to_table", "Auto-scroll to table", "Automatically scroll to the transaction table when prompted", settings.auto_scroll_to_table)}
        ${settingsRow("pause_on_selection", "Pause on selection", "Pause mempool updates when auto-selecting transactions", settings.pause_on_selection)}
        ${settingsRow("clear_selection_on_job_declaration", "Clear selection on job declaration", "Automatically clear selected transactions after successful job declaration", settings.clear_selection_on_job_declaration)}
      </section>
      <section class="card settings-group"><h3>Notifications</h3><p>Configure when and how notifications are displayed</p>
        ${settingsRow("show_notifications", "Show notifications", "Display toast notifications for events and status updates", settings.show_notifications)}
      </section>
      <section class="card settings-group"><h3>Settings Backup & Restore</h3><p>Export your settings for backup or import previously saved settings</p><div class="backup-actions"><button class="btn" type="button" data-action="export-settings">${icon("download", 16)} Export Settings</button><button class="btn" type="button" data-action="import-settings">${icon("upload", 16)} Import Settings</button><input id="settings-import" type="file" accept="application/json" hidden /></div></section>
      <div class="settings-footer"><button class="btn primary" type="submit">Save Settings</button></div>
    </form>
  </section>`;
}

function settingsRow(name, title, description, checked) {
  return `<div class="setting-row"><div class="setting-copy"><strong>${title}</strong><span>${description}</span></div>${switchControl(name, checked, title)}</div>`;
}

function collectGeneralSettings(form) {
  const data = new FormData(form);
  return {
    auto_scroll_to_table: data.has("auto_scroll_to_table"),
    pause_on_selection: data.has("pause_on_selection"),
    clear_selection_on_job_declaration: data.has("clear_selection_on_job_declaration"),
    show_notifications: data.has("show_notifications"),
  };
}

function changeTablePage(action) {
  const totalPages = Math.max(1, Math.ceil(filteredTransactions().length / state.tablePageSize));
  if (action === "first") state.tablePage = 1;
  else if (action === "previous") state.tablePage = Math.max(1, state.tablePage - 1);
  else if (action === "next") state.tablePage = Math.min(totalPages, state.tablePage + 1);
  else if (action === "last") state.tablePage = totalPages;
  renderMempoolTable();
}

function changeJobPage(action) {
  const totalPages = Math.max(1, state.jobTotalPages || 1);
  if (action === "first") state.jobPage = 1;
  else if (action === "previous") state.jobPage = Math.max(1, state.jobPage - 1);
  else if (action === "next") state.jobPage = Math.min(totalPages, state.jobPage + 1);
  else if (action === "last") state.jobPage = totalPages;
  loadJobHistory();
}

document.addEventListener("click", async (event) => {
  const nav = event.target.closest("[data-nav]");
  if (nav) {
    event.preventDefault();
    navigate(nav.dataset.nav);
    return;
  }
  const actionTarget = event.target.closest("[data-action]");
  if (actionTarget) {
    const action = actionTarget.dataset.action;
    if (action === "toggle-sidebar") {
      const sidebar = document.querySelector("#sidebar");
      if (window.innerWidth < 768) sidebar.classList.toggle("mobile-open");
      else sidebar.classList.toggle("expanded");
    } else if (action === "toggle-mode") {
      state.mode = state.mode === "dark" ? "light" : "dark";
      localStorage.setItem("demand-mode", state.mode);
      applyAppearance();
    } else if (action === "close-modal" && (!event.target.closest("[data-modal-panel]") || actionTarget.closest("button"))) {
      closeModal();
    } else if (action === "open-stats") await openDetailedStats();
    else if (action === "toggle-pause") { state.paused = !state.paused; renderOverview(); }
    else if (action === "run-auto") await runAutoSelection();
    else if (action === "open-auto-settings") openAutoSelectionSettings();
    else if (action === "open-selection-summary") openSelectionSummary();
    else if (action === "clear-selection") { state.selected.clear(); renderMempoolTable(); updateSelectionUI(); closeModal(); }
    else if (action === "declare-job") await declareJob();
    else if (action === "refresh-history") loadJobHistory();
    else if (action === "reset-settings") {
      state.settings = { ...DEFAULT_SETTINGS };
      await saveSettings(state.settings, "Settings reset");
      renderSettings();
    } else if (action === "export-settings") {
      downloadText(JSON.stringify(state.settings, null, 2), `demand-dashboard-settings-${new Date().toISOString().slice(0, 10)}.json`, "application/json");
    } else if (action === "import-settings") document.querySelector("#settings-import")?.click();
    else if (action === "clear-filter") {
      state.filters[actionTarget.dataset.filterKey] = actionTarget.dataset.filterKey === "depends" ? "" : null;
      closeModal();
      renderOverview();
    }
  }
  const filter = event.target.closest("[data-filter]");
  if (filter) openFilterModal(filter.dataset.filter);
  const tablePage = event.target.closest("[data-table-page]");
  if (tablePage) changeTablePage(tablePage.dataset.tablePage);
  const jobPage = event.target.closest("[data-job-page]");
  if (jobPage) changeJobPage(jobPage.dataset.jobPage);
  const viewTxids = event.target.closest("[data-view-txids]");
  if (viewTxids) openJobTxids(viewTxids.dataset.viewTxids);
  const copy = event.target.closest("[data-copy]");
  if (copy) {
    try { await copyText(copy.dataset.copy); toast("Copied to clipboard", "", "success", 2_000); }
    catch (error) { toast("Copy failed", error.message, "error"); }
  }
  const statsTab = event.target.closest("[data-stats-tab]");
  if (statsTab) {
    document.querySelectorAll("[data-stats-tab]").forEach((tab) => tab.classList.toggle("active", tab === statsTab));
    document.querySelector("#stats-tab-panel").innerHTML = statsTabContent(statsTab.dataset.statsTab);
  }
});

document.addEventListener("change", async (event) => {
  const target = event.target;
  if (target.id === "theme-select") {
    state.theme = target.value;
    localStorage.setItem("demand-theme", state.theme);
    applyAppearance();
  } else if (target.id === "view-control") {
    state.tableView = target.value;
    renderMempoolTable();
  } else if (target.id === "sort-control") {
    [state.sort.key, state.sort.direction] = target.value.split(":");
    renderMempoolTable();
  } else if (target.id === "table-page-size") {
    state.tablePageSize = Number(target.value);
    state.tablePage = 1;
    renderMempoolTable();
  } else if (target.id === "job-page-size") {
    state.jobPerPage = Number(target.value);
    state.jobPage = 1;
    loadJobHistory();
  } else if (target.matches("[data-tx-select]")) {
    if (target.checked) state.selected.add(target.dataset.txSelect);
    else state.selected.delete(target.dataset.txSelect);
    renderMempoolTable();
    updateSelectionUI();
  } else if (target.matches('[data-action="select-page"]')) {
    const rows = filteredTransactions().slice((state.tablePage - 1) * state.tablePageSize, state.tablePage * state.tablePageSize);
    rows.forEach((row) => target.checked ? state.selected.add(row.txid) : state.selected.delete(row.txid));
    renderMempoolTable();
    updateSelectionUI();
  } else if (target.name === "periodic_enabled") {
    const wrapper = document.querySelector("#periodic-interval-wrap");
    if (wrapper) wrapper.style.display = target.checked ? "flex" : "none";
  } else if (target.id === "settings-import" && target.files?.[0]) {
    try {
      const imported = JSON.parse(await target.files[0].text());
      const allowed = Object.keys(DEFAULT_SETTINGS);
      const sanitized = Object.fromEntries(Object.entries(imported).filter(([key]) => allowed.includes(key)));
      await saveSettings({ ...state.settings, ...sanitized }, "Settings imported");
      renderSettings();
    } catch (error) {
      toast("Import failed", "The selected file is not valid dashboard settings JSON.", "error");
    }
  }
});

document.addEventListener("input", (event) => {
  if (event.target.id === "tx-search") {
    state.search = event.target.value;
    state.tablePage = 1;
    renderMempoolTable();
  }
});

document.addEventListener("submit", async (event) => {
  if (event.target.id === "filter-form") {
    event.preventDefault();
    const key = event.submitter?.dataset.filterKey;
    const value = event.target.querySelector("#filter-value").value.trim();
    state.filters[key] = key === "depends" ? value : value === "" ? null : Number(value);
    state.tablePage = 1;
    closeModal();
    renderOverview();
  } else if (event.target.id === "auto-settings-form") {
    event.preventDefault();
    await saveSettings(formSettings(event.target), "Auto-selection settings saved");
    closeModal();
    if (state.route === "/dashboard/overview") renderOverview();
  } else if (event.target.id === "general-settings-form") {
    event.preventDefault();
    await saveSettings(collectGeneralSettings(event.target));
    renderSettings();
  }
});

window.addEventListener("popstate", () => {
  state.route = normalizeRoute(window.location.pathname);
  updateNavigation();
  renderRoute();
});

window.addEventListener("keydown", (event) => {
  if (event.key === "Escape") closeModal();
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "b") {
    event.preventDefault();
    document.querySelector('[data-action="toggle-sidebar"]')?.click();
  }
});

function initialize() {
  applyAppearance();
  if (window.location.pathname !== state.route) window.history.replaceState({}, "", state.route);
  renderShell();
  loadSettings();
  pollStats();
  loadMempool();
  connectWebSockets();
  setInterval(pollStats, API_POLL_INTERVAL);
  setInterval(() => loadMempool(), MEMPOOL_REFRESH_INTERVAL);
}

initialize();
