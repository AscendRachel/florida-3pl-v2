// Admin (admins only): the things to fix in All Orders, and the 3PL Daily Update for the chosen day.
import { api, brand, esc, fmt, toast } from "./util.js";

const MONTHS = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

// "2026-11-04" -> "November 4"
const longDate = (iso) => { const m = /^(\d{4})-(\d\d)-(\d\d)$/.exec(iso || ""); return m ? `${MONTHS[+m[2] - 1]} ${+m[3]}` : "No delivery date"; };
const isoOf = (d) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;

export async function mount(el) {
  const now = new Date(), today = isoOf(now);
  let day = today; // the day of the Daily Update

  let cards = [], panel = "", labels = null; // panel: the card whose list is open under the cards
  let showApproved = false; // the "approved" part of the label list is open
  let lastSig = "";         // what was last drawn; the page is only redrawn when something in it changed
  let showOlder = false;    // the older pending packages are listed day by day
  let rep = null;          // the Daily Update of `day`: { arrived, shipped: [{date, n}], pending: [{date, n}] }

  // what the page shows, as one text: if it is the same as the last time, nothing needs to be redrawn
  const signature = () => JSON.stringify([day, rep, cards, panel, showApproved, showOlder, labels && [labels.mismatches, labels.approved, labels.checked, labels.unread, labels.total, labels.reading]]);

  // Pending is listed day by day for the last 2 weeks of Delivery Dates; the older ones are one line (they can be opened)
  const KEEP_DAYS = 14;
  const split = () => {
    const [y, m, d] = day.split("-").map(Number), cutoff = isoOf(new Date(y, m - 1, d - KEEP_DAYS));
    const recent = rep.pending.filter((r) => r.date >= cutoff), older = rep.pending.filter((r) => r.date < cutoff);
    return { cutoff, recent, older, olderN: older.reduce((a, r) => a + r.n, 0) };
  };
  // the report as plain text, ready to paste into a message
  const reportText = () => {
    if (!rep) return "";
    const p = split();
    const lines = ["3PL Daily Update", longDate(day), `Total Deliveries Arrived: ${rep.arrived}`];
    if (rep.shipped.length) rep.shipped.forEach((r) => lines.push(`Orders Shipped: ${r.n} (${longDate(r.date)})`)); else lines.push("Orders Shipped: 0");
    p.recent.forEach((r) => lines.push(`Pending: ${r.n} (${longDate(r.date)})`));
    if (p.olderN) lines.push(`Pending: ${p.olderN} (before ${longDate(p.cutoff)})`);
    if (!p.recent.length && !p.olderN) lines.push("Pending: 0");
    return lines.join("\n");
  };
  const reportPanel = () => {
    if (!rep) return `<div class="panel"><span class="muted">Loading…</span></div>`;
    const p = split();
    const row = (label, n, date) => `<tr><td>${label}</td><td class="num"><b>${fmt(n)}</b></td><td>${date ? `(${esc(longDate(date))})` : ""}</td></tr>`;
    const shipped = rep.shipped.length ? rep.shipped.map((r) => row("Orders Shipped", r.n, r.date)).join("") : row("Orders Shipped", 0, "");
    const pend = p.recent.map((r) => row("Pending", r.n, r.date)).join("")
      + (p.olderN ? `<tr><td>Pending</td><td class="num"><b>${fmt(p.olderN)}</b></td><td>(before ${esc(longDate(p.cutoff))}, ${p.older.length} delivery dates) <button class="btn small" id="toggleOlder">${showOlder ? "Hide" : "Show"}</button></td></tr>`
        + (showOlder ? p.older.map((r) => `<tr class="muted"><td></td><td class="num">${fmt(r.n)}</td><td>(${esc(longDate(r.date))} ${esc(r.date.slice(0, 4))})</td></tr>`).join("") : "") : "")
      + (!p.recent.length && !p.olderN ? row("Pending", 0, "") : "");
    return `<div class="panel"><div class="btnrow" style="justify-content:space-between;align-items:center"><h3 style="margin:0">3PL Daily Update <span class="muted">${esc(longDate(day))}</span></h3><button class="btn small" id="copyReport" title="Copy the report as text">Copy report</button></div>
      <div class="tablewrap" style="max-height:none;margin-top:10px"><table class="adm-table"><tbody>
        ${row("Total Deliveries Arrived", rep.arrived, "")}${shipped}${pend}
      </tbody></table></div>
      <p class="hint" style="margin-top:8px"><b>Arrived:</b> packages with this Delivery Date that are Delivered. <b>Orders Shipped:</b> packages shipped to the customer on this day, by their Delivery Date. <b>Pending:</b> Delivered packages with this or an earlier Delivery Date that were not shipped by the end of this day.</p></div>`;
  };

  const draw = () => {
    // a redraw rebuilds the page, which would throw the list's scroll position (and the page's) back to the top: keep both
    const wrap = el.querySelector("#labelsWrap"), listTop = wrap ? wrap.scrollTop : 0, pageTop = window.scrollY;
    lastSig = signature();
    el.innerHTML = `<div class="section-head"><div><div class="eyebrow">${esc(brand.name)}</div><h2>Admin</h2>
        <p>Things to fix in All Orders, and the 3PL Daily Update. Updates live.</p></div>
        <div class="btnrow dash-date"><button class="btn" id="prev" title="Day before">‹</button>
          <label class="field" style="margin:0"><span>Select day</span><input type="date" id="day" value="${day}"></label>
          <button class="btn" id="next" title="Next day">›</button>
          ${day === today ? "" : '<button class="btn" id="thisDay">Today</button>'}</div></div>
      <div class="dash-cards">${cards.map((c) => `<a class="scard warn dash-card${c.panel && panel === c.panel ? " on" : ""}" ${c.panel ? `href="#admin" data-panel="${esc(c.panel)}" title="Show the list"` : `href="${esc(c.link)}" title="Show these orders in All Orders"`} style="text-decoration:none;color:inherit"><div class="dash-n">${fmt(c.n)}</div>
        <div class="dash-l">${esc(c.label)}<div class="muted">${esc(c.note)}</div></div></a>`).join("")}</div>
      ${panel === "labels" ? labelsPanel() : ""}
      ${reportPanel()}`;
    const wrap2 = el.querySelector("#labelsWrap");
    if (wrap2) wrap2.scrollTop = listTop;
    if (window.scrollY !== pageTop) window.scrollTo(0, pageTop);
  };

  // the label links whose file name does not fit the order: Label Number / Tracking Number + Customer Name
  const labelsPanel = () => {
    if (!labels) return `<div class="panel"><span class="muted">Loading…</span></div>`;
    const rows = labels.mismatches.slice(0, 500);
    return `<div class="panel"><div class="btnrow" style="justify-content:space-between;align-items:center"><b>Label links that do not fit the order</b><button class="btn small" id="closePanel">Close</button></div>
      <p class="muted" style="margin:6px 0 10px">A label file is named Customer name + tracking number, or label number + Customer name. These files do not hold the whole customer name, or the label / tracking number (a name the label maker cut short is flagged too). Open the order and check the file. If it is the right label, click <b>Approve</b>: it leaves this list and the card count. Checked ${fmt(labels.checked)} of ${fmt(labels.total)} links so far${labels.unread ? `; ${fmt(labels.unread)} could not be read (private or deleted)` : ""}. Google is read a few files at a time in the background, so this list grows until every link is checked.</p>
      ${rows.length ? `<div class="tablewrap" id="labelsWrap" style="max-height:420px"><table><thead><tr><th>Label</th><th>Customer</th><th>Tracking to customer</th><th>File name</th><th>Problem</th><th></th></tr></thead><tbody>
        ${rows.map((m) => `<tr><td><a href="#orders?${new URLSearchParams({ q: m.label, row: String(m.rn), field: "Shipping Label Link", t: String(Date.now()) })}">${esc(m.label)}</a></td><td>${esc(m.customer)}</td><td class="mono">${esc(m.tracking)}</td><td class="clip" title="${esc(m.file)}">${esc(m.file)}</td><td>${esc(m.why)}</td><td><button class="btn small" data-approve="${m.rn}|${esc(m.file_id)}" title="I checked this label: it is the right one">Approve</button></td></tr>`).join("")}</tbody></table></div>${labels.mismatches.length > rows.length ? `<p class="hint">Showing the first ${rows.length} of ${fmt(labels.mismatches.length)}.</p>` : ""}` : `<p>No mismatches among the links checked so far.</p>`}
      ${approvedPart()}</div>`;
  };
  // the ones somebody approved: kept apart, so a wrong click can be taken back
  const approvedPart = () => {
    const list = labels.approved || [];
    if (!list.length) return "";
    return `<div style="margin-top:12px"><button class="btn small" id="toggleApproved">${showApproved ? "Hide" : "Show"} approved (${fmt(list.length)})</button>
      ${showApproved ? `<div class="tablewrap" style="max-height:260px;margin-top:8px"><table><thead><tr><th>Label</th><th>Customer</th><th>File name</th><th>Approved by</th><th></th></tr></thead><tbody>
        ${list.map((m) => `<tr><td>${esc(m.label)}</td><td>${esc(m.customer)}</td><td class="clip" title="${esc(m.file)}">${esc(m.file)}</td><td>${esc(m.by)} <span class="muted">${esc(m.at)}</span></td><td><button class="btn small" data-undo="${m.rn}|${esc(m.file_id)}">Undo</button></td></tr>`).join("")}</tbody></table></div>` : ""}</div>`;
  };
  async function setApproval(rn, fid, yes) {
    try {
      await api("/api/admin/labels/approve", { rn, file_id: fid, approve: yes });
      const [l, c] = await Promise.all([api("/api/admin/labels"), api("/api/admin/cards")]);
      labels = l; cards = c.cards; draw();
    } catch (e) { toast(e.message, "bad"); draw(); }
  }
  async function loadLabels() { try { labels = await api("/api/admin/labels"); draw(); } catch (e) { toast(e.message, "bad"); } }

  let seq = 0;
  async function load() {
    const my = ++seq;
    rep = null;
    draw();
    try {
      const [r, c] = await Promise.all([api("/api/admin/daily?date=" + day), api("/api/admin/cards")]);
      if (my !== seq) return;
      rep = r; cards = c.cards;
      draw();
    } catch (e) { toast(e.message, "bad"); }
  }
  const go = (n) => { const [y, m, d] = day.split("-").map(Number); day = isoOf(new Date(y, m - 1, d + n)); load(); };
  el.onchange = (e) => {
    if (e.target.id !== "day" || !/^\d{4}-\d\d-\d\d$/.test(e.target.value)) return;
    day = e.target.value; load();
  };
  el.onclick = (e) => {
    const card = e.target.closest("[data-panel]");
    if (card) { e.preventDefault(); panel = panel === card.dataset.panel ? "" : card.dataset.panel; labels = null; draw(); if (panel === "labels") loadLabels(); return; }
    const b = e.target.closest("button");
    if (!b) return;
    if (b.dataset.approve || b.dataset.undo) { const [rn, fid] = (b.dataset.approve || b.dataset.undo).split("|"); b.disabled = true; setApproval(+rn, fid, !!b.dataset.approve); return; }
    if (b.id === "toggleOlder") { showOlder = !showOlder; draw(); return; }
    if (b.id === "toggleApproved") { showApproved = !showApproved; draw(); return; }
    if (b.id === "closePanel") { panel = ""; draw(); return; }
    if (b.id === "prev") go(-1);
    else if (b.id === "next") go(1);
    else if (b.id === "thisDay") { day = today; load(); }
    else if (b.id === "copyReport") {
      navigator.clipboard.writeText(reportText()).then(() => toast("Report copied"), () => toast("Could not copy: select the text and copy it", "bad"));
    }
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
      const [r, c] = await Promise.all([api("/api/admin/daily?date=" + day), api("/api/admin/cards")]);
      if (r.date !== day) return; // the day was changed while this was loading
      rep = r; cards = c.cards;
      if (panel === "labels") labels = await api("/api/admin/labels");
      if (signature() !== lastSig) draw(); // nothing changed on the page: leave it alone (no jump to the top)
    } catch { /* offline for a moment */ }
  }, 3000);
}
