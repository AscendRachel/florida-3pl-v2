// The bell in the top bar: notes other people wrote on orders. Click it for a list; click a notification to open that order's row.
import { $, ago, api, esc, roleNames } from "./util.js";

const ROLE = { admin: "Admin", warehouse: "Warehouse", member: "Member" };
const state = { items: [], unread: 0, open: false, fresh: new Set(), orders: true };
let timer = null, popup = null, outside = null;

const bellHtml = () => `<button class="btn small bell" id="bellBtn" title="Notifications" aria-label="Notifications" aria-haspopup="true">
    <svg viewBox="0 0 24 24" width="17" height="17" aria-hidden="true"><path d="M12 3a6 6 0 0 0-6 6v3.5L4.5 16h15L18 12.5V9a6 6 0 0 0-6-6zM10 19a2 2 0 0 0 4 0" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>
    <span class="bell-n" id="bellN" hidden></span></button>`;
export const bell = bellHtml;

// the red number on the bell (called after the top bar is drawn, and whenever the count changes)
export function paintBell() {
  const n = $("#bellN");
  if (!n) return;
  n.textContent = state.unread > 99 ? "99+" : String(state.unread);
  n.hidden = !state.unread;
  $("#bellBtn")?.classList.toggle("on", state.open);
}

// the link of one notification: All Orders searched for that order, with its row picked out
// what the person did: a plain note, or an answer to / a rewrite of somebody's note (told in particular to the one who wrote it)
const what = (it) => {
  const verb = it.kind === "reply" ? "replied to" : "rewrote";
  if (!it.kind) return "wrote a note";
  return it.mine ? `${verb} <b>your</b> note` : `${verb} ${esc(it.to_name || "another person")}'s note`;
};
// (an account that has Order Cards but not All Orders is taken to the card of that order instead)
const link = (it) => state.orders
  ? `#orders?${new URLSearchParams({ q: it.label || "", row: String(it.rn), field: it.field, t: String(Date.now()) })}`
  : `#cards?${new URLSearchParams({ q: it.label || "" })}`;

function drawPopup() {
  if (!popup) return;
  const rows = state.items.map((it) => `<a class="nt-item${state.fresh.has(it.id) ? " new" : ""}${it.mine ? " mine" : ""}" href="${esc(link(it))}">
      <span class="nt-av" aria-hidden="true">${esc((it.by_name || "?").trim().charAt(0).toUpperCase())}</span>
      <span class="nt-body">
        <span class="nt-line"><b>${esc(it.by_name)}</b> <span class="nt-role">${esc(roleNames[it.by_role] || ROLE[it.by_role] || it.by_role || "Member")}</span> ${what(it)}</span>
        <span class="nt-order">${esc(it.label || "Order")}${it.customer ? ` · ${esc(it.customer)}` : ""}${it.field === "Notes" ? "" : " · Internal notes"}</span>
        <span class="nt-text">${esc(it.text)}</span>
        <span class="nt-time">${esc(ago(it.at))}</span>
      </span>${state.fresh.has(it.id) ? '<span class="nt-dot" title="New"></span>' : ""}</a>`).join("");
  popup.querySelector(".nt-list").innerHTML = rows || `<div class="nt-empty"><b>No notifications yet.</b><p>When someone writes a note on an order, you will see it here.</p></div>`;
}

function close() {
  if (!popup) return;
  popup.remove(); popup = null; state.open = false;
  document.removeEventListener("mousedown", outside, true);
  document.removeEventListener("keydown", onKey, true);
  paintBell();
}
const onKey = (e) => { if (e.key === "Escape") close(); };

async function open() {
  const btn = $("#bellBtn");
  if (!btn) return;
  state.open = true;
  state.fresh = new Set(state.items.filter((i) => i.unread).map((i) => i.id)); // what was new stays marked while the list is open
  popup = document.createElement("div");
  popup.className = "nt-pop";
  popup.setAttribute("role", "dialog");
  popup.setAttribute("aria-label", "Notifications");
  popup.innerHTML = `<div class="nt-head"><b>Notifications</b><span class="muted">Notes other people wrote on orders</span></div><div class="nt-list"></div>`;
  document.body.append(popup);
  const r = btn.getBoundingClientRect(), w = Math.min(380, innerWidth - 16);
  popup.style.width = w + "px";
  popup.style.top = r.bottom + 8 + "px";
  popup.style.left = Math.max(8, Math.min(r.right - w, innerWidth - w - 8)) + "px";
  popup.style.maxHeight = Math.max(240, innerHeight - r.bottom - 24) + "px";
  drawPopup();
  popup.addEventListener("click", (e) => { if (e.target.closest("a.nt-item")) close(); });
  outside = (e) => { if (!popup?.contains(e.target) && !e.target.closest("#bellBtn")) close(); };
  document.addEventListener("mousedown", outside, true);
  document.addEventListener("keydown", onKey, true);
  state.unread = 0; paintBell();
  try { await api("/api/notify/seen", {}); } catch { /* the number comes back at the next check if this did not go through */ }
}

async function refresh() {
  if (document.hidden && !state.open) return;
  try {
    const r = await api("/api/notify");
    state.items = r.items;
    state.orders = r.can_orders !== false;
    state.unread = state.open ? 0 : r.unread;
    if (state.open && r.unread) api("/api/notify/seen", {}).catch(() => {});
    paintBell(); drawPopup();
  } catch { /* offline for a moment, or signed out */ }
}

document.addEventListener("click", (e) => {
  const b = e.target.closest("#bellBtn");
  if (!b) return;
  if (state.open) close(); else open();
});

export function startNotifications() {
  clearInterval(timer);
  refresh();
  timer = setInterval(refresh, 4000);
  document.addEventListener("visibilitychange", () => { if (!document.hidden) refresh(); });
}
