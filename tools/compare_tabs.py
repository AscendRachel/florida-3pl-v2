"""Compares the Rust app with the Python app on Returns, Materials, Kohl's, the Work Log chat, branding and the Admin month table.
   python tools/compare_tabs.py http://localhost:8777 http://localhost:8766
Same setup as compare_write.py (the user `cmp` / `compare-pass-1` on both)."""
import http.cookiejar, json, re, sys, urllib.request

OLD, NEW = sys.argv[1], sys.argv[2]


def client(base):
    jar = http.cookiejar.CookieJar()
    op = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))

    def call(path, body=None):
        data = json.dumps(body).encode() if body is not None else None
        req = urllib.request.Request(base + path, data=data, headers={"Content-Type": "application/json"} if data else {})
        try:
            with op.open(req) as r:
                return json.loads(r.read() or b"{}")
        except urllib.error.HTTPError as e:
            return {"__status": e.code, **json.loads(e.read() or b"{}")}
    call("/api/login", {"username": "cmp", "password": "compare-pass-1"})
    return call


old, new = client(OLD), client(NEW)
bad = 0
TS = re.compile(r"\d{4}-\d\d-\d\d \d\d:\d\d(:\d\d)?")


def mask(x, drop=()):
    if isinstance(x, str):
        return TS.sub("<ts>", x)
    if isinstance(x, list):
        return [mask(i, drop) for i in x]
    if isinstance(x, dict):
        return {k: mask(v, drop) for k, v in x.items() if k not in ("v",) + tuple(drop)}
    return x


def same(name, a, b, drop=()):
    global bad
    a, b = mask(a, drop), mask(b, drop)
    if a == b:
        print("  ok  ", name)
        return True
    bad += 1
    print("  DIFF", name)
    if isinstance(a, dict) and isinstance(b, dict):
        for k in sorted(set(a) | set(b)):
            if a.get(k) != b.get(k):
                print("       key", k, "\n         old:", str(a.get(k))[:400], "\n         new:", str(b.get(k))[:400])
    elif isinstance(a, list) and isinstance(b, list):
        print("       lengths", len(a), len(b))
        for i, (x, y) in enumerate(zip(a, b)):
            if x != y:
                print("       item", i, "\n         old:", str(x)[:300], "\n         new:", str(y)[:300])
                break
    else:
        print("       old:", str(a)[:300], "\n       new:", str(b)[:300])
    return False


def both(path, body=None):
    return old(path, body), new(path, body)


print("reading")
a, b = both("/api/materials")
same("materials", a, b)
a, b = both("/api/kohls")
same("kohls", a, b)
a, b = both("/api/returns")
same("returns rows", a["rows"], b["rows"])
same("returns locks", a["locks"], b["locks"])
same("returns labels", a["labels"], b["labels"])
same("returns status settings", a["status"]["settings"], b["status"]["settings"])
same("returns status last", a["status"]["last"], b["status"]["last"])
same("returns status log", a["status"]["log"], b["status"]["log"])
a, b = both("/api/chat")
same("chat today (empty)", {"items": a["items"]}, {"items": b["items"]})
a, b = both("/api/worklog")
same("worklog", {k: a[k] for k in ("items", "total", "people")}, {k: b[k] for k in ("items", "total", "people")})
# (the Admin tab's month table is gone: it is now the 3PL Daily Update, which v1 does not have)
a, b = both("/api/branding")
same("branding", a, b)

print("materials")
a, b = both("/api/materials/add", {"values": {"name": "Test mailer", "unit": "Pack", "current": "5", "alert": "10", "minimum": "2", "notes": "n1"}})
same("add", a, b)
mid = a["row"]["id"]
for name, body in [("edit current to 0", {"id": mid, "field": "current", "value": "0"}), ("edit current 20", {"id": mid, "field": "current", "value": "20"}),
                   ("edit bad number", {"id": mid, "field": "alert", "value": "abc"}), ("edit notes", {"id": mid, "field": "notes", "value": "second note"}),
                   ("edit name", {"id": mid, "field": "name", "value": "Renamed mailer"}), ("edit bad column", {"id": mid, "field": "status", "value": "x"}),
                   ("edit missing row", {"id": 99999, "field": "name", "value": "x"})]:
    a, b = both("/api/materials/edit", body)
    same(name, a, b)
a, b = both("/api/materials/bulk", {"edits": [{"id": mid, "field": "pack", "value": "12"}, {"id": mid, "field": "minimum", "value": "30"}], "new": [{"name": "Bulk one", "current": "1"}, {"name": "Bulk two", "unit": "Roll"}]})
same("bulk", a, b)
a, b = both("/api/materials")
same("materials after", a, b)
a, b = both("/api/materials/delete", {"id": mid})
same("delete", a, b)
a, b = both("/api/materials")
same("materials after delete", a, b)

print("kohls")
a, b = both("/api/kohls/add", {"values": {"store": "KOHLS JESS", "stock_no": "K-1", "sku": "S1", "order_number": "O1", "product": "Thing", "qty": "3", "good": "yes", "notes": "kn"}})
same("add", a, b)
kid = a["row"]["id"]
for name, body in [("edit good", {"id": kid, "field": "good", "value": False}), ("edit used true", {"id": kid, "field": "used", "value": True}),
                   ("edit stock_no (a sort)", {"id": kid, "field": "stock_no", "value": "K-2"}), ("edit stock_no same", {"id": kid, "field": "stock_no", "value": "K-2"}),
                   ("edit qty", {"id": kid, "field": "qty", "value": "7"}), ("edit bad column", {"id": kid, "field": "nope", "value": "x"})]:
    a, b = both("/api/kohls/edit", body)
    same(name, a, b)
a, b = both("/api/kohls/bulk", {"edits": [{"id": kid, "field": "stock_no", "value": "K-3"}, {"id": kid, "field": "good", "value": "TRUE"}], "new": [{"store": "KOHLS ACPJR", "stock_no": "K-9"}]})
same("bulk", a, b)
a, b = both("/api/kohls")
same("kohls after", a, b)
a, b = both("/api/dashboard")
same("dashboard sorting after kohls", {"s": a["sorting"], "t": a["sorting_total"]}, {"s": b["sorting"], "t": b["sorting_total"]})
a, b = both("/api/kohls/delete", {"id": kid})
same("delete", a, b)

print("returns")
rows = old("/api/returns")["rows"]
r1 = next(r for r in rows if r["shipped"] == "FALSE" and not r["removed"])
rid = r1["rid"]
for name, body in [("tick shipped", {"rid": rid, "field": "shipped", "value": True}), ("untick shipped", {"rid": rid, "field": "shipped", "value": False}),
                   ("wh notes", {"rid": rid, "field": "wh_notes", "value": "warehouse says hi"}), ("override qty", {"rid": rid, "field": "qty", "value": "9"}),
                   ("override qty bad", {"rid": rid, "field": "qty", "value": "nine"}), ("override due date", {"rid": rid, "field": "due_date", "value": "Dec 5, 2026"}),
                   ("override refunded yes", {"rid": rid, "field": "refunded", "value": "yes"}), ("override notes", {"rid": rid, "field": "notes", "value": "typed over"}),
                   ("type back the sheet value", {"rid": rid, "field": "qty", "value": r1["qty"]}), ("revert notes", {"rid": rid, "field": "notes", "value": "", "revert": True}),
                   ("processed_by", {"rid": rid, "field": "processed_by", "value": "Somebody"}), ("bad column", {"rid": rid, "field": "po_x", "value": "z"}),
                   ("missing return", {"rid": "NOPE-1", "field": "wh_notes", "value": "x"})]:
    a, b = both("/api/returns/edit", body)
    same(name, a, b)
a, b = both("/api/returns/labels", {"k": "po", "label": "Order No"})
same("rename a returns column", a, b)
a, b = both("/api/returns/labels", {"k": "po", "label": ""})
same("clear the rename", a, b)
a, b = both("/api/returns/settings", {"interval_min": 15, "warehouse": "FLORIDA"})
same("settings save", {"s": a["status"]["settings"]}, {"s": b["status"]["settings"]})
a, b = both("/api/returns/settings", {"interval_min": 7})
same("settings bad interval", a, b)
a, b = both("/api/returns")
same("returns after", a["rows"], b["rows"])

print("locks of returns")
a, b = both("/api/locks/set", {"sheet": "returns", "kind": "col", "targets": ["notes"], "mode": "nobody", "note": "frozen"})
same("protect a returns column", a, b)
a, b = both("/api/returns")
same("returns locks after", a["locks"], b["locks"])
a, b = both("/api/locks/set", {"sheet": "returns", "kind": "col", "targets": ["notes"], "mode": ""})
same("unprotect", a, b)

print("chat and branding")
a, b = both("/api/chat/send", {"text": "Sorting stock 93"})
same("send", a, b)
a, b = both("/api/chat/send", {"text": "   "})
same("send empty", a, b)
a, b = both("/api/chat/send", {"text": "x" * 1001})
same("send too long", a, b)
a, b = both("/api/chat")
same("chat today", {"items": a["items"]}, {"items": b["items"]})
a, b = both("/api/worklog?q=stock")
same("worklog search", {k: a[k] for k in ("items", "total", "people")}, {k: b[k] for k in ("items", "total", "people")})
today = old("/api/chat")["today"]
a, b = both(f"/api/worklog?date={today}")
same("worklog by date", {k: a[k] for k in ("items", "total")}, {k: b[k] for k in ("items", "total")})
a, b = both("/api/branding", {"name": "  Test  Warehouse ", "initials": "TW", "color": "#336699"})
same("branding save", a, b)
a, b = both("/api/branding", {"name": "x", "color": "blue"})
same("branding bad colour", a, b)
a, b = both("/api/branding")
same("branding after", a, b)
a, b = both("/api/branding", {"name": "Florida 3PL", "initials": "3PL", "color": "#26a63f"})

print("\n%d difference(s)" % bad)
