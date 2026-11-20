// Use Stock: send a product to a customer order from the stock that was set aside, and see what has been used.
// Process: type the STOCK NO., choose the stock item, say which customer order it goes to and how many. The stock item's QTY is never
// changed: what is left is its QTY minus what was used, and a mistake can be put back from the Used list.
import { $, api, brand, debounce, esc, fmt, toast } from "./util.js";

const SOURCE = { order: "All Orders", kohls: "Kohl's Returns" };
const isUrl = (v) => /^https?:\/\//.test(v || "");
const field = (name, html) => (html.replace(/<[^>]*>/g, "").trim() === "" ? "" : `<div class="cf"><span class="cl">${esc(name)}</span><div class="cv">${html}</div></div>`);

export async function mount(el, params, ctx) {
  const tab = params.get("tab") === "used" ? "used" : "process";
  el.innerHTML = `<div class="section-head"><div><div class="eyebrow">${esc(brand.name)}</div><h2>Use Stock</h2>
      <p>Send a product to a customer from the stock that was set aside. Times are US Eastern.</p></div>
      <div class="cardtabs"><a href="#stockuse"${tab === "process" ? ' class="on"' : ""}>Process</a><a href="#stockuse?tab=used"${tab === "used" ? ' class="on"' : ""}>Used from stock</a></div></div>
    <div id="su"></div>`;
  const root = $("#su");
  return tab === "used" ? usedView(root, ctx) : processView(root);
}

// ---------------------------------------------------------------- process
function processView(root) {
  root.innerHTML = `<div class="panel" style="margin-bottom:14px"><form id="suFind" class="btnrow" style="align-items:end">
      <label class="field" style="flex:1;min-width:240px;margin:0"><span>STOCK NO.</span><input id="suNo" name="no" autocomplete="off" spellcheck="false" placeholder="Type or scan the stock number" autofocus></label>
      <button class="btn primary" type="submit">Find</button></form></div>
    <div class="cardgrid" id="suItems"></div>`;
  const input = $("#suNo"), box = $("#suItems");
  let items = [], last = "";

  const itemCard = (it, n) => {
    const none = it.remaining <= 0;
    return `<article class="ocard" data-n="${n}">
      ${isUrl(it.image) ? `<div class="oc-img"><img loading="lazy" referrerpolicy="no-referrer" src="${esc(it.image)}" alt=""></div>` : `<div class="oc-img none"></div>`}
      <div class="oc-body">
        ${field("STOCK NO.:", `<b>${esc(it.stock_no)}</b>`)}
        ${field("Store:", esc(it.store))}
        ${field("SKU:", esc(it.sku))}
        ${field("Product Name:", `<small>${esc(it.product)}</small>`)}
        ${field("From:", esc(SOURCE[it.source] || it.source))}
        <hr>
        <div class="cf"><span class="cl">In stock now:</span><div class="cv"><b style="font-size:1.3em">${fmt(it.remaining)}</b> <span class="muted">of ${fmt(it.qty)}${it.used ? ` · ${fmt(it.used)} used` : ""}</span></div></div>
        ${none ? `<p class="muted">Nothing left of this one.</p>` : `<hr>
        <label class="field"><span>Customer order (Store Order No. or Label Number)</span><input class="su-order" autocomplete="off" spellcheck="false"></label>
        <div class="su-hint muted" style="min-height:1.2em;margin:-4px 0 8px;font-size:.9em"></div>
        <div class="formgrid" style="grid-template-columns:110px 1fr"><label class="field"><span>Qty to use</span><input class="su-qty" type="number" min="1" max="${it.remaining}" value="1" inputmode="numeric"></label>
          <label class="field"><span>Note (optional)</span><input class="su-note" maxlength="500" autocomplete="off"></label></div>`}
      </div>
      ${none ? "" : `<div class="oc-actions"><button class="btn primary" data-use="${n}">Use from stock</button></div>`}
    </article>`;
  };
  const draw = (msg) => {
    box.innerHTML = items.length ? items.map(itemCard).join("") : `<div class="empty">${msg || ""}</div>`;
  };

  async function find(no) {
    last = no;
    if (!no) { items = []; return draw(""); }
    try {
      items = (await api("/api/stockuse/find?" + new URLSearchParams({ stock_no: no }))).items;
      draw(`No stock with the STOCK NO. "${esc(no)}". Check the number, or that Stock is ticked on that order in All Orders.`);
      if (items.length === 1) box.querySelector(".su-order")?.focus();
    } catch (e) { toast(e.message, "bad"); }
  }
  $("#suFind").onsubmit = (e) => { e.preventDefault(); find(input.value.trim()); };

  // show what the typed customer order is, so a wrong number is seen before the stock is used
  const lookup = debounce(async (inp) => {
    const hint = inp.closest("article")?.querySelector(".su-hint");
    if (!hint) return;
    const ref = inp.value.trim();
    if (!ref) { hint.textContent = ""; return; }
    try {
      const { orders } = await api("/api/stockuse/order?" + new URLSearchParams({ ref }));
      if (inp.value.trim() !== ref) return;
      hint.style.color = orders.length ? "" : "var(--warn, #b7791f)";
      hint.textContent = orders.length ? orders.map((o) => [o.customer, o.product, o.store && `(${o.store})`].filter(Boolean).join(" · ")).join("  |  ") : "No order with that number in All Orders yet. It will still be saved as typed.";
    } catch { /* the hint is only a help */ }
  }, 300);
  box.oninput = (e) => { if (e.target.classList.contains("su-order")) lookup(e.target); };

  box.onclick = async (e) => {
    const b = e.target.closest("[data-use]");
    if (!b) return;
    const card = b.closest("article"), it = items[+b.dataset.use];
    const order_ref = card.querySelector(".su-order").value.trim(), qty = parseInt(card.querySelector(".su-qty").value, 10), note = card.querySelector(".su-note").value.trim();
    if (!order_ref) { toast("Enter the customer order this stock goes to.", "bad"); return card.querySelector(".su-order").focus(); }
    if (!(qty >= 1)) return toast("Enter how many to use (1 or more).", "bad");
    if (qty > it.remaining) return toast(`Only ${it.remaining} left in stock.`, "bad");
    b.disabled = true;
    try {
      const out = await api("/api/stockuse/use", { source: it.source, ref: it.ref, qty, order_ref, note });
      toast(`Used ${qty} of ${it.stock_no} for ${order_ref}.${out.item ? ` ${out.item.remaining} left.` : ""}`);
      await find(last);
      input.select();
    } catch (err) { toast(err.message, "bad"); b.disabled = false; }
  };
  input.focus();
}

// ---------------------------------------------------------------- used
async function usedView(root, ctx) {
  root.innerHTML = `<div class="panel"><div class="btnrow" style="align-items:end"><label class="field wl-search" style="flex:1;margin:0"><span>Search</span><input type="search" id="usQ" placeholder="Stock No., SKU, product, customer order, person" autocomplete="off"></label>
      <span class="muted" id="usCount"></span></div>
    <div class="tablewrap" style="max-height:none;margin-top:10px"><table class="wl-table"><thead><tr><th>Date</th><th>STOCK NO.</th><th>Product</th><th>SKU</th><th>Qty</th><th>Customer order</th><th>By</th><th></th></tr></thead><tbody id="usRows"></tbody></table></div></div>`;
  let q = "", known = null;
  async function load() {
    const d = await api("/api/stockuse/used?" + new URLSearchParams({ q }));
    $("#usCount").textContent = d.rows.length ? `${fmt(d.rows.length)} use${d.rows.length === 1 ? "" : "s"} · ${fmt(d.total_qty)} unit${d.total_qty === 1 ? "" : "s"}` : "";
    $("#usRows").innerHTML = d.rows.length
      ? d.rows.map((r) => `<tr><td class="wl-date">${esc(r.at)}</td><td><b>${esc(r.stock_no)}</b></td><td>${esc(r.product)}${r.note ? `<br><small class="muted">${esc(r.note)}</small>` : ""}</td><td>${esc(r.sku)}</td><td><b>${fmt(r.qty)}</b></td><td>${esc(r.order_ref)}</td><td>${esc(r.by)}</td><td>${r.can_undo ? `<button class="btn small" data-undo="${r.id}" title="Put this back in stock">Undo</button>` : ""}</td></tr>`).join("")
      : `<tr><td colspan="8" class="empty">${q ? "Nothing matches." : "Nothing has been used from stock yet."}</td></tr>`;
  }
  const reload = () => load().catch((e) => toast(e.message, "bad"));
  root.oninput = debounce((e) => { if (e.target.id === "usQ") { q = e.target.value.trim(); reload(); } }, 250);
  root.onclick = async (e) => {
    const b = e.target.closest("[data-undo]");
    if (!b) return;
    const row = b.closest("tr");
    if (!confirm(`Put this back in stock?\n\n${row.children[1].textContent} · qty ${row.children[4].textContent} · ${row.children[5].textContent}`)) return;
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
