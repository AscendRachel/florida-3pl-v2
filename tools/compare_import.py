"""Compares the Amazon import and the Import page (xlsx and csv) of the Rust app with the Python app's.
   python tools/compare_import.py http://localhost:8777 http://localhost:8766 <the Amazon orders .csv>"""
import http.cookiejar, io, json, re, sys, urllib.parse, urllib.request, zipfile

OLD, NEW, AMAZON = sys.argv[1], sys.argv[2], sys.argv[3]


def client(base):
    jar = http.cookiejar.CookieJar()
    op = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))

    def call(path, body=None, raw=None, name=None):
        if raw is not None:
            req = urllib.request.Request(base + path, data=raw, headers={"X-Filename": urllib.parse.quote(name or "file"), "Content-Type": "application/octet-stream"})
        else:
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


def dump(call):
    out = []
    for off in range(0, 60000, 500):
        r = call("/api/orders", {"offset": off, "limit": 500})
        out += r["rows"]
        if len(r["rows"]) < 500:
            break
    return out


def compare_dumps(label):
    global bad
    da, db = dump(old), dump(new)
    same(f"{label}: row count", len(da), len(db))
    diff = [(x[0], i) for x, y in zip(da, db) for i in range(len(x)) if mask(x[i]) != mask(y[i])]
    diff = [d for d in diff if d[1] not in (41, 42)]  # the junk text in the TAPICKER dates differs by design
    print(f"  {label}: cells that differ:", len(diff), diff[:8])
    if diff:
        bad += 1


print("Amazon import")
data = open(AMAZON, "rb").read()
a = old("/api/amazon/preview", raw=data, name="orders.csv")
b = new("/api/amazon/preview", raw=data, name="orders.csv")
tok_a, tok_b = a.pop("token"), b.pop("token")
for k in ("lines", "orders", "matched_rows", "skipped_done", "tracking_added", "status_changed", "dates_changed", "brands_added", "eta_column", "eta_found", "moves", "unmatched_total", "changed_total", "unsure", "file"):
    same(f"preview {k}", a.get(k), b.get(k))
same("preview unmatched_orders", a.get("unmatched_orders"), b.get("unmatched_orders"))
same("preview shown (first 300 changes)", a.get("shown"), b.get("shown"))
x = old("/api/amazon/apply", {"token": tok_a})
y = new("/api/amazon/apply", {"token": tok_b})
same("apply", x, y)
compare_dumps("after the Amazon import")
same("apply again (token gone)", old("/api/amazon/apply", {"token": tok_a}), new("/api/amazon/apply", {"token": tok_b}))
bad_file = old("/api/amazon/preview", raw=b"a,b\n1,2\n", name="x.csv"), new("/api/amazon/preview", raw=b"a,b\n1,2\n", name="x.csv")
same("not an Amazon file", *bad_file)


def xlsx(rows, dropdown=None):
    """A small workbook: sheet 'March' (rows of text / numbers / dates / booleans), and a list validation."""
    def cell(r, c, v):
        ref = f"{chr(65 + c)}{r}"
        if isinstance(v, tuple) and v[0] == "date":
            return f'<c r="{ref}" s="1"><v>{v[1]}</v></c>'
        if isinstance(v, bool):
            return f'<c r="{ref}" t="b"><v>{int(v)}</v></c>'
        if isinstance(v, (int, float)):
            return f'<c r="{ref}"><v>{v}</v></c>'
        if isinstance(v, tuple) and v[0] == "sci":
            return f'<c r="{ref}"><v>{v[1]}</v></c>'
        return f'<c r="{ref}" t="inlineStr"><is><t>{str(v).replace("&", "&amp;").replace("<", "&lt;")}</t></is></c>'
    body = "".join(f'<row r="{i}">' + "".join(cell(i, c, v) for c, v in enumerate(r) if v is not None) + "</row>" for i, r in enumerate(rows, start=1))
    dv = f'<dataValidations count="1"><dataValidation type="list" sqref="{dropdown[0]}" showErrorMessage="1"><formula1>"{dropdown[1]}"</formula1></dataValidation></dataValidations>' if dropdown else ""
    sheet = f'<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{body}</sheetData>{dv}</worksheet>'
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("[Content_Types].xml", '<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>')
        z.writestr("xl/workbook.xml", '<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Notes" sheetId="1" r:id="rId2"/><sheet name="March" sheetId="2" r:id="rId1"/></sheets></workbook>')
        z.writestr("xl/_rels/workbook.xml.rels", '<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Target="worksheets/sheet2.xml"/></Relationships>')
        z.writestr("xl/styles.xml", '<?xml version="1.0"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><cellXfs count="2"><xf numFmtId="0"/><xf numFmtId="14"/></cellXfs></styleSheet>')
        z.writestr("xl/worksheets/sheet1.xml", sheet)
        z.writestr("xl/worksheets/sheet2.xml", '<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>nothing here</t></is></c></row></sheetData></worksheet>')
    return buf.getvalue()


header = ["Label Number", "Store Order No.", "Product Name", "Brand", "QTY", "Customer Name", "Delivery Date", "Shipped to Customer", "Tracking Number To Customer", "Source Order Status", "Notes", "Extra Column"]
rows = [header,
        ["LN-IMP-1", "PO-1", "Mug & cup", "Acme", 2, "Ann Import", ("date", 46327.5), True, ("sci", "5.42923753189E11"), "In Transit", "first", "x"],
        ["LN-IMP-2", "PO-2", "Plate <big>", "", 1, "Bob Import", ("date", 46330), False, "9400111899223", "Delivered", None, "y"],
        [None, None, None, None],
        ["Counters", None],
        ["LN-IMP-3", "PO-3", "Bowl", "Zed", 3.0, "Cy Import", None, None, None, "Unfulfilled", "line1\nline2", "z"]]
book = xlsx(rows, ("K2:K4", "first,second"))
print("Import page: upload")
a = old("/api/upload", raw=book, name="sheet.xlsx")
b = new("/api/upload", raw=book, name="sheet.xlsx")
ida, idb = a.pop("id"), b.pop("id")
same("upload xlsx", a, b)
a2 = old("/api/upload", raw=b"x", name="notes.txt"), new("/api/upload", raw=b"x", name="notes.txt")
same("upload wrong type", *a2)
a3 = old("/api/upload", raw=b"this is not a zip", name="bad.xlsx"), new("/api/upload", raw=b"this is not a zip", name="bad.xlsx")
same("upload broken xlsx (message text may differ)", {k: v for k, v in a3[0].items() if k != "error"}, {k: v for k, v in a3[1].items() if k != "error"})
csv_bytes = ("\n".join([",".join(map(str, ["" if v is None else v if not isinstance(v, tuple) else "2026-11-05" for v in r])) for r in rows[:4]])).encode()
c = old("/api/upload", raw=csv_bytes, name="march.csv"), new("/api/upload", raw=csv_bytes, name="march.csv")
cid_a, cid_b = c[0].pop("id"), c[1].pop("id")
same("upload csv", *c)

print("Import page: import")
r = old("/api/import", {"id": ida, "sheet": "March", "mode": "append"}), new("/api/import", {"id": idb, "sheet": "March", "mode": "append"})
same("append xlsx", *r)
compare_dumps("after appending")
for rn in range(0, 0):
    pass
r = old("/api/import", {"id": ida, "sheet": "March", "mode": "append"}), new("/api/import", {"id": idb, "sheet": "March", "mode": "append"})
same("import the same upload twice (it is gone)", *r)
r = old("/api/import", {"id": cid_a, "sheet": "CSV file", "mode": "append"}), new("/api/import", {"id": cid_b, "sheet": "CSV file", "mode": "append"})
same("append csv", *r)
compare_dumps("after appending the csv")
a, b = old("/api/upload", raw=book, name="sheet.xlsx"), new("/api/upload", raw=book, name="sheet.xlsx")
r = old("/api/import", {"id": a["id"], "sheet": "March", "mode": "dropdowns"}), new("/api/import", {"id": b["id"], "sheet": "March", "mode": "dropdowns"})
same("dropdowns only (a file that does not line up)", *r)
a, b = old("/api/upload", raw=book, name="sheet.xlsx"), new("/api/upload", raw=book, name="sheet.xlsx")
r = old("/api/import", {"id": a["id"], "sheet": "March", "mode": "replace"}), new("/api/import", {"id": b["id"], "sheet": "March", "mode": "replace"})
same("replace everything", *r)
compare_dumps("after replacing")
same("meta after", {k: v for k, v in old("/api/meta").items() if k in ("cards", "total")}, {k: v for k, v in new("/api/meta").items() if k in ("cards", "total")})
r = old("/api/orders", {"offset": 0, "limit": 10}), new("/api/orders", {"offset": 0, "limit": 10})
same("dropdown cells after replace", {"dv": r[0]["dv"], "lists": r[0]["lists"]}, {"dv": r[1]["dv"], "lists": r[1]["lists"]})
print("\n%d difference(s)" % bad)
