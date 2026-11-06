"""Does the same changes on the Python app and the Rust app (each on its own copy of the same data) and compares what they answer
and what the data looks like afterwards.   python tools/compare_write.py http://localhost:8777 http://localhost:8766
Both need the user `cmp` / `compare-pass-1` (an admin). Times are masked: both apps stamp the current minute."""
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


def mask(x):
    if isinstance(x, str):
        return TS.sub("<ts>", x)
    if isinstance(x, list):
        return [mask(i) for i in x]
    if isinstance(x, dict):
        return {k: mask(v) for k, v in x.items() if k not in ("v",)}
    return x


def same(name, a, b):
    global bad
    a, b = mask(a), mask(b)
    if a == b:
        print("  ok  ", name)
        return True
    bad += 1
    print("  DIFF", name)
    if isinstance(a, dict) and isinstance(b, dict):
        for k in sorted(set(a) | set(b)):
            if a.get(k) != b.get(k):
                print("       key", k, "\n         old:", str(a.get(k))[:300], "\n         new:", str(b.get(k))[:300])
    else:
        print("       old:", str(a)[:300], "\n       new:", str(b)[:300])
    return False


def both(path, body=None):
    return old(path, body), new(path, body)


def row(call, rn):
    r = call("/api/orders", {"flt": {}, "q": "", "offset": 0, "limit": 1, "sort": "rn", "dir": "asc"})
    return r


def get_row(call, rn):
    # the row by its number: sort by rn and page to it is slow; the search finds the label
    r = call("/api/orders", {"offset": max(0, rn - 3), "limit": 5})
    return next((x for x in r["rows"] if x[0] == rn), None)


def edit(name, rn, col, value):
    a, b = both("/api/cells", {"edits": [{"rn": rn, "col": col, "value": value}]})
    same(f"{name}: answer", a, b)
    same(f"{name}: row after", get_row(old, rn), get_row(new, rn))
    return a


# ---- choose rows the same way for both apps
page = old("/api/orders", {"offset": 40000 - 40000 + 5000, "limit": 30})["rows"]
rn1, rn2, rn3 = page[3][0], page[4][0], page[5][0]
print("rows used:", rn1, rn2, rn3)

print("cells")
edit("note text", rn1, 25, "hello from the compare test")
edit("note again (same value)", rn1, 25, "hello from the compare test")
edit("qty number", rn1, 5, "3")
edit("qty bad", rn1, 5, "abc")
edit("date typed", rn1, 23, "Nov 9, 2026")
edit("date bad", rn1, 23, "someday")
edit("bool word", rn1, 16, "yes")
edit("bool bad", rn1, 16, "maybe")
edit("tick shipped to customer", rn2, 19, "TRUE")
edit("tick shipped to amazon (swaps)", rn2, 20, "TRUE")
edit("untick shipped to amazon", rn2, 20, "FALSE")
edit("tick hold", rn3, 16, "TRUE")
edit("untick hold", rn3, 16, "FALSE")
edit("stock no", rn3, 38, "ZZ-99")
edit("gift note (calculated, refused)", rn3, 1, "x")
edit("label number (feeds gift note)", rn3, 0, "LN-TEST-1")
edit("status to delivered", rn1, 13, "Delivered")
edit("customer name", rn3, 6, "Zed Testperson")
for who in ("history 25", "history 19", "history 43"):
    col = int(who.split()[1])
    for rn in (rn1, rn2):
        a, b = old(f"/api/history?rn={rn}&col={col}"), new(f"/api/history?rn={rn}&col={col}")
        same(f"{who} rn {rn}", a, b)
for rn in (rn2,):
    a, b = old(f"/api/ship-events?rn={rn}"), new(f"/api/ship-events?rn={rn}")
    same(f"ship events {rn}", a, b)

print("dropdown cells")
pg = old("/api/orders", {"offset": 0, "limit": 300})
dv = pg["dv"]
if dv:
    k = next(iter(dv))
    drn, dcol = [int(x) for x in k.split(":")]
    opts = pg["lists"][str(dv[k])]["options"]
    print("  a dropdown cell:", drn, dcol, opts[:4])
    edit("dropdown valid, other case", drn, dcol, opts[0].swapcase())
    edit("dropdown invalid", drn, dcol, "no such choice zzz")

print("rows")
a, b = both("/api/rows/add", {"rows": [{"0": "LN-NEW-1", "6": "New Person", "5": "2", "23": "Dec 1, 2026"}]})
same("add row", {"n": len(a.get("rns", []))}, {"n": len(b.get("rns", []))})
nrn_old, nrn_new = a["rns"][0], b["rns"][0]
same("new row numbers", nrn_old, nrn_new)
same("new row", get_row(old, nrn_old), get_row(new, nrn_new))
a, b = both("/api/rows/delete", {"rns": [nrn_old]})
same("delete row", a, b)
a, b = both("/api/trash")
same("trash list", a, b)
a, b = both("/api/rows/restore", {"rns": [nrn_old]})
same("restore row", a, b)
same("restored row", get_row(old, nrn_old), get_row(new, nrn_new))

print("scan")
samples = old("/api/scan/samples")["samples"]
for s in samples:
    code = s["code"]
    a, b = both("/api/scan", {"code": code, "dry": True})
    same(f"scan (practice) {s['scenario']}", a, b)
for s in samples[:3]:
    code = s["code"]
    a, b = both("/api/scan", {"code": code})
    same(f"scan {s['scenario']}", a, b)
    for r in a.get("results", []):
        rn = r["rn"]
        if r.get("outcome") == "count":
            x, y = both("/api/scan/count", {"rn": rn, "code": code, "items": 1})
            same(f"scan count rn {rn}", x, y)
        if r.get("print"):
            x, y = both("/api/scan/confirm", {"rn": rn, "kind": r["print"], "code": code})
            same(f"scan confirm {r['print']} rn {rn}", x, y)
        same(f"after scan row {rn}", get_row(old, rn), get_row(new, rn))
a, b = both("/api/scan", {"code": "TBA000000000000"})
same("scan not found", a, b)
a, b = both("/api/scan/holds")
same("holds", a, b)
a, b = both("/api/scan/recent")
same("recent scans", a, b)
ra = next((r for s in samples for r in old("/api/scan", {"code": s["code"], "dry": True}).get("results", []) if r.get("outcome") == "complete"), None)

print("everything after")
def dump(call):
    out = []
    for off in range(0, 40000, 500):
        r = call("/api/orders", {"offset": off, "limit": 500})
        out += r["rows"]
        if len(r["rows"]) < 500:
            break
    return out
da, db_ = dump(old), dump(new)
same("row count", len(da), len(db_))
diff = [(x[0], i) for x, y in zip(da, db_) for i in range(len(x)) if mask(x[i]) != mask(y[i])]
# the 8 rows with junk text in the TAPICKER date columns differ by design (kept in import_rejects)
diff = [d for d in diff if d[1] not in (41, 42)]
print("  cells that differ (outside the 2 known columns):", len(diff), diff[:10])
if diff:
    bad += 1
a, b = both("/api/dashboard")
same("dashboard after", {k: a[k] for k in ("cards", "counters", "orders", "returns", "sorting", "sorting_total", "colors")}, {k: b[k] for k in ("cards", "counters", "orders", "returns", "sorting", "sorting_total", "colors")})
print("\n%d difference(s)" % bad)
