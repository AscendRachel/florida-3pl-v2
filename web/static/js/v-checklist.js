// Team Daily Check List: the daily tasks of the team, one row each in the colour of the agent assigned to it, with Doing / Done / Remarks
// ticks for the chosen day, the numbers (counted by the app for some tasks, typed for the others), and the two free lists beside it.
import { api, brand, dialog, esc, fmt, toast } from "./util.js";

const isoOf = (d) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
const FIELDS = [["doing", "DOING"], ["done", "DONE"], ["remarks", "REMARKS"]];

export async function mount(el, params, ctx) {
  let day = "", data = null, lastSig = "", known = null; // day "" = the app's today (US Eastern, as everywhere in the app)
  const me = ctx.me;

  const colorOf = (agent) => data.agents.find((a) => a.agent === agent)?.color || "#ffffff";
  const busy = () => { const a = document.activeElement; return !!a && el.contains(a) && (a.tagName === "TEXTAREA" || (a.tagName === "INPUT" && a.type !== "checkbox" && a.type !== "date")); };

  // the Additional Task and SKU columns: one editable cell in every row, kept in the row where it was typed
  const COLUMNS = [["additional", "ADDITIONAL TASK", "#c9daf8"], ["sku", "SKU WITH MORE THAN ONE BOX", "#d9d2e9"]];
  const cells = (i) => COLUMNS.map(([kind, , bg]) => {
    const it = data[kind].find((x) => x.pos === i);
    return `<td class="chk-x" style="background:${bg}"><textarea data-kind="${kind}" data-pos="${i}" rows="1" aria-label="${kind === "sku" ? "SKU" : "Additional task"} row ${i + 1}" title="${it && it.by ? "Last changed by " + esc(it.by) : ""}">${esc(it ? it.text : "")}</textarea></td>`;
  }).join("");
  // the rows: one per task, and below them as many as the two columns need (plus one empty one to keep going)
  const rowCount = () => Math.max(data.tasks.length, ...COLUMNS.flatMap(([k]) => data[k].map((x) => x.pos + 2)));

  const taskRow = (t, i) => {
    const bg = colorOf(t.agent);
    const boxes = FIELDS.map(([f]) => `<td class="chk-c"><input type="checkbox" data-mark="${t.code}|${f}" ${t[f] ? "checked" : ""} title="${t[f] && t[f + "_by"] ? "Ticked by " + esc(t[f + "_by"]) : "Tick when " + f}" aria-label="${f} ${t.code}"></td>`).join("");
    const num = t.counted
      ? `<td class="chk-n" title="${esc(t.how)}">${t.link && t.n != null ? `<a href="${esc(t.link)}" style="color:inherit">${fmt(t.n)}</a>` : t.n == null ? "–" : fmt(t.n)}</td>`
      : `<td class="chk-n typed"><textarea data-num="${t.code}" rows="1" placeholder="" aria-label="Numbers of ${t.code}">${esc(t.number_text)}</textarea></td>`;
    return `<tr style="background:${bg}"><td class="chk-t"><b>${t.code}.</b> ${esc(t.title)}${ctx.isAdmin() ? ` <button class="chk-edit" data-edit="${t.code}" title="Change this task or who it is assigned to">✎</button>` : ""}</td>${boxes}${num}${cells(i)}</tr>`;
  };
  const extraRow = (i) => `<tr class="chk-extra"><td colspan="5" class="chk-blank"></td>${cells(i)}</tr>`;

  const draw = () => {
    if (!data) return;
    lastSig = JSON.stringify(data);
    const pageTop = window.scrollY;
    el.innerHTML = `<div class="section-head"><div><div class="eyebrow">${esc(brand.name)}</div><h2>Team Daily Check List</h2>
        <p>The tasks of the day. Use your assigned colour to identify that you are the one doing the task. Updates live.</p></div>
        <div class="btnrow dash-date"><button class="btn" id="prev" title="Day before">‹</button>
          <label class="field" style="margin:0"><span>Select day</span><input type="date" id="day" value="${esc(day)}"></label>
          <button class="btn" id="next" title="Next day">›</button>
          ${day === data.today ? "" : '<button class="btn" id="today">Today</button>'}</div></div>
      <div class="dash-cards" style="margin-bottom:12px"><a class="scard warn dash-card" href="#orders?show=preship" title="Show these orders in All Orders" style="text-decoration:none;color:inherit"><div class="dash-n">${fmt(data.preshipment)}</div>
        <div class="dash-l">Total Preshipment<div class="muted">Tracking Status is Preshipment, leaving out For Refund, Refunded, Cancelled, Unfulfilled, empty rows (no Store Order No.), and the ones ticked To Void or Package Not Received</div></div></a></div>
      <div class="chk-legend"><div class="chk-head">AGENT LEGEND</div>${data.agents.map((a) => `<div class="chk-agent" style="background:${esc(a.color)}">${esc(a.name)}</div>`).join("")}</div>
      <p class="chk-rule">Use your assigned color to identify that you're the one doing the task.</p>
      <div class="chk-main tablewrap" style="max-height:none"><table class="chk-table"><thead><tr><th>DAILY TASK</th>${FIELDS.map(([, l]) => `<th>${l}</th>`).join("")}<th>NUMBERS</th>${COLUMNS.map(([, l]) => `<th>${l}</th>`).join("")}</tr></thead>
        <tbody>${Array.from({ length: rowCount() }, (_, i) => (data.tasks[i] ? taskRow(data.tasks[i], i) : extraRow(i))).join("")}</tbody></table></div>
      <details class="chk-how"><summary>How the numbers are counted</summary><ul>${data.tasks.filter((t) => t.counted).map((t) => `<li><b>${t.code}.</b> ${esc(t.how)}</li>`).join("")}</ul>
        <p class="hint">The other tasks have a box to type in. The number of A is "right now": on an earlier day it shows what it was the last time somebody had this page open that day. The others count what happened on the selected day. The Additional Task and SKU columns do not depend on the day: a cell stays until somebody clears it.</p></details>`;
    el.querySelectorAll("textarea").forEach(grow);
    if (window.scrollY !== pageTop) window.scrollTo(0, pageTop);
  };
  const grow = (t) => { t.style.height = "auto"; t.style.height = Math.min(220, t.scrollHeight + 2) + "px"; };

  async function load(redraw = true) {
    try {
      const d = await api("/api/checklist?date=" + day);
      if (day && d.date !== day) return; // the day was changed while this was loading
      day = d.date;
      data = d;
      if (redraw && JSON.stringify(data) !== lastSig) draw();
    } catch (e) { toast(e.message, "bad"); }
  }

  el.onchange = async (e) => {
    const t = e.target;
    try {
      if (t.id === "day") { if (/^\d{4}-\d\d-\d\d$/.test(t.value)) { day = t.value; data = null; await load(); } return; }
      if (t.dataset.mark) {
        const [code, field] = t.dataset.mark.split("|");
        await api("/api/checklist/mark", { date: day, task: code, field, value: t.checked });
        const task = data.tasks.find((x) => x.code === code);
        if (task) { task[field] = t.checked; task[field + "_by"] = t.checked ? me.name : ""; t.title = t.checked ? "Ticked by " + me.name : "Tick when " + field; lastSig = JSON.stringify(data); }
      } else if (t.dataset.num) {
        await api("/api/checklist/number", { date: day, task: t.dataset.num, text: t.value });
        const task = data.tasks.find((x) => x.code === t.dataset.num);
        if (task) { task.number_text = t.value; lastSig = JSON.stringify(data); }
      } else if (t.dataset.pos) {
        const kind = t.dataset.kind, pos = +t.dataset.pos;
        await api("/api/checklist/cell", { kind, pos, text: t.value });
        const list = data[kind], at = list.findIndex((x) => x.pos === pos);
        if (!t.value.trim()) { if (at >= 0) list.splice(at, 1); }
        else if (at >= 0) Object.assign(list[at], { text: t.value, by: me.name });
        else list.push({ pos, text: t.value, by: me.name });
        lastSig = JSON.stringify(data);
        // the last row got text: one more empty row appears below it (added without redrawing, so a click on something else still lands)
        const body = el.querySelector(".chk-table tbody");
        for (let n = body.children.length; n < pos + 2; n++) body.insertAdjacentHTML("beforeend", extraRow(n));
      }
    } catch (err) {
      toast(err.message, "bad");
      await load(); // show what is really saved
    }
  };
  el.oninput = (e) => { if (e.target.tagName === "TEXTAREA") grow(e.target); };
  el.onclick = async (e) => {
    const b = e.target.closest("button");
    if (!b) return;
    try {
      if (b.id === "prev" || b.id === "next") { const [y, m, d] = day.split("-").map(Number); day = isoOf(new Date(y, m - 1, d + (b.id === "next" ? 1 : -1))); data = null; await load(); }
      else if (b.id === "today") { day = data.today; data = null; await load(); }
      else if (b.dataset.edit) {
        const t = data.tasks.find((x) => x.code === b.dataset.edit);
        const r = await dialog({
          title: `Task ${t.code}`,
          body: `<label class="field"><span>Task</span><textarea name="title" rows="3" maxlength="400">${esc(t.title)}</textarea></label>
            <label class="field"><span>Assigned to (the row takes this colour)</span><select name="agent">${data.agents.map((a) => `<option value="${esc(a.agent)}"${a.agent === t.agent ? " selected" : ""}>${esc(a.name)}</option>`).join("")}</select></label>`,
          actions: [["Cancel", "close"], ["Save", "ok", "primary"]],
        });
        if (r.value !== "ok") return;
        const f = new FormData(r.el.querySelector("form"));
        await api("/api/checklist/task", { code: t.code, title: String(f.get("title") || ""), agent: String(f.get("agent") || "") });
        toast("Saved.");
        await load();
      }
    } catch (err) { toast(err.message, "bad"); }
  };

  // live: when anybody ticks or types (or an order changes), look again, but never while somebody is typing here
  known = (await api("/api/version")).v;
  await load();
  const timer = setInterval(async () => {
    if (!el.isConnected || location.hash.split("?")[0] !== "#checklist") return clearInterval(timer);
    if (document.hidden || busy() || document.querySelector("dialog[open]")) return;
    try {
      const { v } = await api("/api/version");
      if (v === known) return;
      known = v;
      await load();
    } catch { /* offline for a moment */ }
  }, 3000);
}
