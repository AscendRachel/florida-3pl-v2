// Team: who can sign in (admins only).
import { applyBranding } from "./branding.js";
import { api, brand, confirmBox, dialog, esc, roleNames, toast } from "./util.js";

const TAB_LIST = [["dashboard", "Dashboard"], ["scan", "Scan"], ["cards", "Order Cards"], ["orders", "All Orders"], ["returns", "Returns"], ["materials", "Materials"], ["checklist", "Team Daily Check List"], ["timeframe", "Source Time Frame"],
  ["worklog", "Work Log"], ["amazon", "Amazon Orders"], ["import", "Import"], ["team", "Team"], ["admin", "Admin"]];
const ADMIN_TABS = ["worklog", "amazon", "import", "team", "admin"]; // only when ticked: a member with no list does not get these
const tabBoxes = (on) => `<fieldset class="tabpick" style="border:1px solid var(--line,#ccc);border-radius:8px;padding:8px 12px;margin:0 0 10px"><legend>Tabs this person can see</legend>
  <div style="display:grid;grid-template-columns:1fr 1fr;gap:4px 14px">${TAB_LIST.map(([k, l]) => `<label class="check"><input type="checkbox" name="tab" value="${k}" ${on ? (on.includes(k) ? "checked" : "") : ADMIN_TABS.includes(k) ? "" : "checked"}> ${l}</label>`).join("")}</div></fieldset>`;
let ROLES = []; // [{name, label, builtin, tabs, people}] from the server
const roleLabel = (k) => ROLES.find((r) => r.name === k)?.label || k || "Member";
const roleOptions = (cur) => ROLES.map((r) => `<option value="${esc(r.name)}"${r.name === cur ? " selected" : ""}>${esc(r.label)}</option>`).join("");
const tabName = (k) => (TAB_LIST.find((t) => t[0] === k) || [k, k])[1];
// tick the boxes of a role's default tabs (the usual tabs when the role has none)
const tickRoleTabs = (root, role) => root.querySelectorAll('input[name="tab"]').forEach((b) => { b.checked = role?.tabs ? role.tabs.includes(b.value) : !ADMIN_TABS.includes(b.value); });
const pickedTabs = (form) => new FormData(form).getAll("tab");

export async function mount(el, params, ctx) {
  const draw = async () => {
    const got = await api("/api/team");
    const rows = got.items;
    ROLES = got.roles || [];
    for (const k of Object.keys(roleNames)) delete roleNames[k];
    ROLES.forEach((r) => (roleNames[r.name] = r.label));
    const me = ctx.me.username;
    el.innerHTML = `<div class="section-head"><div><div class="eyebrow">${esc(brand.name)}</div><h2>Team</h2>
        <p>Everyone who can sign in. Everyone except admins sees only the tabs you give them (the Tabs button), whatever their role. <b>Admins</b> can also import orders, change dropdown choices and sync settings, and manage the team.</p></div>
        <div class="btnrow"><button class="btn" id="brandBtn">App name &amp; logo</button><button class="btn primary" id="add">Add team member</button></div></div>
      <div class="panel"><div class="tablewrap"><table style="min-width:620px"><thead><tr><th>Name</th><th>Username</th><th>Role</th><th>Tabs</th><th>Added</th><th></th></tr></thead><tbody>
      ${rows.map((u) => `<tr><td><b>${esc(u.name)}</b>${u.username === me ? ` <span class="muted">(you)</span>` : ""}</td><td class="mono">${esc(u.username)}</td>
        <td>${u.is_super ? '<span class="pill s-ok" title="The first admin account. Nobody else can change or remove it.">Super admin</span>' : u.role === "admin" ? '<span class="pill s-ok">Admin</span>' : esc(roleLabel(u.role))}</td>
        <td class="muted">${u.role === "admin" ? "All" : !u.tabs ? "All except the admin tabs" : esc(u.tabs.map(tabName).join(", "))}</td><td class="muted">${esc((u.created_at || "").slice(0, 10))}</td>
        <td class="num">${u.role === "admin" ? "" : `<button class="btn small" data-tabs="${esc(u.username)}" data-have="${esc((u.tabs || []).join(","))}">Tabs</button> `}${u.is_super && u.username !== me ? "" : `<button class="btn small" data-reset="${esc(u.username)}">Reset password</button>`}
          ${u.username === me || u.is_super || (u.role === "admin" && !ctx.me.is_super) ? "" : `<button class="btn small" data-role="${esc(u.username)}" data-cur="${esc(u.role)}">Role</button> <button class="btn small danger" data-remove="${esc(u.username)}">Remove</button>`}</td></tr>`).join("")}
      </tbody></table></div></div>
      ${ctx.isAdmin() ? `<div class="panel" style="margin-top:16px"><div class="btnrow" style="justify-content:space-between"><h3>Roles</h3><button class="btn small primary" id="addRole">Add role</button></div>
        <p class="hint" style="margin-top:0">A role is a name you can give to people (it shows next to their name). It can carry a default set of tabs that is offered when you give the role to someone. Any role except Admin can be removed once nobody has it. Admin accounts are removed only by the super admin (the first admin account).</p>
        <div class="tablewrap"><table style="min-width:520px"><thead><tr><th>Role</th><th>People</th><th>Default tabs</th><th></th></tr></thead><tbody>
        ${ROLES.map((r) => `<tr><td><b>${esc(r.label)}</b></td><td class="num">${r.people}</td><td class="muted">${r.name === "admin" ? "All" : r.tabs ? esc(r.tabs.map(tabName).join(", ")) : "None set"}</td>
          <td class="num"><button class="btn small" data-editrole="${esc(r.name)}">Edit</button>${r.name === "admin" ? "" : ` <button class="btn small danger" data-delrole="${esc(r.name)}">Remove</button>`}</td></tr>`).join("")}
        </tbody></table></div></div>` : ""}
      <p class="hint">Whoever signs in is recorded by name on every scan, tick and edit they make.</p>`;
  };
  el.onclick = async (e) => {
    const t = e.target;
    try {
      if (t.closest("#add")) {
        const r = await dialog({
          title: "Add team member",
          body: `<label class="field"><span>Name</span><input name="name" required></label><label class="field"><span>Username</span><input name="username" required autocapitalize="off" autocomplete="off"></label>
                 <label class="field"><span>Temporary password (10+ characters)</span><input name="password" minlength="10" required autocomplete="new-password"></label>
                 <label class="field"><span>Role</span><select name="role">${roleOptions("member")}</select></label>
                 ${tabBoxes(ROLES.find((r) => r.name === "member")?.tabs || null)}<p class="hint">Picking a role ticks its default tabs; you can still change them. Admins always see every tab.</p>`,
          actions: [["Cancel", "close"], ["Add", "ok", "primary"]],
          onOpen: (dlg) => dlg.querySelector('select[name="role"]').addEventListener("change", (ev) => tickRoleTabs(dlg, ROLES.find((r) => r.name === ev.target.value))),
        });
        if (r.value === "ok") {
          const form = r.el.querySelector("form"), body = Object.fromEntries(new FormData(form));
          delete body.tab;
          if (body.role !== "admin") body.tabs = pickedTabs(form);
          await api("/api/team/add", body);
          toast("Team member added. Share the password privately.");
          draw();
        }
      }
      if (t.closest("#brandBtn")) {
        const r = await dialog({
          title: "App name & logo",
          body: `<label class="field"><span>App name (shown at the top and in the browser tab)</span><input name="name" value="${esc(brand.name)}" maxlength="40" required></label>
            <div class="op-edit"><label class="field"><span>Initials (up to 3 letters, used when there is no logo)</span><input name="initials" value="${esc(brand.initials)}" maxlength="3"></label>
            <label class="field"><span>Colour</span><input type="color" name="color" value="${esc(brand.color)}" style="height:38px;width:100%;padding:2px"></label></div>
            <label class="field"><span>Logo (PNG, JPG, GIF or WebP, up to 1.5 MB; square works best)</span><input type="file" name="logo" accept="image/png,image/jpeg,image/gif,image/webp"></label>
            ${brand.logo ? `<label class="check"><input type="checkbox" name="remove_logo"> Remove the current logo (<img alt="" src="/branding/logo?v=${brand.v}" style="height:20px;vertical-align:middle">) and show the initials</label>` : ""}`,
          actions: [["Cancel", "close"], ["Save", "ok", "primary"]],
        });
        if (r.value === "ok") {
          const f = new FormData(r.el.querySelector("form")), file = f.get("logo");
          const body = { name: f.get("name"), initials: f.get("initials"), color: f.get("color"), remove_logo: !!f.get("remove_logo") };
          if (file && file.size) {
            if (file.size > 1_500_000) throw new Error("The logo can be up to 1.5 MB.");
            body.logo = await new Promise((ok, no) => { const rd = new FileReader(); rd.onload = () => ok(rd.result); rd.onerror = () => no(new Error("That image could not be read.")); rd.readAsDataURL(file); });
          }
          applyBranding(await api("/api/branding", body));
          toast("Saved. Everyone sees the new name and logo the next time their page loads.");
          draw();
        }
      }
      const rs = t.closest("[data-reset]");
      if (rs) {
        const r = await dialog({ title: "Reset password for " + rs.dataset.reset, body: `<label class="field"><span>New password (10+ characters)</span><input name="password" minlength="10" required autocomplete="new-password"></label><p class="hint">They are signed out everywhere and use the new password next time.</p>`, actions: [["Cancel", "close"], ["Reset", "ok", "primary"]] });
        if (r.value === "ok") {
          await api("/api/team/reset", { username: rs.dataset.reset, password: new FormData(r.el.querySelector("form")).get("password") });
          toast("Password reset.");
        }
      }
      const tb = t.closest("[data-tabs]");
      if (tb) {
        const r = await dialog({ title: "Tabs for " + tb.dataset.tabs, body: `${tabBoxes(tb.dataset.have ? tb.dataset.have.split(",") : null)}<p class="hint">They only see the ticked tabs. Takes effect the next time their page loads.</p>`, actions: [["Cancel", "close"], ["Save", "ok", "primary"]] });
        if (r.value === "ok") {
          await api("/api/team/tabs", { username: tb.dataset.tabs, tabs: pickedTabs(r.el.querySelector("form")) });
          toast("Tabs saved.");
          draw();
        }
      }
      const ro = t.closest("[data-role]");
      if (ro) {
        const r = await dialog({ title: "Role for " + ro.dataset.role, body: `<label class="field"><span>Role</span><select name="role">${roleOptions(ro.dataset.cur)}</select></label><label class="check" id="roleTabsRow" hidden><input type="checkbox" name="applytabs" checked> Also set their tabs to this role's default tabs</label>
          <p class="hint">Admins see every tab and can manage the team. Everyone else only sees the tabs you give them (the Tabs button).</p>`, actions: [["Cancel", "close"], ["Save", "ok", "primary"]],
          onOpen: (dlg) => {
            const sel = dlg.querySelector('select[name="role"]'), row = dlg.querySelector("#roleTabsRow");
            const sync = () => { const rl = ROLES.find((x) => x.name === sel.value); row.hidden = !(rl && rl.tabs && rl.name !== "admin" && rl.name !== ro.dataset.cur); };
            sel.addEventListener("change", sync); sync();
          } });
        if (r.value === "ok") {
          const f = new FormData(r.el.querySelector("form")), to = f.get("role"), rl = ROLES.find((x) => x.name === to);
          await api("/api/team/role", { username: ro.dataset.role, role: to });
          if (f.get("applytabs") && rl?.tabs && to !== "admin") await api("/api/team/tabs", { username: ro.dataset.role, tabs: rl.tabs });
          toast(`${ro.dataset.role} is now ${roleLabel(to)}.`);
          draw();
        }
      }
      const roleDialog = async (r) => {
        const res = await dialog({
          title: r ? `Edit role: ${r.label}` : "Add role",
          body: `<label class="field"><span>Role name</span><input name="label" value="${esc(r?.label || "")}" maxlength="40" required autocomplete="off"></label>
            ${r?.name === "admin" ? `<p class="hint">Admins always see every tab.</p>` : `${tabBoxes(r ? (r.tabs || null) : null).replace("Tabs this person can see", "Default tabs for this role")}<p class="hint">These are offered when you give someone this role. Each person's own tabs can still be changed with the Tabs button.</p>`}`,
          actions: [["Cancel", "close"], ["Save", "ok", "primary"]],
          enterSaves: true,
        });
        if (res.value !== "ok") return;
        const form = res.el.querySelector("form");
        const label = new FormData(form).get("label");
        const tabs = r?.name === "admin" ? [] : pickedTabs(form);
        await api(r ? "/api/roles/edit" : "/api/roles/add", { name: r?.name, label, tabs });
        toast(r ? "Role saved." : "Role added. You can give it to people with the Role button.");
        draw();
      };
      if (t.closest("#addRole")) await roleDialog(null);
      const er = t.closest("[data-editrole]");
      if (er) await roleDialog(ROLES.find((x) => x.name === er.dataset.editrole));
      const dr = t.closest("[data-delrole]");
      if (dr && (await confirmBox("Remove the role " + roleLabel(dr.dataset.delrole) + "?", "Only a role nobody has can be removed. You can add it again later.", "Remove"))) {
        await api("/api/roles/remove", { name: dr.dataset.delrole });
        toast("Role removed.");
        draw();
      }
      const rm = t.closest("[data-remove]");
      if (rm && (await confirmBox("Remove " + rm.dataset.remove + "?", "They will not be able to sign in any more. What they scanned and edited stays in the records.", "Remove"))) {
        await api("/api/team/remove", { username: rm.dataset.remove });
        draw();
      }
    } catch (err) { toast(err.message, "bad"); }
  };
  await draw();
}
