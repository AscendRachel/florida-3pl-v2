// Stock History: what has been used from stock (who, when, how many). Using stock happens on the cards of Order Cards > Stocks.
// A mistake can be put back from here.
import { $, api, brand, debounce, esc, fmt, toast } from "./util.js";

export async function mount(el, params, ctx) {
  el.innerHTML = `<div class="section-head"><div><div class="eyebrow">${esc(brand.name)}</div><h2>Stock History</h2>
      <p>The products used from stock. To use stock, press Use Stock on a card in Order Cards &gt; Stocks. Times are US Eastern.</p></div></div>
    <div id="su"></div>`;
  return usedView($("#su"), ctx);
}

// ---------------------------------------------------------------- used
async function usedView(root, ctx) {
  root.innerHTML = `<div class="panel"><div class="btnrow" style="align-items:end"><label class="field wl-search" style="flex:1;margin:0"><span>Search</span><input type="search" id="usQ" placeholder="Stock No., SKU, product, person" autocomplete="off"></label>
      <span class="muted" id="usCount"></span></div>
    <div class="tablewrap" style="max-height:none;margin-top:10px"><table class="wl-table"><thead><tr><th>Date</th><th>STOCK NO.</th><th>Product</th><th>SKU</th><th>Qty</th><th>By</th><th></th></tr></thead><tbody id="usRows"></tbody></table></div></div>`;
  let q = "", known = null;
  async function load() {
    const d = await api("/api/stockuse/used?" + new URLSearchParams({ q }));
    $("#usCount").textContent = d.rows.length ? `${fmt(d.rows.length)} use${d.rows.length === 1 ? "" : "s"} · ${fmt(d.total_qty)} unit${d.total_qty === 1 ? "" : "s"}` : "";
    $("#usRows").innerHTML = d.rows.length
      ? d.rows.map((r) => `<tr><td class="wl-date">${esc(r.at)}</td><td><b>${esc(r.stock_no)}</b></td><td>${esc(r.product)}${r.note ? `<br><small class="muted">${esc(r.note)}</small>` : ""}</td><td>${esc(r.sku)}</td><td><b>${fmt(r.qty)}</b></td><td>${esc(r.by)}</td><td>${r.can_undo ? `<button class="btn small" data-undo="${r.id}" title="Put this back in stock">Undo</button>` : ""}</td></tr>`).join("")
      : `<tr><td colspan="7" class="empty">${q ? "Nothing matches." : "Nothing has been used from stock yet."}</td></tr>`;
  }
  const reload = () => load().catch((e) => toast(e.message, "bad"));
  root.oninput = debounce((e) => { if (e.target.id === "usQ") { q = e.target.value.trim(); reload(); } }, 250);
  root.onclick = async (e) => {
    const b = e.target.closest("[data-undo]");
    if (!b) return;
    const row = b.closest("tr");
    if (!confirm(`Put this back in stock?\n\n${row.children[1].textContent} · qty ${row.children[4].textContent}`)) return;
    b.disabled = true;
    try { await api("/api/stockuse/undo", { id: +b.dataset.undo }); toast("Put back in stock."); await load(); } catch (err) { toast(err.message, "bad"); b.disabled = false; }
  };
  await load();
  const timer = setInterval(async () => {
    if (!root.isConnected || location.hash.split("?")[0] !== "#stockuse") return clearInterval(timer);
    if (document.hidden) return;
    try {
      const { v } = await api("/api/version");
      if (known === null) known = v;
      else if (v !== known) { known = v; await load(); }
    } catch { /* offline for a moment */ }
  }, 3000);
}
