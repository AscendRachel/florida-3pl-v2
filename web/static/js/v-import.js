// Import: upload the downloaded 3PL sheet (.xlsx or .csv), pick the orders tab, then replace or add to the orders.
import { $, api, brand, confirmBox, esc, fmt, toast } from "./util.js";

export async function mount(el) {
  const meta = await api("/api/meta");
  let up = null; // the uploaded file: {id, file, sheets, pick}

  const current = () => meta.total
    ? `<p><b>${fmt(meta.total)}</b> orders in the app${meta.imported ? ` · last import <b>${esc(meta.imported.file)}</b> (${esc(meta.imported.at.replace("T", " "))})` : ""}.</p>`
    : `<p class="muted">The app has no orders yet.</p>`;

  const draw = () => {
    el.innerHTML = `<div class="section-head"><div><div class="eyebrow">${esc(brand.name)}</div><h2>Import</h2>
      <p>Download the 3PL Google Sheet (File → Download → Microsoft Excel .xlsx) and upload it here. The app finds the orders tab and loads it into All Orders.</p></div></div>
      <div class="panel">${current()}</div>
      <div class="card">
        <h3>1. Choose the file</h3>
        <label class="dropzone" id="drop"><input type="file" id="file" accept=".xlsx,.csv" hidden>
          <b>${up ? esc(up.file) : "Choose an .xlsx or .csv file"}</b><span class="muted">${up ? "Choose a different file" : "or drag it here"}</span></label>
        <div class="or"><span>or paste a Google Sheets link</span></div>
        <div class="btnrow linkrow"><input id="link" class="grow" type="url" placeholder="https://docs.google.com/spreadsheets/d/…" autocomplete="off"><button class="btn" id="fetchLink">Get sheet</button></div>
        <p class="hint">The sheet must be shared as "Anyone with the link can view" (Share → General access).</p>
        <div id="step2">${up ? step2() : ""}</div>
      </div>
      <div id="progress"></div>
      <div id="result"></div>`;
  };

  const step2 = () => {
    const ok = up.sheets.filter((s) => s.ok);
    if (!ok.length) return `<div class="banner bad" style="margin-top:14px">None of the ${up.sheets.length} tab(s) in this file has a header row with a <b>Label Number</b> column, so there is nothing to import. Check that you downloaded the right sheet.</div>`;
    return `<h3 style="margin-top:20px">2. Pick the tab</h3>
      <label class="field" style="max-width:420px"><span>Tab that holds the orders</span>
        <select id="sheet">${up.sheets.map((s) => `<option value="${esc(s.name)}" ${s.name === up.pick ? "selected" : ""} ${s.ok ? "" : "disabled"}>${esc(s.name)}${s.ok ? ` — ${s.matched} columns recognised` : " — not an orders tab"}</option>`).join("")}</select></label>
      <h3 style="margin-top:20px">3. What to do with the orders already in the app</h3>
      <div class="radios">
        ${meta.total ? `<label class="check"><input type="radio" name="mode" value="dropdowns" checked> <span><b>Only update the dropdown lists</b><br><span class="muted">Keeps every order and every edit as they are, and loads just the dropdown choices from this file. Use it with the same sheet the orders came from.</span></span></label>` : ""}
        <label class="check"><input type="radio" name="mode" value="replace" ${meta.total ? "" : "checked"}> <span><b>Replace everything</b><br><span class="muted">Deletes the current orders and any edits made in the app, then loads this file.</span></span></label>
        <label class="check"><input type="radio" name="mode" value="append"> <span><b>Add to the existing orders</b><br><span class="muted">Keeps what is there and adds every row of this file to the bottom. Rows already in the app are not checked for duplicates.</span></span></label>
      </div>
      <div class="btnrow" style="margin-top:18px"><button class="btn primary" id="go">Import orders</button></div>`;
  };

  // ---- the progress bar: it shows what the step is doing, the percent when it is known, the time so far and a time left estimate.
  // The server reports the percent of a long step under a token (GET /api/progress); the browser reports the upload itself.
  let bar = null;
  const mb = (n) => (n / 1048576).toFixed(1);
  const clock = (s) => (s < 60 ? `${Math.round(s)} sec` : `${Math.floor(s / 60)} min ${Math.round(s % 60)} sec`);
  function startBar(title) {
    stopBar();
    const token = Math.random().toString(36).slice(2, 12) + Date.now().toString(36);
    const box = document.createElement("div");
    box.className = "imp-prog";
    box.innerHTML = `<div class="imp-prog-top"><b class="ip-title"></b><span class="ip-pct"></span></div><div class="ip-track"><i></i></div><div class="ip-text muted"></div>`;
    $("#progress").replaceChildren(box);
    const t0 = Date.now(), fill = box.querySelector(".ip-track i"), track = box.querySelector(".ip-track");
    let state = { pct: null, text: "Starting…" }, timer = null;
    const paintBar = () => {
      const s = (Date.now() - t0) / 1000, p = state.pct;
      box.querySelector(".ip-title").textContent = title;
      box.querySelector(".ip-pct").textContent = p == null ? "" : `${Math.min(100, Math.floor(p))}%`;
      track.classList.toggle("busy", p == null);
      fill.style.width = p == null ? "" : `${Math.max(2, Math.min(100, p))}%`;
      const left = p != null && p >= 8 && p < 100 && s > 3 ? ` · about ${clock(s * (100 - p) / p)} left` : "";
      box.querySelector(".ip-text").textContent = `${state.text} · ${clock(s)} so far${left}`;
    };
    const poll = async () => {
      try { const r = await api("/api/progress?token=" + token); if (r.text && !state.local) state = r; } catch { /* the next check tries again */ }
      paintBar();
    };
    timer = setInterval(poll, 500);
    const tick = setInterval(paintBar, 1000);
    paintBar();
    bar = {
      token,
      local: (pct, text) => { state = { pct, text, local: true }; paintBar(); },  // the browser's own step (sending the file)
      server: () => { state = { pct: null, text: "Waiting for the server…" }; },     // hand the bar back to the server's numbers
      stop: () => { clearInterval(timer); clearInterval(tick); box.remove(); },
    };
    return bar;
  }
  function stopBar() { bar?.stop(); bar = null; }

  // sends the file with XMLHttpRequest, because fetch cannot report how much of the upload has gone
  const sendFile = (f, token, onPart) => new Promise((resolve, reject) => {
    const x = new XMLHttpRequest();
    x.open("POST", "/api/upload");
    x.setRequestHeader("X-Filename", encodeURIComponent(f.name));
    x.setRequestHeader("X-Progress", token);
    x.upload.onprogress = (e) => { if (e.lengthComputable) onPart(e.loaded, e.total); };
    x.upload.onload = () => onPart(null, null);
    x.onerror = () => reject(new Error("The file could not be sent. Check the connection and try again."));
    x.onload = () => {
      if (x.status === 401) { location.href = "/login"; return reject(new Error("Please sign in.")); }
      let j = {};
      try { j = JSON.parse(x.responseText); } catch { /* not JSON */ }
      if (x.status >= 200 && x.status < 300) resolve(j); else reject(new Error(j.error || `Error ${x.status}`));
    };
    x.send(f);
  });

  const upload = async (f) => {
    if (!f) return;
    $("#result").innerHTML = "";
    const b = startBar(`Loading ${f.name}`);
    try {
      up = await sendFile(f, b.token, (done, total) => {
        if (done == null) b.server();
        else b.local(done / total * 100, `Sending the file… ${mb(done)} of ${mb(total)} MB`);
      });
      stopBar(); draw();
    } catch (e) {
      stopBar(); up = null; draw();
      $("#result").innerHTML = `<div class="banner bad">${esc(e.message)}</div>`;
    }
  };

  const fromLink = async () => {
    const url = $("#link").value.trim();
    if (!url) return toast("Paste the sheet's link first.", "bad");
    $("#result").innerHTML = "";
    const btn = $("#fetchLink");
    btn.disabled = true; btn.textContent = "Downloading…";
    const b = startBar("Getting the Google Sheet");
    try {
      up = await api("/api/upload-link", { url, token: b.token });
      stopBar(); draw();
    } catch (e) {
      stopBar();
      btn.disabled = false; btn.textContent = "Get sheet";
      $("#result").innerHTML = `<div class="banner bad">${esc(e.message)}</div>`;
    }
  };

  el.onchange = (e) => { if (e.target.id === "file") upload(e.target.files[0]); };
  el.onkeydown = (e) => { if (e.key === "Enter" && e.target.id === "link") fromLink(); };
  el.ondragover = (e) => { e.preventDefault(); $("#drop")?.classList.add("over"); };
  el.ondragleave = () => $("#drop")?.classList.remove("over");
  el.ondrop = (e) => { e.preventDefault(); $("#drop")?.classList.remove("over"); upload(e.dataTransfer.files[0]); };
  el.onclick = async (e) => {
    if (e.target.closest("#fetchLink")) return fromLink();
    if (!e.target.closest("#go")) return;
    const mode = el.querySelector("input[name=mode]:checked").value, sheet = $("#sheet").value;
    if (mode === "replace" && meta.total && !(await confirmBox("Replace all orders?", `This deletes the ${fmt(meta.total)} orders in the app, including edits you made here, and loads "${sheet}" from ${up.file}.`, "Replace"))) return;
    const btn = $("#go");
    btn.disabled = true; btn.textContent = "Importing…";
    const pb = startBar(mode === "dropdowns" ? "Updating the dropdown lists" : "Importing orders");
    try {
      const r = await api("/api/import", { id: up.id, sheet, mode, token: pb.token });
      stopBar();
      meta.total = (mode === "replace" ? 0 : meta.total) + r.rows;
      up = null; draw();
      $("#result").innerHTML = `<div class="banner good">${mode === "dropdowns" ? `<b>Dropdowns updated.</b> Your orders and edits were not touched.` : `<b>Imported ${fmt(r.rows)} orders.</b> ${r.matched} columns matched${r.skipped ? ` · ${fmt(r.skipped)} blank or counter rows skipped` : ""}.`}${r.dropdown_cells ? ` ${fmt(r.dropdown_lists)} dropdown lists kept on ${fmt(r.dropdown_cells)} cells.` : ""}${r.dropdowns_unsupported ? ` ${r.dropdowns_unsupported} dropdown(s) that read their choices from other cells were not imported.` : ""}
        <a class="btn small primary" href="#orders" style="margin-left:10px">Open All Orders</a></div>
        ${r.ignored.length ? `<div class="panel"><h3>Columns in the file that the app doesn't use</h3><p class="muted">${r.ignored.map(esc).join(", ")}</p></div>` : ""}
        ${r.missing.length ? `<div class="panel"><h3>Columns the app has that were not in the file (left empty)</h3><p class="muted">${r.missing.filter((m) => !m.includes("Softr")).map(esc).join(", ") || "none"}</p></div>` : ""}`;
      $("#result").scrollIntoView({ behavior: "smooth" });
    } catch (err) {
      stopBar();
      btn.disabled = false; btn.textContent = "Import orders";
      toast(err.message, "bad");
    }
  };
  draw();
}
