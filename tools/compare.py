"""Compares the Rust app's answers with the Python app's, on the same data.
   python tools/compare.py http://localhost:8777 http://localhost:8766
Both need the user `cmp` / `compare-pass-1`. Prints every difference it finds."""
import http.cookiejar, json, sys, urllib.request

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


def same(name, a, b, show=3):
    global bad
    if a == b:
        print("  ok  ", name)
        return True
    bad += 1
    print("  DIFF", name)
    if isinstance(a, dict) and isinstance(b, dict):
        for k in sorted(set(a) | set(b)):
            if a.get(k) != b.get(k):
                print("       key", k, "old:", str(a.get(k))[:150], "| new:", str(b.get(k))[:150])
                show -= 1
                if show <= 0:
                    break
    elif isinstance(a, list) and isinstance(b, list):
        print("       lengths", len(a), len(b))
        n = 0
        for i, (x, y) in enumerate(zip(a, b)):
            if x != y:
                print("       item", i, "old:", str(x)[:200], "| new:", str(y)[:200])
                n += 1
                if n >= show:
                    break
    else:
        print("       old:", str(a)[:200], "| new:", str(b)[:200])
    return False


def q(path, body=None, key=None):
    a, b = old(path, body), new(path, body)
    if key:
        a, b = a.get(key), b.get(key)
    return a, b


print("meta")
a, b = q("/api/meta")
same("columns", a["columns"], b["columns"])
same("cards", a["cards"], b["cards"])
same("total", a["total"], b["total"])
same("imported", a["imported"], b["imported"])
same("locks", a["locks"], b["locks"])

print("orders (first rows, formats)")
for name, body in [
    ("page 0", {"offset": 0, "limit": 100}),
    ("page 300", {"offset": 300, "limit": 100}),
    ("newest first", {"sort": "rn", "dir": "desc", "offset": 0, "limit": 50}),
    ("search", {"q": "nov", "offset": 0, "limit": 50}),
    ("search 2 words", {"q": "ln-5 greentek", "offset": 0, "limit": 50}),
    ("card delivered", {"card": "delivered", "offset": 0, "limit": 50}),
    ("card hold", {"card": "hold", "offset": 0, "limit": 50}),
    ("sort by col 5 desc", {"sort": "5", "dir": "desc", "offset": 0, "limit": 50}),
    ("sort by date col 23", {"sort": "23", "dir": "asc", "offset": 0, "limit": 50}),
    ("filter values", {"flt": {"24": {"v": {"mode": "in", "list": ["Delivered", "In Transit"]}}}, "offset": 0, "limit": 50}),
    ("filter not in", {"flt": {"26": {"v": {"mode": "out", "list": ["Citrus"]}}}, "offset": 0, "limit": 50}),
    ("cond contains", {"flt": {"3": {"c": {"op": "contains", "a": "mug"}}}, "offset": 0, "limit": 50}),
    ("cond empty", {"flt": {"4": {"c": {"op": "empty"}}}, "offset": 0, "limit": 50}),
    ("cond num gt", {"flt": {"5": {"c": {"op": "gt", "a": "2"}}}, "offset": 0, "limit": 50}),
    ("cond date eq", {"flt": {"23": {"c": {"op": "eq", "a": "2026-11-02"}}}, "offset": 0, "limit": 50}),
    ("cond date between", {"flt": {"23": {"c": {"op": "between", "a": "2026-11-01", "b": "2026-11-03"}}}, "offset": 0, "limit": 50}),
    ("color filter", {"flt": {"3": {"k": {"list": ["#ffff00"]}}}, "offset": 0, "limit": 50}),
    ("color sort", {"sort": "color:3:#ff0000", "offset": 0, "limit": 50}),
    ("dup filter", {"flt": {"22": {"k": {"list": ["dup"]}}}, "offset": 0, "limit": 50}),
]:
    a, b = old("/api/orders", body), new("/api/orders", body)
    same(f"{name}: total", a.get("total"), b.get("total"))
    same(f"{name}: rows", a.get("rows"), b.get("rows"))
    if name == "page 0":
        same("page 0: dv", a.get("dv"), b.get("dv"))
        same("page 0: lists", a.get("lists"), b.get("lists"))
a, b = old("/api/orders", {"rns_only": True, "limit": 20, "offset": 5}), new("/api/orders", {"rns_only": True, "limit": 20, "offset": 5})
same("rns_only", a, b)

print("filter menus")
for col in (2, 3, 5, 13, 19, 23, 24, 26, 44, 51):
    body = {"col": col}
    a, b = old("/api/values", body), new("/api/values", body)
    same(f"values col {col}", a, b)
for col in (3, 13, 22, 24, 43, 23, 42):
    a, b = old("/api/colors", {"col": col}), new("/api/colors", {"col": col})
    same(f"colors col {col}", a, b)
a, b = old("/api/values", {"col": 23, "s": "nov 4"}), new("/api/values", {"col": 23, "s": "nov 4"})
same("values date search 'nov 4'", a, b)

print("lookups")
a, b = old("/api/duplicates"), new("/api/duplicates")
same("duplicates", {k: sorted(v) for k, v in a["values"].items()}, {k: sorted(v) for k, v in b["values"].items()})
a, b = old("/api/views"), new("/api/views")
same("views", a, b)
a, b = old("/api/history?rn=3&col=0"), new("/api/history?rn=3&col=0")
same("history", a, b)

print("\n%d difference(s)" % bad)
