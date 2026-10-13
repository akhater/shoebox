// Renders make.mjs's SVGs to PNG with headless Edge, each at its exact size
// so the small app icons stay crisp.
//   node render.mjs          -> app-icon-1024.png, app-icon-16/24/32.png (the Sb tile),
//                               shoebox-1024.png (the box on its tile),
//                               shoebox-box.png (the box alone, the sidebar's logo)
//   node render.mjs --icons  -> also puts the small app icons in icon.ico and
//                               the box in src/assets. Run it after
//                               `npm run tauri icon design/logo/app-icon-1024.png`,
//                               which shrinks the 1024 to every size.
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const EDGE = "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe";
const SIZES = [16, 24, 32];
const ICONS = "../../src-tauri/icons";

const tmp = mkdtempSync(join(tmpdir(), "shoebox-icon-"));
try {
  const svgUrl = (file) => "data:image/svg+xml;base64," + readFileSync(file).toString("base64");
  // The box alone is 860 x 800: 3x its 86 px in the sidebar.
  const items = [
    { out: "app-icon-1024.png", w: 1024, h: 1024, src: svgUrl("app-icon.svg") },
    ...SIZES.map((n) => ({ out: `app-icon-${n}.png`, w: n, h: n, src: svgUrl("app-icon.svg") })),
    { out: "shoebox-1024.png", w: 1024, h: 1024, src: svgUrl("shoebox.svg") },
    { out: "shoebox-box.png", w: 258, h: 240, src: svgUrl("shoebox-box.svg") },
  ];
  writeFileSync(join(tmp, "r.html"), `<!doctype html><pre id="out"></pre><script>
    const items = ${JSON.stringify(items)};
    Promise.all(items.map(({ out, w, h, src }) => new Promise((ok, fail) => {
      const img = new Image();
      img.onload = () => {
        const c = document.createElement("canvas");
        c.width = w;
        c.height = h;
        c.getContext("2d").drawImage(img, 0, 0, w, h);
        ok([out, c.toDataURL("image/png")]);
      };
      img.onerror = fail;
      img.src = src;
    }))).then((r) => (out.textContent = JSON.stringify(Object.fromEntries(r))), (e) => (out.textContent = "ERR " + e));
  </script>`);
  const dom = execFileSync(EDGE, [
    "--headless=new", "--disable-gpu", `--user-data-dir=${join(tmp, "profile")}`,
    "--virtual-time-budget=5000", "--dump-dom", pathToFileURL(join(tmp, "r.html")).href,
  ], { encoding: "utf8", maxBuffer: 1 << 26 });
  const json = dom.match(/<pre id="out">([^<]*)<\/pre>/)?.[1];
  if (!json || json.startsWith("ERR")) throw new Error("Edge did not render the icons: " + (json || "no output"));
  const pngs = Object.fromEntries(Object.entries(JSON.parse(json)).map(([out, url]) => [out, Buffer.from(url.split(",")[1], "base64")]));
  for (const [out, png] of Object.entries(pngs)) writeFileSync(out, png);

  if (process.argv.includes("--icons")) {
    patchIco(`${ICONS}/icon.ico`, Object.fromEntries(SIZES.map((n) => [n, pngs[`app-icon-${n}.png`]])));
    writeFileSync(`${ICONS}/32x32.png`, pngs["app-icon-32.png"]);
    writeFileSync("../../src/assets/shoebox-box.png", pngs["shoebox-box.png"]);
  }
  console.log("ok");
} finally {
  rmSync(tmp, { recursive: true, force: true });
}

// Swaps the PNG entries of the given sizes in an .ico, keeps the others.
function patchIco(path, pngs) {
  const old = readFileSync(path);
  const count = old.readUInt16LE(4);
  const entries = [];
  for (let k = 0; k < count; k++) {
    const e = old.subarray(6 + 16 * k, 22 + 16 * k);
    const size = e[0] || 256;
    const data = pngs[size] ?? old.subarray(e.readUInt32LE(12), e.readUInt32LE(12) + e.readUInt32LE(8));
    entries.push({ head: Buffer.from(e), data });
  }
  let offset = 6 + 16 * count;
  const head = Buffer.alloc(offset);
  old.copy(head, 0, 0, 6);
  entries.forEach(({ head: e, data }, k) => {
    e.writeUInt32LE(data.length, 8);
    e.writeUInt32LE(offset, 12);
    e.copy(head, 6 + 16 * k);
    offset += data.length;
  });
  writeFileSync(path, Buffer.concat([head, ...entries.map((e) => e.data)]));
}
