// Printing a shipping label from the browser. The label PDF is drawn to pictures with pdf.js (served from this app),
// then printed through a hidden frame whose page size is the label's own size, so it prints on the operator's own
// default printer. With Chrome started with --kiosk-printing it prints without a dialog.
import { esc } from "./util.js";

let pdfLoad;
function loadPdfJs() {
  pdfLoad ||= new Promise((resolve, reject) => {
    const s = document.createElement("script");
    s.src = "/static/vendor/pdf.min.js";
    s.onload = () => {
      window.pdfjsLib.GlobalWorkerOptions.workerSrc = "/static/vendor/pdf.worker.min.js";
      resolve(window.pdfjsLib);
    };
    s.onerror = () => { pdfLoad = null; reject(new Error("Could not load the PDF reader.")); };
    document.head.append(s);
  });
  return pdfLoad;
}

// The label of an order (downloaded by the server from Google Drive) as pictures: [{img, wIn, hIn}]
export async function labelPages(url) {
  const res = await fetch(url);
  if (!res.ok) {
    const j = await res.json().catch(() => ({}));
    throw new Error(j.error || `Could not get the label (error ${res.status}).`);
  }
  const data = await res.arrayBuffer();
  const pdf = await (await loadPdfJs()).getDocument({ data }).promise;
  const pages = [];
  for (let n = 1; n <= pdf.numPages; n++) {
    const page = await pdf.getPage(n);
    const size = page.getViewport({ scale: 1 });
    // Some labels (the return labels) come as a small page, about 2 x 3 inches. The printer paper is 4 x 6, so such a page is
    // enlarged to 4 inches wide (same shape), otherwise the browser shrinks it into the middle of the paper and adds its header and footer.
    const grow = size.width < 3.9 * 72 ? (4 * 72) / size.width : 1;
    const vp = page.getViewport({ scale: (300 / 72) * grow }); // 300 dpi
    const canvas = document.createElement("canvas");
    canvas.width = Math.round(vp.width);
    canvas.height = Math.round(vp.height);
    const ctx = canvas.getContext("2d");
    ctx.fillStyle = "#fff";
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    // intent "print" draws without waiting for animation frames, so it also works when the tab is in the background
    await page.render({ canvasContext: ctx, viewport: vp, intent: "print" }).promise;
    const wIn = (size.width * grow) / 72, hIn = (size.height * grow) / 72;
    const box = contentBox(canvas); // what is drawn on the page (null when it is blank)
    const fill = box ? ((box.x1 - box.x0 + 1) * (box.y1 - box.y0 + 1)) / (canvas.width * canvas.height) : 0;
    // Some carriers (FedEx, ...) give a Letter or A4 page with the 4 x 6 label drawn on it, small or enlarged. Printed as it is, the label
    // comes out tiny or on the wrong paper. Such a page is cropped to what is drawn on it and fitted onto a 4 x 6 inch page.
    const big = wIn > 4.6 || hIn > 6.6;
    const fitted = big && box ? fitTo4x6(canvas, box) : null;
    pages.push({ ...(fitted || { img: canvas.toDataURL("image/png"), wIn, hIn }), big, fill, blank: !box });
  }
  // A label PDF often comes with paperwork that goes in the package: a packing slip, a manifest, instructions (FedEx, Wayfair, ...).
  // Every page with something on it is printed, each one shrunk to the label size (4 x 6 inch). Blank pages are left out.
  const used = pages.filter((p) => !p.blank);
  return used.length ? used : pages;
}

// The box round everything that is not white on a canvas: {x0, y0, x1, y1}, or null when it is blank.
function contentBox(src) {
  const w = src.width, h = src.height;
  const d = src.getContext("2d").getImageData(0, 0, w, h).data;
  let x0 = w, y0 = h, x1 = -1, y1 = -1;
  for (let y = 0; y < h; y++) {
    const row = y * w * 4;
    for (let x = 0; x < w; x++) {
      const i = row + x * 4;
      if (d[i] < 235 || d[i + 1] < 235 || d[i + 2] < 235) {
        if (x < x0) x0 = x;
        if (x > x1) x1 = x;
        if (y < y0) y0 = y;
        if (y > y1) y1 = y;
      }
    }
  }
  return x1 < 0 ? null : { x0, y0, x1, y1 };
}

// Crop a canvas to its content (box) and centre it, as large as it fits, on a 4 x 6 inch (1200 x 1800 px) white page.
function fitTo4x6(src, box) {
  const w = src.width, h = src.height;
  const pad = 12; // a little white round the content
  const x0 = Math.max(0, box.x0 - pad), y0 = Math.max(0, box.y0 - pad), x1 = Math.min(w - 1, box.x1 + pad), y1 = Math.min(h - 1, box.y1 + pad);
  const cw = x1 - x0 + 1, ch = y1 - y0 + 1;
  const out = document.createElement("canvas");
  out.width = 1200; out.height = 1800;
  const c = out.getContext("2d");
  c.fillStyle = "#fff";
  c.fillRect(0, 0, 1200, 1800);
  const margin = 30; // 0.1 inch
  const k = Math.min((1200 - 2 * margin) / cw, (1800 - 2 * margin) / ch);
  const dw = cw * k, dh = ch * k;
  c.imageSmoothingQuality = "high";
  c.drawImage(src, x0, y0, cw, ch, (1200 - dw) / 2, margin, dw, dh); // from the top, as a label sits at the top of its sheet
  return { img: out.toDataURL("image/png"), wIn: 4, hIn: 6 };
}

// Print some HTML on pages of the given size (inches). Resolves once the print dialog was handled.
export function printHtml(body, wIn, hIn) {
  return new Promise((resolve) => {
    const f = document.createElement("iframe");
    f.setAttribute("aria-hidden", "true");
    f.style.cssText = "position:fixed;right:0;bottom:0;width:0;height:0;border:0";
    f.srcdoc = `<!doctype html><html><head><meta charset="utf-8"><style>
      @page { size: ${wIn}in ${hIn}in; margin: 0 }
      html, body { margin: 0; padding: 0; background: #fff; overflow: hidden }
      section { overflow: hidden; break-after: page; page-break-after: always }
      section:last-child { break-after: auto; page-break-after: auto }
      img { display: block; width: 100%; height: 100% }
    </style></head><body>${body}</body></html>`;
    let done = false;
    const finish = () => { if (!done) { done = true; setTimeout(() => f.remove(), 1000); resolve(true); } };
    f.onload = async () => {
      const w = f.contentWindow;
      await Promise.all([...w.document.images].map((i) => (i.complete ? 0 : new Promise((r) => { i.onload = i.onerror = r; }))));
      w.addEventListener("afterprint", finish);
      w.focus();
      w.print(); // waits for the dialog in most browsers; silent when the browser prints without asking
      setTimeout(finish, 1500);
    };
    document.body.append(f);
  });
}

export const printPages = (pages) =>
  // each sheet is a hair shorter than the paper: a sheet exactly as tall as the page can spill over by a rounding error and print a blank label after it
  printHtml(pages.map((p) => `<section style="width:${p.wIn}in;height:${(p.hIn - 0.03).toFixed(3)}in"><img alt="" src="${p.img}"></section>`).join(""), pages[0].wIn, pages[0].hIn);

// A 4 x 6 inch page to check that a printer is set up for labels: all four edges and corners should be visible.
export const printTest = (who) => printHtml(
  `<section style="width:4in;height:6in;box-sizing:border-box;border:4px solid #000;padding:.2in;display:flex;flex-direction:column;justify-content:space-between;font-family:Arial,sans-serif">
    <div style="display:flex;justify-content:space-between;font-weight:700;font-size:12pt"><span>&#9700; TOP LEFT</span><span>TOP RIGHT &#9701;</span></div>
    <div><h1 style="margin:0 0 .12in;font-size:32pt">PRINTER TEST</h1>
      <p style="margin:0 0 .08in;font-size:16pt">Operator: ${esc(who || "(no name)")}</p>
      <p style="margin:0 0 .15in;font-size:13pt">${esc(new Date().toLocaleString())}</p>
      <p style="margin:0;font-size:12pt;line-height:1.4">The label printer is set up correctly if you can read all of this, the frame is visible on all four sides and nothing is cut off.</p></div>
    <div style="display:flex;justify-content:space-between;font-weight:700;font-size:12pt"><span>&#9699; BOTTOM LEFT</span><span>BOTTOM RIGHT &#9698;</span></div>
  </section>`, 4, 6);
