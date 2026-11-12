// Source Time Frame: the orders whose ETA moved to another date. Each with its Source Order Date and Number, the first ETA that was
// recorded, the current ETA, how many times it moved, and how many days it moved in all (New ETA minus First ETA). Read only.
import { $, api, brand, esc, fmt, toast } from "./util.js";

const COLS = [
  ["sod", "Source Order Date"], ["son", "Source Order Number"], ["first", "First ETA"], ["eta", "New ETA"], ["changes", "Changes Count"], ["frame", "Delivery Time Frame"],
];

export async function mount(el) {
  const st = { q: "", sort: "sod", dir: "desc", page: 1 };
  let data = null, gen = 0, timer = null;

  el.innerHTML = `<div class="section-head"><div><div class="eyebrow">${esc(brand.name)}</div><h2>Source Time Frame</h2>
      <p>The orders whose estimated delivery date (ETA) was moved to another date, for example by an Amazon order import. <b>First ETA</b> is the first date that was recorded, <b>New ETA</b> is the current Delivery Date, <b>Changes Count</b> is how many times it moved, and <b>Delivery Time Frame</b> is how many days it moved in all (New ETA minus First ETA).</p></div></div>
    <div class="panel"><div class="btnrow" style="gap:12px;align-items:center;flex-wrap:wrap">
      <input type="search" id="tfq" placeholder="Search a Source Order Number or date" autocomplete="off" style="min-width:260px">
      <span class="muted" id="tfcount"></span></div>
      <div class="tablewrap" style="max-height:none;margin-top:10px"><table style="min-width:760px"><thead><tr id="tfhead"></tr></thead><tbody id="tfbody"></tbody></table></div>
      <div class="btnrow" style="justify-content:center;align-items:center;margin-top:10px;gap:10px">
        <button class="btn small" id="tfprev">‹ Previous</button><span class="muted" id="tfpage"></span><button class="btn small" id="tfnext">Next ›</button></div></div>`;

  const head = () => {
    $("#tfhead").innerHTML = COLS.map(([k, name]) => `<th data-sort="${k}" style="cursor:pointer;white-space:nowrap" title="Sort by ${esc(name)}"${k === "changes" || k === "frame" ? ' class="num"' : ""}>${esc(name)}${st.sort === k ? (st.dir === "asc" ? " ▲" : " ▼") : ""}</th>`).join("");
  };

  const draw = () => {
    const rows = data.items;
    $("#tfbody").innerHTML = rows.length
      ? rows.map((r) => `<tr><td>${esc(r.source_date)}</td>
          <td><a href="#orders?${esc(new URLSearchParams({ q: r.source_order, row: String(r.rn), field: "Notes", t: String(Date.now()) }).toString())}" title="Open this order in All Orders" class="mono">${esc(r.source_order)}</a></td>
          <td>${esc(r.first_eta)}</td><td>${esc(r.new_eta)}</td><td class="num">${r.changes ? `<b>${fmt(r.changes)}</b>` : "0"}</td>
          <td class="num">${r.frame == null ? '<span class="muted">—</span>' : `${r.frame > 0 ? "+" : ""}${fmt(r.frame)} ${Math.abs(r.frame) === 1 ? "day" : "days"}`}</td></tr>`).join("")
      : `<tr><td colspan="6" class="muted">${st.q ? "No orders match." : "No order has had its ETA changed yet."}</td></tr>`;
    $("#tfcount").textContent = `${fmt(data.total)} order${data.total === 1 ? "" : "s"}`;
    $("#tfpage").textContent = `Page ${fmt(data.page)} of ${fmt(data.pages)}`;
    $("#tfprev").disabled = data.page <= 1;
    $("#tfnext").disabled = data.page >= data.pages;
  };

  async function load() {
    const my = ++gen;
    try {
      const out = await api("/api/timeframe?" + new URLSearchParams({ q: st.q, sort: st.sort, dir: st.dir, page: String(st.page) }));
      if (my !== gen) return;
      data = out;
      if (st.page > out.pages) { st.page = out.pages; return load(); }
      head(); draw();
    } catch (e) { toast(e.message, "bad"); }
  }

  el.addEventListener("input", (e) => {
    if (e.target.id !== "tfq") return;
    clearTimeout(timer);
    timer = setTimeout(() => { st.q = e.target.value.trim(); st.page = 1; load(); }, 400);
  });
  el.addEventListener("click", (e) => {
    const th = e.target.closest("[data-sort]");
    if (th) { const k = th.dataset.sort; st.dir = st.sort === k ? (st.dir === "asc" ? "desc" : "asc") : (k === "son" ? "asc" : "desc"); st.sort = k; st.page = 1; return void load(); }
    if (e.target.closest("#tfprev") && st.page > 1) { st.page--; load(); }
    if (e.target.closest("#tfnext") && data && st.page < data.pages) { st.page++; load(); }
  });

  head();
  await load();
}
