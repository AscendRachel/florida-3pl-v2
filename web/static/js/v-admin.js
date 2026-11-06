// Admin (admins only): a month table with one row per day. The three columns are filled in once their conditions are decided.
import { api, brand, esc, fmt, toast } from "./util.js";

const MONTHS = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const DAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const COLUMNS = ["Package Received", "Shipped to Customer", "Pending"];

export async function mount(el) {
  const now = new Date();
  let year = now.getFullYear(), month = now.getMonth(); // month: 0-11
  const today = `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}-${String(now.getDate()).padStart(2, "0")}`;

  let cards = [], panel = "", labels = null; // panel: the card whose list is open under the cards
  let received = {}, shipped = {}; // day (YYYY-MM-DD) -> packages received / shipped to the customer

  // Pending = packages received - shipped to the customer (of that same day's packages)
  const sum = (o) => Object.values(o).reduce((a, b) => a + b, 0);
  const pending = (r = 0, s = 0) => (r || s ? fmt(r - s) : "–");

  const draw = () => {
    const count = new Date(year, month + 1, 0).getDate();
    const rows = [];
    for (let d = 1; d <= count; d++) {
      const iso = `${year}-${String(month + 1).padStart(2, "0")}-${String(d).padStart(2, "0")}`;
      const dow = new Date(year, month, d).getDay();
      rows.push(`<tr class="${iso === today ? "nowrow" : ""}${dow === 0 || dow === 6 ? " adm-weekend" : ""}"><td>${MONTHS[month].slice(0, 3)} ${d}, ${year} <span class="muted">${DAYS[dow]}</span></td>
        <td class="num">${received[iso] ? fmt(received[iso]) : "–"}</td><td class="num">${shipped[iso] ? fmt(shipped[iso]) : "–"}</td><td class="num">${pending(received[iso], shipped[iso])}</td></tr>`);
    }
    el.innerHTML = `<div class="section-head"><div><div class="eyebrow">${esc(brand.name)}</div><h2>Admin</h2>
        <p>Things to fix in All Orders, and the month's packages day by day. Updates live.</p></div>
        <div class="btnrow dash-date"><button class="btn" id="prev" title="Month before">‹</button>
          <label class="field" style="margin:0"><span>Select month</span><input type="month" id="month" value="${year}-${String(month + 1).padStart(2, "0")}"></label>
          <button class="btn" id="next" title="Next month">›</button>
          ${year === now.getFullYear() && month === now.getMonth() ? "" : '<button class="btn" id="thisMonth">This month</button>'}</div></div>
      <div class="dash-cards">${cards.map((c) => `<a class="scard warn dash-card${c.panel && panel === c.panel ? " on" : ""}" ${c.panel ? `href="#admin" data-panel="${esc(c.panel)}" title="Show the list"` : `href="${esc(c.link)}" title="Show these orders in All Orders"`} style="text-decoration:none;color:inherit"><div class="dash-n">${fmt(c.n)}</div>
        <div class="dash-l">${esc(c.label)}<div class="muted">${esc(c.note)}</div></div></a>`).join("")}</div>
      ${panel === "labels" ? labelsPanel() : ""}
      <div class="panel"><h3 style="margin:0 0 10px">${MONTHS[month]} ${year}</h3>
        <div class="tablewrap" style="max-height:none"><table class="adm-table"><thead><tr><th>Date</th>${COLUMNS.map((c) => `<th class="num">${esc(c)}</th>`).join("")}</tr></thead>
          <tbody>${rows.join("")}</tbody>
          <tfoot><tr><td><b>Total</b></td><td class="num"><b>${fmt(sum(received))}</b></td><td class="num"><b>${fmt(sum(shipped))}</b></td><td class="num"><b>${pending(sum(received), sum(shipped))}</b></td></tr></tfoot></table></div></div>`;
  };

  // the label links whose file name does not fit the order: Label Number / Tracking Number + Customer Name
  const labelsPanel = () => {
    if (!labels) return `<div class="panel"><span class="muted">Loading…</span></div>`;
    const rows = labels.mismatches.slice(0, 500);
    return `<div class="panel"><div class="btnrow" style="justify-content:space-between;align-items:center"><b>Label links that do not fit the order</b><button class="btn small" id="closePanel">Close</button></div>
      <p class="muted" style="margin:6px 0 10px">A label file is named Customer name + tracking number, or label number + Customer name. These files have neither the right customer nor the right number. Checked ${fmt(labels.checked)} of ${fmt(labels.total)} links so far${labels.unread ? `; ${fmt(labels.unread)} could not be read (private or deleted)` : ""}. Google is read a few files at a time in the background, so this list grows until every link is checked.</p>
      ${rows.length ? `<div class="tablewrap" style="max-height:420px"><table><thead><tr><th>Label</th><th>Customer</th><th>Tracking to customer</th><th>File name</th><th>Problem</th></tr></thead><tbody>
        ${rows.map((m) => `<tr><td><a href="#orders?${new URLSearchParams({ q: m.label, row: String(m.rn), field: "Shipping Label Link", t: String(Date.now()) })}">${esc(m.label)}</a></td><td>${esc(m.customer)}</td><td class="mono">${esc(m.tracking)}</td><td class="clip" title="${esc(m.file)}">${esc(m.file)}</td><td>${esc(m.why)}</td></tr>`).join("")}</tbody></table></div>${labels.mismatches.length > rows.length ? `<p class="hint">Showing the first ${rows.length} of ${fmt(labels.mismatches.length)}.</p>` : ""}` : `<p>No mismatches among the links checked so far.</p>`}</div>`;
  };
  async function loadLabels() { try { labels = await api("/api/admin/labels"); draw(); } catch (e) { toast(e.message, "bad"); } }

  let seq = 0;
  async function load() {
    const my = ++seq, m = `${year}-${String(month + 1).padStart(2, "0")}`;
    received = {}; shipped = {};
    draw();
    try {
      const [r, c] = await Promise.all([api("/api/admin/month?month=" + m), api("/api/admin/cards")]);
      if (my !== seq) return;
      received = r.received; shipped = r.shipped; cards = c.cards;
      draw();
    } catch (e) { toast(e.message, "bad"); }
  }
  const go = (n) => { const d = new Date(year, month + n, 1); year = d.getFullYear(); month = d.getMonth(); load(); };
  el.onchange = (e) => {
    if (e.target.id !== "month" || !/^\d{4}-\d\d$/.test(e.target.value)) return;
    year = +e.target.value.slice(0, 4); month = +e.target.value.slice(5, 7) - 1; load();
  };
  el.onclick = (e) => {
    const card = e.target.closest("[data-panel]");
    if (card) { e.preventDefault(); panel = panel === card.dataset.panel ? "" : card.dataset.panel; labels = null; draw(); if (panel === "labels") loadLabels(); return; }
    const b = e.target.closest("button");
    if (!b) return;
    if (b.id === "closePanel") { panel = ""; draw(); return; }
    if (b.id === "prev") go(-1);
    else if (b.id === "next") go(1);
    else if (b.id === "thisMonth") { year = now.getFullYear(); month = now.getMonth(); load(); }
  };
  load();
  // live: when anything in the app changed, look again
  let known = (await api("/api/version")).v;
  let ticks = 0;
  const timer = setInterval(async () => {
    if (!el.isConnected || location.hash.split("?")[0] !== "#admin") return clearInterval(timer);
    if (document.hidden) return;
    try {
      const { v } = await api("/api/version");
      const slow = ++ticks % 5 === 0 && cards.some((c) => c.panel); // the label check moves on without any change in the app
      if (v === known && !slow) return;
      known = v;
      const [r, c] = await Promise.all([api("/api/admin/month?month=" + `${year}-${String(month + 1).padStart(2, "0")}`), api("/api/admin/cards")]);
      received = r.received; shipped = r.shipped; cards = c.cards;
      if (panel === "labels") labels = await api("/api/admin/labels");
      draw();
    } catch { /* offline for a moment */ }
  }, 3000);
}
