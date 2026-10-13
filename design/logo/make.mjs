// Builds the Shoebox logo (vintage: kraft box, tarnished silver lid, twine).
//   node make.mjs
// The drafts it was picked from are in drafts/.
import { writeFileSync } from "node:fs";

const INK = "#3a2a1c";
const f = (n) => +n.toFixed(1);

// Box colours. Kraft came first; at 24 px it read as Explorer's yellow folder.
// stops: full logo, top to bottom; px: pixel icons, left to right.
const BOXES = {
  kraft: { stops: ["#cf9f62", "#bb8a50", "#94643a"], rim: "#e2bc84", hi: "#e7c48e", lo: "#6e4826", px: ["#d3a364", "#bf8c4f", "#9c6b3d"], pxRim: "#d9ae74", pxLo: "#7a5130" },
  red: { stops: ["#b0473c", "#93322a", "#66201a"], rim: "#d07a68", hi: "#d98b78", lo: "#3e120e", px: ["#b54a3e", "#963329", "#6e231c"], pxRim: "#cf7a68", pxLo: "#4a1712" },
  green: { stops: ["#4a7a5c", "#365f47", "#22402f"], rim: "#7ea88c", hi: "#8bb397", lo: "#15281d", px: ["#4d7d5f", "#386249", "#264634"], pxRim: "#7ea88c", pxLo: "#182c21" },
  brown: { stops: ["#875a3a", "#6c472d", "#4a2f1d"], rim: "#b08460", hi: "#b88d68", lo: "#2a1a10", px: ["#8c5d3c", "#70492e", "#52341f"], pxRim: "#ad8260", pxLo: "#2e1c11" },
};

// Photo tones: sky top/bottom, middle ground, foreground, figures.
const PAL = {
  sepia: { s1: "#ecdcb8", s2: "#cdb07e", mid: "#9c7c50", low: "#c9ab7a", ink: "#4a3320" },
  bw: { s1: "#eeece6", s2: "#c4c1b9", mid: "#85827a", low: "#b9b6ae", ink: "#2f2d29" },
};

// ---------------------------------------------------------------- materials

const defs = `
  <filter id="aged" x="0" y="0" width="100%" height="100%" color-interpolation-filters="sRGB">
    <feTurbulence type="fractalNoise" baseFrequency="0.9" numOctaves="2" seed="4" result="f"/>
    <feColorMatrix in="f" type="matrix" values="0.32 0 0 0 0.76  0.32 0 0 0 0.76  0.32 0 0 0 0.76  0 0 0 0 1" result="fine"/>
    <feTurbulence type="fractalNoise" baseFrequency="0.008 0.016" numOctaves="3" seed="11" result="b"/>
    <feColorMatrix in="b" type="matrix" values="0.42 0 0 0 0.79  0.5 0 0 0 0.74  0.6 0 0 0 0.68  0 0 0 0 1" result="blot"/>
    <feBlend in="fine" in2="blot" mode="multiply" result="tex"/>
    <feBlend in="SourceGraphic" in2="tex" mode="multiply" result="mix"/>
    <feComposite in="mix" in2="SourceAlpha" operator="in"/>
  </filter>
  <filter id="worn" x="-4%" y="-4%" width="108%" height="108%">
    <feTurbulence type="fractalNoise" baseFrequency="0.035" numOctaves="2" seed="3" result="t"/>
    <feDisplacementMap in="SourceGraphic" in2="t" scale="6" xChannelSelector="R" yChannelSelector="G"/>
  </filter>
  <filter id="inky" x="-5%" y="-5%" width="110%" height="110%">
    <feTurbulence type="fractalNoise" baseFrequency="0.12" numOctaves="2" seed="8" result="t"/>
    <feDisplacementMap in="SourceGraphic" in2="t" scale="5" xChannelSelector="R" yChannelSelector="G" result="d"/>
    <feTurbulence type="fractalNoise" baseFrequency="0.5" numOctaves="1" seed="5" result="g"/>
    <feColorMatrix in="g" type="matrix" values="0 0 0 0 0  0 0 0 0 0  0 0 0 0 0  0 0 0 -2.2 1.7" result="holes"/>
    <feComposite in="d" in2="holes" operator="in"/>
  </filter>
  <linearGradient id="kraft" x1="0" y1="0" x2="0.2" y2="1">
    <stop offset="0%" stop-color="#cf9f62"/>
    <stop offset="55%" stop-color="#bb8a50"/>
    <stop offset="100%" stop-color="#94643a"/>
  </linearGradient>
  <linearGradient id="bodyShade" x1="0" y1="0" x2="1" y2="0">
    <stop offset="0%" stop-color="#ffffff" stop-opacity="0.12"/>
    <stop offset="7%" stop-color="#ffffff" stop-opacity="0"/>
    <stop offset="90%" stop-color="#000000" stop-opacity="0"/>
    <stop offset="100%" stop-color="#000000" stop-opacity="0.22"/>
  </linearGradient>
  <linearGradient id="under" x1="0" y1="0" x2="0" y2="1">
    <stop offset="0%" stop-color="#2a1a0c" stop-opacity="0.55"/>
    <stop offset="100%" stop-color="#2a1a0c" stop-opacity="0"/>
  </linearGradient>
  <!-- Old silver: warm, tarnished, not chrome -->
  <linearGradient id="tin" x1="0" y1="0" x2="1" y2="0.35">
    <stop offset="0%" stop-color="#a9a69c"/>
    <stop offset="18%" stop-color="#dcd9cf"/>
    <stop offset="32%" stop-color="#f1eee4"/>
    <stop offset="55%" stop-color="#a6a398"/>
    <stop offset="78%" stop-color="#d3d0c5"/>
    <stop offset="100%" stop-color="#8a877d"/>
  </linearGradient>
  <linearGradient id="sheen" x1="0" y1="0" x2="0" y2="1">
    <stop offset="0%" stop-color="#ffffff" stop-opacity="0.3"/>
    <stop offset="45%" stop-color="#ffffff" stop-opacity="0"/>
    <stop offset="100%" stop-color="#2a1a0c" stop-opacity="0.3"/>
  </linearGradient>
  <radialGradient id="ground" cx="0.5" cy="0.5" r="0.5">
    <stop offset="0%" stop-color="#000000" stop-opacity="0.5"/>
    <stop offset="100%" stop-color="#000000" stop-opacity="0"/>
  </radialGradient>
  <linearGradient id="sepia" x1="0" y1="0" x2="0" y2="1">
    <stop offset="0%" stop-color="#e6d3a9"/>
    <stop offset="100%" stop-color="#bf9f6c"/>
  </linearGradient>
  <linearGradient id="faded" x1="0" y1="0" x2="0" y2="1">
    <stop offset="0%" stop-color="#a9c2bb"/>
    <stop offset="100%" stop-color="#eadbbb"/>
  </linearGradient>
  <linearGradient id="paper" x1="0" y1="0" x2="0.7" y2="1">
    <stop offset="0%" stop-color="#f3e9d0"/>
    <stop offset="100%" stop-color="#dcc7a0"/>
  </linearGradient>
  <radialGradient id="vignette" cx="0.5" cy="0.45" r="0.62">
    <stop offset="60%" stop-color="#7a5a32" stop-opacity="0"/>
    <stop offset="100%" stop-color="#7a5a32" stop-opacity="0.38"/>
  </radialGradient>
  <linearGradient id="tile" x1="0" y1="0" x2="0.85" y2="1">
    <stop offset="0%" stop-color="#3a3f46"/>
    <stop offset="45%" stop-color="#23262b"/>
    <stop offset="100%" stop-color="#15171a"/>
  </linearGradient>
  <linearGradient id="edge" x1="0" y1="0" x2="1" y2="1">
    <stop offset="0%" stop-color="#8e979f" stop-opacity="0.9"/>
    <stop offset="50%" stop-color="#5a6169" stop-opacity="0.35"/>
    <stop offset="100%" stop-color="#c9d1d9" stop-opacity="0.55"/>
  </linearGradient>
  <pattern id="hatch" width="12" height="12" patternUnits="userSpaceOnUse" patternTransform="rotate(35)">
    <rect width="12" height="4" fill="${INK}"/>
  </pattern>
  <pattern id="twist" width="9" height="9" patternUnits="userSpaceOnUse" patternTransform="rotate(40)">
    <rect width="9" height="9" fill="#cdb07c"/>
    <rect width="3.5" height="9" fill="#8d6f45"/>
  </pattern>
  <pattern id="lines" width="10" height="10" patternUnits="userSpaceOnUse">
    <rect width="10" height="3" fill="${INK}"/>
  </pattern>
  ${printDef("printA", 230, 170, "sepia")}
  ${printDef("printB", 230, 170, "faded")}
  ${printDef("printC", 230, 170, "sepia")}
  ${photoDef("photoPeople", 230, 170, "people", "sepia")}
  ${photoDef("photoTree", 230, 170, "tree", "bw")}
  ${photoDef("photoSea", 230, 170, "sea", "sepia")}
  ${Object.entries(BOXES).map(([k, b]) => `<linearGradient id="box-${k}" x1="0" y1="0" x2="0.2" y2="1">
    <stop offset="0%" stop-color="${b.stops[0]}"/><stop offset="55%" stop-color="${b.stops[1]}"/><stop offset="100%" stop-color="${b.stops[2]}"/>
  </linearGradient>`).join("\n  ")}
  <linearGradient id="inside" x1="0" y1="0" x2="0" y2="1">
    <stop offset="0%" stop-color="#6b4526"/>
    <stop offset="100%" stop-color="#24160a"/>
  </linearGradient>
`;

// Scalloped "deckle" edge of an old print.
function deckle(w, h, size = 13, d = 4) {
  const side = (x0, y0, x1, y1, nx, ny) => {
    const n = Math.max(2, Math.round(Math.hypot(x1 - x0, y1 - y0) / size));
    let s = "";
    for (let i = 0; i < n; i++) {
      const t = (i + 0.5) / n, u = (i + 1) / n;
      s += ` Q${f(x0 + (x1 - x0) * t + nx * d)} ${f(y0 + (y1 - y0) * t + ny * d)} ${f(x0 + (x1 - x0) * u)} ${f(y0 + (y1 - y0) * u)}`;
    }
    return s;
  };
  return `M0 0${side(0, 0, w, 0, 0, -1)}${side(w, 0, w, h, 1, 0)}${side(w, h, 0, h, 0, 1)}${side(0, h, 0, 0, -1, 0)} Z`;
}

function printDef(id, w, h, sky) {
  const b = 16, iw = w - 2 * b, ih = h - 2 * b;
  const X = (t) => f(b + iw * t), Y = (t) => f(b + ih * t);
  const ground = sky === "sepia" ? "#7a5a38" : "#9a5d3e";
  const sun = sky === "sepia" ? "#f3e6c4" : "#f4e3b8";
  return `<g id="${id}">
    <path d="${deckle(w, h)}" fill="#efe6d0" stroke="#bfa97f" stroke-width="2"/>
    <rect x="${b}" y="${b}" width="${iw}" height="${ih}" fill="url(#${sky})"/>
    <circle cx="${X(0.72)}" cy="${Y(0.3)}" r="${f(ih * 0.13)}" fill="${sun}"/>
    <path d="M${X(0)} ${Y(1)} L${X(0)} ${Y(0.7)} L${X(0.22)} ${Y(0.45)} L${X(0.4)} ${Y(0.66)} L${X(0.62)} ${Y(0.4)} L${X(1)} ${Y(0.78)} L${X(1)} ${Y(1)} Z" fill="${ground}"/>
    <rect x="${b}" y="${b}" width="${iw}" height="${ih}" fill="#8a6a3a" fill-opacity="0.12"/>
  </g>`;
}

// Old photos, not the sun-over-mountains "image missing" placeholder: a family
// snapshot, a tree in a field, a beach. Sepia or black and white.
function photoDef(id, w, h, scene, pal) {
  const p = PAL[pal], b = 16, iw = w - 2 * b, ih = h - 2 * b;
  const X = (t) => f(b + iw * t), Y = (t) => f(b + ih * t), R = (t) => f(ih * t);
  const person = (cx, top, s) => `
      <circle cx="${X(cx)}" cy="${Y(top)}" r="${R(0.11 * s)}" fill="${p.ink}"/>
      <path d="M${X(cx - 0.13 * s)} ${Y(1)} V${Y(top + 0.27 * s)} Q${X(cx - 0.13 * s)} ${Y(top + 0.13 * s)} ${X(cx)} ${Y(top + 0.13 * s)} Q${X(cx + 0.13 * s)} ${Y(top + 0.13 * s)} ${X(cx + 0.13 * s)} ${Y(top + 0.27 * s)} V${Y(1)} Z" fill="${p.ink}"/>`;
  const art = {
    people: `<rect x="${b}" y="${Y(0.8)}" width="${iw}" height="${R(0.2)}" fill="${p.low}"/>${person(0.37, 0.32, 1)}${person(0.64, 0.4, 0.9)}`,
    tree: `<path d="M${X(0)} ${Y(0.74)} Q${X(0.5)} ${Y(0.62)} ${X(1)} ${Y(0.72)} V${Y(1)} H${X(0)} Z" fill="${p.mid}"/>
      <rect x="${X(0.285)}" y="${Y(0.42)}" width="${f(iw * 0.03)}" height="${R(0.3)}" fill="${p.ink}"/>
      <circle cx="${X(0.3)}" cy="${Y(0.32)}" r="${R(0.17)}" fill="${p.ink}"/>
      <circle cx="${X(0.22)}" cy="${Y(0.42)}" r="${R(0.12)}" fill="${p.ink}"/>
      <circle cx="${X(0.38)}" cy="${Y(0.43)}" r="${R(0.12)}" fill="${p.ink}"/>
      <circle cx="${X(0.77)}" cy="${Y(0.6)}" r="${R(0.06)}" fill="${p.ink}" fill-opacity="0.6"/>`,
    sea: `<rect x="${b}" y="${Y(0.56)}" width="${iw}" height="${R(0.2)}" fill="${p.mid}"/>
      <rect x="${b}" y="${Y(0.76)}" width="${iw}" height="${R(0.24)}" fill="${p.low}"/>
      <path d="M${X(0)} ${Y(0.56)} H${X(1)}" stroke="${p.s1}" stroke-width="2" stroke-opacity="0.7"/>
      <rect x="${X(0.67)}" y="${Y(0.66)}" width="6" height="${R(0.13)}" rx="2" fill="${p.ink}"/>
      <circle cx="${f(+X(0.67) + 3)}" cy="${Y(0.63)}" r="4" fill="${p.ink}"/>`,
  }[scene];
  return `<linearGradient id="${id}-sky" x1="0" y1="0" x2="0" y2="1">
    <stop offset="0%" stop-color="${p.s1}"/><stop offset="100%" stop-color="${p.s2}"/>
  </linearGradient>
  <g id="${id}">
    <path d="${deckle(w, h)}" fill="#efe6d0" stroke="#bfa97f" stroke-width="2"/>
    <rect x="${b}" y="${b}" width="${iw}" height="${ih}" fill="url(#${id}-sky)"/>
    ${art}
    <rect x="${b}" y="${b}" width="${iw}" height="${ih}" fill="#8a6a3a" fill-opacity="0.1"/>
  </g>`;
}

// Twine: a light strand with darker twists over it.
function twine(d, w = 14) {
  return `<path d="${d}" fill="none" stroke="url(#twist)" stroke-width="${w}" stroke-linecap="round"/>
    <path d="${d}" fill="none" stroke="#efdcb3" stroke-width="${f(w * 0.22)}" stroke-opacity="0.55" stroke-linecap="round"/>`;
}

const svg = (body, comment, [x, y, w, h] = [0, 0, 1024, 1024]) =>
  `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${x} ${y} ${w} ${h}" width="${w}" height="${h}">
  <!-- ${comment} -->
  <defs>${defs}</defs>
${body}
</svg>
`;

// ----------------------------------------------------- the tied-up shoebox

// Kraft box, tarnished silver lid, twine with a bow, a handwritten tag,
// prints sticking out of both sides. Drawn for a 1024 canvas.
const tiedBox = `
  <ellipse cx="512" cy="786" rx="430" ry="32" fill="url(#ground)"/>

  <use href="#printA" transform="translate(40 322) rotate(-16)"/>
  <use href="#printB" transform="translate(776 296) rotate(14)"/>

  <g filter="url(#worn)">
    <rect x="132" y="420" width="760" height="360" rx="14" fill="url(#kraft)" filter="url(#aged)"/>
    <rect x="132" y="420" width="760" height="360" rx="14" fill="url(#bodyShade)"/>
    <rect x="132" y="426" width="760" height="56" fill="url(#under)"/>
    <path d="M146 764 H878" stroke="#6e4826" stroke-width="4" stroke-opacity="0.5"/>
    <path d="M150 770 Q140 740 146 700 M876 690 Q884 730 878 768" stroke="#e7c48e" stroke-width="5" stroke-opacity="0.55" fill="none"/>
  </g>

  <g filter="url(#worn)">
    <rect x="100" y="280" width="824" height="150" rx="16" fill="url(#tin)" filter="url(#aged)"/>
    <rect x="100" y="280" width="824" height="150" rx="16" fill="url(#sheen)"/>
    <rect x="100" y="280" width="824" height="150" rx="16" fill="none" stroke="#5f5a50" stroke-opacity="0.75" stroke-width="4"/>
    <rect x="122" y="288" width="780" height="6" rx="3" fill="#ffffff" fill-opacity="0.6"/>
  </g>

  <g filter="url(#worn)">
    ${twine("M104 352 H920")}
    ${twine("M604 284 V778")}
    <path d="M606 356 C 646 420 690 470 692 559" fill="none" stroke="#8d6f45" stroke-width="3.5"/>
    ${twine("M604 352 C 548 292 492 318 520 366 C 540 398 586 384 604 352", 12)}
    ${twine("M604 352 C 660 292 716 318 688 366 C 668 398 622 384 604 352", 12)}
    ${twine("M604 352 C 596 400 574 432 552 470", 12)}
    ${twine("M604 352 C 618 398 640 428 660 462", 12)}
    <circle cx="604" cy="352" r="15" fill="#b8986a" stroke="#8d6f45" stroke-width="3"/>
  </g>

  <g transform="rotate(9 760 580)" filter="url(#worn)">
    <path d="M690 500 H880 V640 H690 L660 604 V536 Z" fill="url(#paper)" filter="url(#aged)"/>
    <path d="M690 500 H880 V640 H690 L660 604 V536 Z" fill="none" stroke="#a88c5c" stroke-width="2.5"/>
    <circle cx="690" cy="570" r="16" fill="#c9b386"/>
    <circle cx="690" cy="570" r="8" fill="#5a4026"/>
    <text x="784" y="590" text-anchor="middle" font-family="Ink Free, Segoe Print, cursive" font-size="54" fill="${INK}">photos</text>
  </g>
`;

const tile = (fill, stroke) => `
  <rect x="72" y="72" width="880" height="880" rx="150" fill="${fill}"/>
  <rect x="72" y="72" width="880" height="880" rx="150" fill="none" stroke="${stroke}" stroke-width="10"/>`;

// ------------------------------------------------ open box (2026-10-13)

// The closed box reads as a generic "package" / missing-icon icon. Same
// style, lid lifted, prints peeking out. Three drafts; peek2 (lid off) is the
// logo because its prints still read at taskbar size.

const lidAt = (x, y, w, h) => `
  <g filter="url(#worn)">
    <rect x="${x}" y="${y}" width="${w}" height="${h}" rx="16" fill="url(#tin)" filter="url(#aged)"/>
    <rect x="${x}" y="${y}" width="${w}" height="${h}" rx="16" fill="url(#sheen)"/>
    <rect x="${x}" y="${y}" width="${w}" height="${h}" rx="16" fill="none" stroke="#5f5a50" stroke-opacity="0.75" stroke-width="4"/>
    <rect x="${x + 22}" y="${y + 8}" width="${w - 44}" height="6" rx="3" fill="#ffffff" fill-opacity="0.6"/>
  </g>`;

// The front wall of the open box; its top edge is the cut cardboard rim.
const wallAt = (x, y, w, h, box = "kraft") => {
  const c = BOXES[box];
  return `
  <g filter="url(#worn)">
    <rect x="${x}" y="${y}" width="${w}" height="${h}" rx="14" fill="url(#box-${box})" filter="url(#aged)"/>
    <rect x="${x}" y="${y}" width="${w}" height="${h}" rx="14" fill="url(#bodyShade)"/>
    <rect x="${x + 6}" y="${y}" width="${w - 12}" height="9" rx="4" fill="${c.rim}"/>
    <path d="M${x + 14} ${y + h - 16} H${x + w - 14}" stroke="${c.lo}" stroke-width="4" stroke-opacity="0.5"/>
    <path d="M${x + 18} ${y + h - 10} Q${x + 8} ${y + h - 40} ${x + 14} ${y + h - 80} M${x + w - 16} ${y + h - 90} Q${x + w - 8} ${y + h - 50} ${x + w - 14} ${y + h - 12}" stroke="${c.hi}" stroke-width="5" stroke-opacity="0.55" fill="none"/>
  </g>`;
};

// Inside of the back wall, seen over the front rim.
const openingAt = (x, y, w, depth) => `
  <path d="M${x + 20} ${y - depth} H${x + w - 20} L${x + w - 4} ${y + 10} H${x + 4} Z" fill="url(#inside)"/>
  <path d="M${x + 20} ${y - depth} H${x + w - 20}" stroke="#d5a868" stroke-width="6" stroke-linecap="round"/>`;

const bowAt = (x, y) => `
    ${twine(`M${x} ${y} C ${x - 56} ${y - 60} ${x - 112} ${y - 34} ${x - 84} ${y + 14} C ${x - 64} ${y + 46} ${x - 18} ${y + 32} ${x} ${y}`, 12)}
    ${twine(`M${x} ${y} C ${x + 56} ${y - 60} ${x + 112} ${y - 34} ${x + 84} ${y + 14} C ${x + 64} ${y + 46} ${x + 18} ${y + 32} ${x} ${y}`, 12)}
    ${twine(`M${x} ${y} C ${x - 8} ${y + 48} ${x - 30} ${y + 80} ${x - 52} ${y + 118}`, 12)}
    ${twine(`M${x} ${y} C ${x + 14} ${y + 46} ${x + 36} ${y + 76} ${x + 56} ${y + 110}`, 12)}
    <circle cx="${x}" cy="${y}" r="15" fill="#b8986a" stroke="#8d6f45" stroke-width="3"/>`;

// The handwritten tag; (x, y) is its pointed end, the string hole is at +30 +70.
const tagAt = (x, y, rot) => `
  <g transform="translate(${x} ${y}) rotate(${rot} 100 70)" filter="url(#worn)">
    <path d="M30 0 H220 V140 H30 L0 104 V36 Z" fill="url(#paper)" filter="url(#aged)"/>
    <path d="M30 0 H220 V140 H30 L0 104 V36 Z" fill="none" stroke="#a88c5c" stroke-width="2.5"/>
    <circle cx="30" cy="70" r="16" fill="#c9b386"/>
    <circle cx="30" cy="70" r="8" fill="#5a4026"/>
    <text x="124" y="90" text-anchor="middle" font-family="Ink Free, Segoe Print, cursive" font-size="54" fill="${INK}">photos</text>
  </g>`;

const print = (id, x, y, rot, s = 1.25) => `<use href="#${id}" transform="translate(${x} ${y}) rotate(${rot}) scale(${s})"/>`;

// 1 - Peeking: the tied box, its lid levered up by the prints inside.
const peek1 = `
  <ellipse cx="512" cy="786" rx="430" ry="32" fill="url(#ground)"/>
  <use href="#printA" transform="translate(40 322) rotate(-16)"/>
  <use href="#printB" transform="translate(776 296) rotate(14)"/>
  ${openingAt(132, 420, 760, 26)}
  ${print("printB", 200, 318, -7)}
  ${print("printC", 452, 282, 5)}
  ${wallAt(132, 420, 760, 360)}
  ${twine("M604 420 V778")}${twine("M607 386 L604 424", 12)}
  <g transform="rotate(8 924 430)">
    ${lidAt(100, 280, 824, 150)}
    <g filter="url(#worn)">${twine("M104 352 H920")}${twine("M604 284 V430")}${bowAt(604, 352)}</g>
  </g>
  <path d="M620 314 C 664 400 700 470 692 559" fill="none" stroke="#8d6f45" stroke-width="3.5"/>
  ${tagAt(660, 500, 9)}
`;

// 2 - Lid off: the lid leans behind the box, prints fanned out in front of it.
// prints: left, right, middle.
const lidOff = (box, [l, r, m]) => `
  <ellipse cx="512" cy="800" rx="430" ry="32" fill="url(#ground)"/>
  <g transform="rotate(-5 512 400)">${lidAt(118, 150, 788, 320)}</g>
  ${openingAt(146, 486, 732, 30)}
  ${print(l, 168, 290, -15, 1.3)}
  ${print(r, 560, 240, 14, 1.3)}
  ${print(m, 352, 200, -2, 1.35)}
  ${wallAt(146, 486, 732, 310, box)}
  <g filter="url(#worn)">${twine("M150 640 H874")}${bowAt(604, 640)}</g>
  <path d="M624 652 C 660 700 690 730 696 778" fill="none" stroke="#8d6f45" stroke-width="3.5"/>
  ${tagAt(664, 720, 12)}
`;
const peek2 = lidOff("kraft", ["printA", "printC", "printB"]);
const PHOTOS = ["photoTree", "photoSea", "photoPeople"];

// 3 - Lid ajar: hinged on the right rim, swung up, prints rising out.
const peek3 = `
  <ellipse cx="512" cy="800" rx="430" ry="32" fill="url(#ground)"/>
  ${openingAt(146, 470, 732, 28)}
  ${print("printA", 150, 330, -18, 1.3)}
  ${print("printB", 330, 250, -6, 1.3)}
  ${print("printC", 520, 300, 8, 1.2)}
  ${wallAt(146, 470, 732, 326)}
  <g transform="rotate(18 906 480)">
    ${lidAt(130, 330, 776, 150)}
    <g filter="url(#worn)">${twine("M134 402 H902")}${bowAt(600, 402)}</g>
  </g>
  <path d="M640 318 C 632 430 626 570 622 713" fill="none" stroke="#8d6f45" stroke-width="3.5"/>
  ${tagAt(590, 660, 14)}
`;

const onTile = (body, comment) => svg(`${tile("url(#tile)", "url(#edge)")}
  <g transform="translate(512 512) scale(0.86) translate(-512 -530)">${body}</g>`, comment);

// The app icon (chosen 2026-10-13: lid off, oxblood box, old photos), and the
// box alone for docs and the website.
const LOGO = lidOff("red", PHOTOS);
writeFileSync("shoebox.svg", onTile(LOGO,
  "Shoebox app icon: open oxblood shoebox, tarnished silver lid behind, old photos fanned out, on Argentum's tile. Generated by make.mjs."));
writeFileSync("shoebox-box.svg", svg(LOGO, "Shoebox, the box alone (no tile), cropped. Generated by make.mjs.", [80, 100, 860, 800]));

writeFileSync("drafts/closed-tied.svg", onTile(tiedBox, "Draft: the tied-up closed box (logo until 2026-10-13). Generated by make.mjs."));
writeFileSync("drafts/peek-1.svg", onTile(peek1, "Draft: peeking, lid levered up by the prints. Generated by make.mjs."));
writeFileSync("drafts/peek-2.svg", onTile(peek2, "Draft: lid off behind the box, prints fanned out. Generated by make.mjs."));
writeFileSync("drafts/peek-3.svg", onTile(peek3, "Draft: lid ajar on the right rim, prints rising out. Generated by make.mjs."));

// ------------------------------------------------------ small sizes, by hand

// At 16-32 px the textures, twine and tag turn to mush, so these are drawn on
// the pixel grid instead of shrunk: the same tile, silver lid behind, prints
// and box, flat. The prints are what say "photos", so they get the pixels.
// They were the app icon's 16/24/32 until the Sb tile took the taskbar; kept
// in drafts/ in case the box goes back there.
// [x, y, w, h] in pixels of that size. Prints listed back to front, as
// [x, y, w, h, slot, tilt] (slot: left, right, middle); tilted ones fan out
// from the rim.
const SMALL = {
  16: { r: 3, lid: [2, 3, 12, 6], body: [3, 9, 10, 5],
    prints: [[4, 4, 8, 6, "m"]] },
  24: { r: 4.5, lid: [3, 4, 18, 10], body: [4, 15, 16, 6],
    prints: [[4, 8, 7, 8, "l", -16], [13, 8, 7, 8, "r", 16], [7, 6, 10, 10, "m"]] },
  32: { r: 6, inset: 1, lid: [5, 5, 22, 14], body: [6, 20, 20, 8], twine: 23, knot: 18, tag: [20, 24, 6, 4],
    prints: [[6, 10, 9, 11, "l", -16], [17, 10, 9, 11, "r", 16], [10, 8, 12, 13, "m"]] },
};
// Which old photo goes in each slot. Not the full logo's order: at 24 px two
// figures in the middle read as the "contacts" icon, a tree reads as a photo.
const SLOTS = { l: ["people", "bw"], r: ["sea", "sepia"], m: ["tree", "sepia"] };

function small(n, box = "kraft") {
  const L = SMALL[n], i = L.inset ?? 0, c = BOXES[box];
  const [lx, ly, lw, lh] = L.lid, [bx, by, bw, bh] = L.body;
  const rect = (x, y, w, h, fill, extra = "") => `<rect x="${x}" y="${y}" width="${w}" height="${h}" fill="${fill}"${extra}/>`;
  const aa = ` shape-rendering="geometricPrecision"`;
  // A print: cream border and an old photo; only the part above the rim shows.
  // Round shapes are anti-aliased, bands stay on the pixel grid.
  const print = ([x, y, w, h, slot, tilt]) => {
    const [scene, pal] = SLOTS[slot], p = PAL[pal];
    const ix = x + 1, iy = y + 1, iw = w - 2, ih = by - iy;
    const X = (u) => f(ix + iw * u), Y = (v) => f(iy + ih * v), band = (v) => iy + Math.round(ih * v);
    const art = {
      people: [[0.36, 0.3, 1], [0.66, 0.42, 0.85]].map(([u, v, s]) => {
        const r = Math.max(0.9, iw * 0.13 * s);
        return `<circle cx="${X(u)}" cy="${Y(v)}" r="${f(r)}" fill="${p.ink}"${aa}/>
        <rect x="${f(+X(u) - r * 1.5)}" y="${f(+Y(v) + r * 1.1)}" width="${f(r * 3)}" height="${ih}" rx="${f(r)}" fill="${p.ink}"${aa}/>`;
      }).join(""),
      tree: `${rect(ix, band(0.7), iw, by - band(0.7), p.mid)}
        <rect x="${f(+X(0.36) - 0.5)}" y="${Y(0.4)}" width="1" height="${f(ih * 0.32)}" fill="${p.ink}"/>
        ${[[0.36, 0.3, 0.2], [0.24, 0.42, 0.15], [0.48, 0.42, 0.15]].map(([u, v, r]) =>
          `<circle cx="${X(u)}" cy="${Y(v)}" r="${f(Math.max(0.8, iw * r))}" fill="${p.ink}"${aa}/>`).join("")}
        ${iw >= 8 ? `<circle cx="${X(0.8)}" cy="${Y(0.64)}" r="${f(iw * 0.08)}" fill="${p.ink}" fill-opacity="0.7"${aa}/>` : ""}`,
      sea: `${rect(ix, band(0.5), iw, band(0.75) - band(0.5), p.mid)}${rect(ix, band(0.75), iw, by - band(0.75), p.low)}`,
    }[scene];
    const body = `${rect(x, y, w, h, "#f1e8d2")}${rect(ix, iy, iw, ih, p.s1)}${art}`;
    return tilt ? `<g transform="rotate(${tilt} ${x + w / 2} ${by})" shape-rendering="geometricPrecision">${body}</g>` : body;
  };
  const tag = L.tag && (() => {
    const [x, y, w, h] = L.tag;
    return `${rect(x, y, w, h, "#efe4c8")}${rect(x, y + h - 1, w, 1, "#c9b386")}${rect(x + 2, y + 2, w - 3, 1, INK, ` fill-opacity="0.6"`)}${rect(x + 1, y + 2, 1, 1, "#5a4026")}`;
  })();
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${n} ${n}" width="${n}" height="${n}" shape-rendering="crispEdges">
  <!-- Shoebox app icon drawn for ${n} px. Generated by make.mjs. -->
  <defs>
    <linearGradient id="tile" x1="0" y1="0" x2="0.85" y2="1">
      <stop offset="0%" stop-color="#3a3f46"/><stop offset="45%" stop-color="#23262b"/><stop offset="100%" stop-color="#15171a"/>
    </linearGradient>
    <linearGradient id="tin" x1="0" y1="0" x2="1" y2="0.3">
      <stop offset="0%" stop-color="#a9a69c"/><stop offset="25%" stop-color="#e2dfd5"/><stop offset="55%" stop-color="#b3b0a5"/><stop offset="80%" stop-color="#d3d0c5"/><stop offset="100%" stop-color="#8a877d"/>
    </linearGradient>
    <linearGradient id="box" x1="0" y1="0" x2="1" y2="0">
      <stop offset="0%" stop-color="${c.px[0]}"/><stop offset="80%" stop-color="${c.px[1]}"/><stop offset="100%" stop-color="${c.px[2]}"/>
    </linearGradient>
  </defs>
  <rect x="${i}" y="${i}" width="${n - 2 * i}" height="${n - 2 * i}" rx="${L.r}" fill="url(#tile)" shape-rendering="auto"/>
  <rect x="${i + 0.5}" y="${i + 0.5}" width="${n - 2 * i - 1}" height="${n - 2 * i - 1}" rx="${L.r - 0.5}" fill="none" stroke="#9aa3ab" stroke-opacity="0.35" shape-rendering="auto"/>
  ${rect(lx, ly, lw, lh, "url(#tin)")}
  ${rect(lx, ly, lw, 1, "#f6f3ea")}
  ${rect(bx, by - 1, bw, 1, "#3e2814")}
  ${L.prints.map(print).join("\n  ")}
  ${rect(bx, by, bw, bh, "url(#box)")}
  ${rect(bx, by, bw, 1, c.pxRim)}
  ${rect(bx, by + bh - 1, bw, 1, c.pxLo)}
  ${L.twine ? rect(bx, L.twine, bw, 1, "#dcc196") : ""}
  ${L.knot ? rect(L.knot, L.twine - 1, 2, 3, "#a07c4a") : ""}
  ${tag ?? ""}
</svg>
`;
}

// Was the app icon's small sizes until the Sb tile took the taskbar.
for (const n of Object.keys(SMALL)) writeFileSync(`drafts/box-${n}.svg`, small(+n, "red"));

// ---------------------------------------------- "Sb" tile drafts (2026-10-13)

// The taskbar icon is Sb and the logo lives elsewhere: an element tile like
// Argentum's Ag (antimony = Sb), no number. Text stays text here: these
// are drafts, rendered on this machine.
const SANS = "Segoe UI, Helvetica Neue, Arial, sans-serif";
const SERIF = "Palatino Linotype, Book Antiqua, Georgia, serif";
const sbTile = ({ tile, edge, ink, font, size = 540, y = 712, spacing = -18, extra = "" }) =>
  `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" width="1024" height="1024">
  <!-- Draft: Shoebox as an element tile, "Sb". Generated by make.mjs. -->
  <defs>
    <linearGradient id="t" x1="0" y1="0" x2="0.85" y2="1">${tile.map((c, k) => `<stop offset="${[0, 45, 100][k]}%" stop-color="${c}"/>`).join("")}</linearGradient>
    <linearGradient id="e" x1="0" y1="0" x2="1" y2="1">${edge.map(([c, o], k) => `<stop offset="${[0, 50, 100][k]}%" stop-color="${c}" stop-opacity="${o}"/>`).join("")}</linearGradient>
    <linearGradient id="silver" x1="0.1" y1="0" x2="0.9" y2="1">
      <stop offset="0%" stop-color="#ffffff"/><stop offset="35%" stop-color="#e8ecf1"/><stop offset="55%" stop-color="#a8b2bd"/><stop offset="75%" stop-color="#e4e9ee"/><stop offset="100%" stop-color="#9aa4af"/>
    </linearGradient>
  </defs>
  <rect x="72" y="72" width="880" height="880" rx="150" fill="url(#t)"/>
  <rect x="72" y="72" width="880" height="880" rx="150" fill="none" stroke="url(#e)" stroke-width="10"/>
  ${extra}
  <text x="512" y="${y}" text-anchor="middle" font-family="${font}" font-size="${size}" font-weight="700" fill="${ink}" letter-spacing="${spacing}">Sb</text>
</svg>
`;
const AG_TILE = ["#3a3f46", "#23262b", "#15171a"], AG_EDGE = [["#8e979f", 0.9], ["#5a6169", 0.35], ["#c9d1d9", 0.55]];
const RED_TILE = ["#b8503f", "#8e302a", "#561a15"], RED_EDGE = [["#e09a88", 0.9], ["#7a2a22", 0.35], ["#d98b78", 0.6]];
const SB = {
  // Argentum's twin: same tile, same silver.
  twin: { tile: AG_TILE, edge: AG_EDGE, ink: "url(#silver)", font: SANS },
  // The box's oxblood with the silver lid's letters.
  red: { tile: RED_TILE, edge: RED_EDGE, ink: "url(#silver)", font: SANS },
  // Oxblood with cream serif letters, like an old box label.
  label: { tile: RED_TILE, edge: RED_EDGE, ink: "#f1e6cc", font: SERIF, size: 560, y: 716, spacing: -8 },
  // Aged paper with ink serif letters.
  paper: { tile: ["#f3e8cf", "#e4d2aa", "#c9b083"], edge: [["#fff8e6", 0.9], ["#a88c5c", 0.4], ["#8a6a3a", 0.6]], ink: INK, font: SERIF, size: 560, y: 716, spacing: -8 },
};
for (const [k, o] of Object.entries(SB)) writeFileSync(`drafts/sb-${k}.svg`, sbTile(o));

// The app icon: Sb, oxblood and silver. The box is the
// logo everywhere else. Its PNGs come from this machine's Segoe UI.
writeFileSync("app-icon.svg", sbTile(SB.red).replace("Draft: Shoebox as an element tile", "Shoebox app icon: element tile"));

// Colour drafts (2026-10-13): the open box with old photos, in each box colour.
for (const box of Object.keys(BOXES)) {
  writeFileSync(`drafts/colour-${box}.svg`, onTile(lidOff(box, PHOTOS), `Draft: open box, old photos, ${box} box. Generated by make.mjs.`));
  for (const n of Object.keys(SMALL)) writeFileSync(`drafts/colour-${box}-${n}.svg`, small(+n, box));
}

console.log("ok");
