// Browser file plumbing for export/import: downloads, diagram images, picking and dropping files.

function save(name, blob) {
  const a = Object.assign(document.createElement('a'), { href: URL.createObjectURL(blob), download: name });
  a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 1000);
}

export function download(name, mime, text) {
  save(name, new Blob([text], { type: mime }));
}

// Styles the stylesheet gives the diagram; copied inline so the file looks the same anywhere.
// Markers stay as attributes (inlining them would bake in this page's URL).
const STYLE = ['fill', 'fill-opacity', 'stroke', 'stroke-width', 'stroke-dasharray', 'stroke-linecap', 'stroke-linejoin',
  'opacity', 'font-family', 'font-size', 'font-weight', 'text-anchor', 'dominant-baseline'];
const UI_ONLY = '.port, .rubber, .rubber-dot, .marquee, .guide';

// The diagram in the box (x, y, w, h) of model coordinates as standalone SVG markup, styles inlined.
export function svgMarkup(svg, x, y, w, h) {
  const clone = svg.cloneNode(true);
  const src = [svg, ...svg.querySelectorAll('*')];
  const dst = [clone, ...clone.querySelectorAll('*')];
  const skip = new Set(svg.querySelectorAll(UI_ONLY));
  src.forEach((el, i) => {
    if (skip.has(el)) return dst[i].remove();
    const cs = getComputedStyle(el);
    dst[i].setAttribute('style', STYLE.map(p => `${p}:${cs.getPropertyValue(p)}`).join(';'));
    dst[i].removeAttribute('class');
  });
  clone.querySelector(':scope > g[transform]')?.removeAttribute('transform');
  const bg = document.createElementNS('http://www.w3.org/2000/svg', 'rect');
  Object.entries({ x, y, width: w, height: h, fill: getComputedStyle(document.body).getPropertyValue('--canvas').trim() || '#fff' })
    .forEach(([k, v]) => bg.setAttribute(k, v));
  clone.prepend(bg);
  Object.entries({ xmlns: 'http://www.w3.org/2000/svg', viewBox: `${x} ${y} ${w} ${h}`, width: w, height: h })
    .forEach(([k, v]) => clone.setAttribute(k, v));
  return new XMLSerializer().serializeToString(clone);
}

// The diagram in the box (x, y, w, h) of model coordinates as a standalone .svg, or a 2x .png.
export function exportImage(svg, x, y, w, h, png, name) {
  const text = svgMarkup(svg, x, y, w, h);
  if (!png) return save(name, new Blob([text], { type: 'image/svg+xml' }));
  const img = new Image();
  img.onload = () => {
    const c = Object.assign(document.createElement('canvas'), { width: w * 2, height: h * 2 });
    const ctx = c.getContext('2d');
    ctx.scale(2, 2);
    ctx.drawImage(img, 0, 0, w, h);
    c.toBlob(b => save(name, b), 'image/png');
  };
  img.src = 'data:image/svg+xml;charset=utf-8,' + encodeURIComponent(text);
}

export function pickFile(accept, done) {
  const input = Object.assign(document.createElement('input'), { type: 'file', accept });
  input.onchange = async () => input.files[0] && done(await input.files[0].text());
  input.click();
}

// Dropping a .json file anywhere on the page hands its text to `done`.
export function onFileDrop(done) {
  const hasFiles = e => [...e.dataTransfer.types].includes('Files');
  addEventListener('dragover', e => hasFiles(e) && e.preventDefault());
  addEventListener('drop', async e => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    const f = [...e.dataTransfer.files].find(f => f.name.toLowerCase().endsWith('.json'));
    if (f) done(await f.text());
  });
}
