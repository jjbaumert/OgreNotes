// Copyright (c) 2026 Joel Baumert. All Rights Reserved.
// Rendered SVG contrast regression for #287, using the production theme CSS.
const fs = require("node:fs");
const path = require("node:path");
const assert = require("node:assert/strict");
const root = path.resolve(__dirname, "../..");
const args = process.argv.slice(2);
const out = args.includes("--out")
  ? args[args.indexOf("--out") + 1]
  : "/tmp/ogrenotes-mermaid-contrast";
fs.mkdirSync(out, { recursive: true });
const { chromium } = require("playwright");
const kinds = [
  "treemap",
  "pie",
  "xy-chart",
  "quadrant-chart",
  "architecture",
  "c4",
  "quadrant-edges",
  "quadrant-literal",
  "quadrant-marker-overlap",
  "pie-empty",
  "treemap-short",
  "c4-long",
  "c4-unicode",
  "treemap-numbers",
  "pie-palette",
  "pie-thin",
  "pie-adjacent",
  "treemap-palette",
  "treemap-wide",
];
(async () => {
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROME_BIN
      ? { executablePath: process.env.CHROME_BIN }
      : {}),
  });
  const page = await browser.newPage({
    viewport: { width: 1800, height: 2400 },
  });
  let failed = false;
  try {
    for (const theme of ["light", "dark"])
      for (const kind of kinds) {
        const fixtures = [
          "c4",
          "quadrant-edges",
          "quadrant-literal",
          "quadrant-marker-overlap",
          "pie-empty",
          "treemap-short",
          "c4-long",
          "c4-unicode",
          "treemap-numbers",
          "pie-palette",
          "pie-thin",
          "pie-adjacent",
          "treemap-wide",
        ];
        const inputs =
          kind === "c4"
            ? [
                "crates/mermaid/tests/fixtures/contrast-c4.svg",
                "crates/mermaid/tests/golden/c4.svg",
              ]
            : kind === "treemap-palette"
              ? Array.from(
                  { length: 8 },
                  (_, depth) =>
                    `crates/mermaid/tests/fixtures/contrast-treemap-depth-${depth}.svg`,
                )
              : [
                  fixtures.includes(kind)
                    ? `crates/mermaid/tests/fixtures/contrast-${kind}.svg`
                    : `crates/mermaid/tests/golden/${kind}.svg`,
                ];
        const svg = inputs
          .map((input) => fs.readFileSync(path.join(root, input), "utf8"))
          .join("");

        const css = [
          "frontend/style/tokens-light.css",
          "frontend/style/tokens-dark.css",
        ]
          .map((p) => fs.readFileSync(path.join(root, p), "utf8"))
          .join("\n");
        const sizing = fs
          .readFileSync(path.join(root, "frontend/style/main.css"), "utf8")
          .match(/\.mermaid-svg svg\s*\{[^}]*\}/)?.[0];
        assert(sizing, "Production Mermaid SVG sizing rule must exist");
        await page.setContent(
          `<html data-theme="${theme}"><style>${css}\n${sizing}\nbody{color:var(--color-text);background:var(--color-surface);margin:20px}svg{display:block}</style><div class="mermaid-svg" style="${kind === "treemap-palette" ? "display:grid;grid-template-columns:repeat(2,1fr)" : ""}">${svg}</div></html>`,
        );
        const tokens = await page.evaluate(() => {
          const root = getComputedStyle(document.documentElement);
          const body = getComputedStyle(document.body);
          return {
            text: root.getPropertyValue("--color-text").trim(),
            surface: root.getPropertyValue("--color-surface").trim(),
            foreground: body.color,
            background: body.backgroundColor,
          };
        });
        assert(
          tokens.text && tokens.surface,
          "Production document colors must be defined",
        );
        assert.equal(
          tokens.foreground,
          theme === "dark" ? "rgb(232, 232, 232)" : "rgb(26, 26, 26)",
        );
        assert.equal(
          tokens.background,
          theme === "dark" ? "rgb(42, 42, 42)" : "rgb(255, 255, 255)",
        );
        if (kind === "quadrant-marker-overlap") {
          // Label/point overlap predates this contrast fix and its positions
          // remain outside scope. This case checks the introduced regression:
          // plot-stroke treatment must not erase a neighboring point marker.
          const marker = await page.evaluate(async () => {
            const root = document.querySelector("svg"),
              clone = root.cloneNode(true);
            const originals = [root, ...root.querySelectorAll("*")],
              copies = [clone, ...clone.querySelectorAll("*")];
            originals.forEach((e, i) => {
              const computed = getComputedStyle(e);
              for (const property of [
                "fill",
                "stroke",
                "font-family",
                "font-size",
                "fill-opacity",
                "stroke-opacity",
                "opacity",
              ])
                copies[i].style.setProperty(
                  property,
                  computed.getPropertyValue(property),
                );
            });
            const circle = root.querySelectorAll("circle")[1],
              point = new DOMPoint(
                +circle.getAttribute("cx"),
                +circle.getAttribute("cy"),
              ).matrixTransform(circle.getScreenCTM());
            const bounds = root.getBoundingClientRect(),
              width = Math.ceil(bounds.width),
              height = Math.ceil(bounds.height);
            clone.setAttribute("width", width);
            clone.setAttribute("height", height);
            clone.style.cssText += `;width:${width}px;height:${height}px;max-width:none`;
            const url = URL.createObjectURL(
              new Blob([new XMLSerializer().serializeToString(clone)], {
                type: "image/svg+xml",
              }),
            );
            try {
              const image = new Image();
              image.src = url;
              await image.decode();
              const canvas = document.createElement("canvas");
              canvas.width = width;
              canvas.height = height;
              const ctx = canvas.getContext("2d");
              ctx.drawImage(image, 0, 0);
              const data = ctx.getImageData(0, 0, width, height).data,
                color = getComputedStyle(circle)
                  .fill.match(/[\d.]+/g)
                  .slice(0, 3)
                  .map(Number);
              let visible = 0;
              for (let dy = -5; dy <= 5; dy++)
                for (let dx = -5; dx <= 5; dx++) {
                  const x = Math.floor(point.x - bounds.x) + dx,
                    y = Math.floor(point.y - bounds.y) + dy,
                    offset = (y * width + x) * 4;
                  if (
                    color.every((v, i) => Math.abs(v - data[offset + i]) <= 3)
                  )
                    visible++;
                }
              return { visible, color };
            } finally {
              URL.revokeObjectURL(url);
            }
          });
          console.log(JSON.stringify({ theme, kind, marker }));
          assert(
            marker.visible >= 40,
            "Neighboring point marker must remain visible",
          );
          await page.screenshot({
            path: path.join(out, `${kind}-${theme}.png`),
          });
          continue;
        }
        const samples = await page.evaluate(() => {
          const rgb = (s) =>
            s
              .match(/[\d.]+/g)
              .slice(0, 3)
              .map(Number);
          const alpha = (s) =>
            s.startsWith("rgba") ? +s.match(/[\d.]+/g)[3] : 1;
          const composite = (a, b, alpha) =>
            a.map((v, i) => v * alpha + b[i] * (1 - alpha));
          const luminance = (c) =>
            c
              .map((x) => {
                x /= 255;
                return x <= 0.04045 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4;
              })
              .reduce((s, x, i) => s + x * [0.2126, 0.7152, 0.0722][i], 0);
          const bg = rgb(getComputedStyle(document.body).backgroundColor);
          return [...document.querySelectorAll("svg text")]
            .filter((t) => t.textContent.trim())
            .flatMap((t) => {
              const box = t.getBoundingClientRect();
              return [0.1, 0.5, 0.9].flatMap((fx) =>
                [0.1, 0.5, 0.9].map((fy) => {
                  const x = box.x + box.width * fx,
                    y = box.y + box.height * fy;
                  const viewportSvg = t.closest("svg");
                  const vb = viewportSvg.viewBox.baseVal;
                  const matrix = viewportSvg.getScreenCTM();
                  const start = new DOMPoint(vb.x, vb.y).matrixTransform(
                    matrix,
                  );
                  const end = new DOMPoint(
                    vb.x + vb.width,
                    vb.y + vb.height,
                  ).matrixTransform(matrix);
                  const viewport = {
                    left: start.x,
                    top: start.y,
                    right: end.x,
                    bottom: end.y,
                  };
                  if (
                    x < viewport.left ||
                    x > viewport.right ||
                    y < viewport.top ||
                    y > viewport.bottom
                  )
                    return null;
                  const layers = document
                    .elementsFromPoint(x, y)
                    .filter(
                      (e) =>
                        e !== t &&
                        [
                          "rect",
                          "path",
                          "circle",
                          "polygon",
                          "ellipse",
                        ].includes(e.tagName) &&
                        !e.closest("defs"),
                    );
                  let background = bg;
                  for (const e of layers.reverse()) {
                    const style = getComputedStyle(e);
                    if (style.fill !== "none") {
                      background = composite(
                        rgb(style.fill),
                        background,
                        +style.fillOpacity * +style.opacity * alpha(style.fill),
                      );
                    }
                  }
                  const style = getComputedStyle(t);
                  const foreground = composite(
                    rgb(style.fill),
                    background,
                    +style.opacity,
                  );
                  const l = [luminance(foreground), luminance(background)].sort(
                    (a, b) => a - b,
                  );
                  return {
                    text: t.textContent,
                    contrast: +((l[1] + 0.05) / (l[0] + 0.05)).toFixed(2),
                    fill: style.fill,
                    background,
                    covered: layers.length,
                  };
                }),
              );
            })
            .filter(Boolean);
        });
        if (kind.startsWith("pie")) {
          const strokes = await page.evaluate(() => {
            const rgb = (s) =>
              s
                .match(/[\d.]+/g)
                .slice(0, 3)
                .map(Number);
            const lum = (c) =>
              c
                .map((x) => {
                  x /= 255;
                  return x <= 0.04045
                    ? x / 12.92
                    : ((x + 0.055) / 1.055) ** 2.4;
                })
                .reduce((s, x, i) => s + x * [0.2126, 0.7152, 0.0722][i], 0);
            const ratio = (a, b) => {
              const l = [lum(a), lum(b)].sort((a, b) => a - b);
              return (l[1] + 0.05) / (l[0] + 0.05);
            };
            const bg = rgb(getComputedStyle(document.body).backgroundColor);
            const wedges = [
              ...document.querySelectorAll("svg path, svg circle"),
            ].filter((e) => getComputedStyle(e).fill !== "none");
            const fills = wedges.map((e) => {
              const s = getComputedStyle(e);
              return rgb(s.fill).map(
                (v, i) => v * +s.fillOpacity + bg[i] * (1 - +s.fillOpacity),
              );
            });
            return [...document.querySelectorAll("svg path, svg circle")].map(
              (e) => {
                const s = getComputedStyle(e);
                const stroke = rgb(s.stroke);
                // Every rim segment must contrast with its own wedge or the
                // canvas. One bright wedge cannot make the whole rim pass.
                const contrast =
                  s.fill === "none"
                    ? fills.length
                      ? Math.min(
                          ...fills.map((fill) =>
                            Math.max(ratio(stroke, bg), ratio(stroke, fill)),
                          ),
                        )
                      : ratio(stroke, bg)
                    : ratio(
                        stroke,
                        rgb(s.fill).map(
                          (v, i) =>
                            v * +s.fillOpacity + bg[i] * (1 - +s.fillOpacity),
                        ),
                      );
                return {
                  element: e.tagName,
                  contrast,
                  width: parseFloat(s.strokeWidth),
                  opacity: +s.strokeOpacity * +s.opacity,
                };
              },
            );
          });
          assert(strokes.length > 0, "Pie must have an outline");
          failed ||= strokes.some(
            (s) => s.contrast < 3 || s.width < 1 || s.opacity < 1,
          );
          console.log(JSON.stringify({ theme, kind, strokes }));
        }
        {
          // Rasterize glyph coverage and the SVG without text. Sample glyph
          // interiors using the declared foreground, excluding unrelated lines.
          const pixels = await page.evaluate(async (kind) => {
            const results = [];
            for (const root of document.querySelectorAll(
              ".mermaid-svg > svg",
            )) {
              const bounds = root.getBoundingClientRect();
              const clone = root.cloneNode(true);
              const originals = [root, ...root.querySelectorAll("*")];
              const copies = [clone, ...clone.querySelectorAll("*")];
              const properties = [
                "fill",
                "stroke",
                "fill-opacity",
                "stroke-opacity",
                "opacity",
                "font-family",
                "font-size",
                "font-weight",
                "text-anchor",
                "dominant-baseline",
                "paint-order",
              ];
              originals.forEach((e, i) => {
                const computed = getComputedStyle(e);
                for (const property of properties) {
                  copies[i].style.setProperty(
                    property,
                    computed.getPropertyValue(property),
                  );
                }
              });
              const width = Math.ceil(bounds.width),
                height = Math.ceil(bounds.height);
              clone.setAttribute("width", width);
              clone.setAttribute("height", height);
              clone.style.cssText += `;width:${width}px;height:${height}px;max-width:none`;
              const render = async (imageSvg = clone, transparent = false) => {
                const url = URL.createObjectURL(
                  new Blob([new XMLSerializer().serializeToString(imageSvg)], {
                    type: "image/svg+xml",
                  }),
                );
                try {
                  const image = new Image();
                  image.src = url;
                  await image.decode();
                  const canvas = document.createElement("canvas");
                  canvas.width = width;
                  canvas.height = height;
                  const ctx = canvas.getContext("2d");
                  if (!transparent) {
                    ctx.fillStyle = getComputedStyle(
                      document.body,
                    ).backgroundColor;
                    ctx.fillRect(0, 0, width, height);
                  }
                  ctx.drawImage(image, 0, 0);
                  return ctx.getImageData(0, 0, width, height).data;
                } finally {
                  URL.revokeObjectURL(url);
                }
              };
              // A glyph-only mask excludes strokes that happen to have the
              // same color as text within its bounding box.
              const mask = clone.cloneNode(true);
              mask
                .querySelectorAll(
                  "rect,path,circle,ellipse,line,polyline,polygon,use,image",
                )
                .forEach((e) => {
                  if (!e.closest("defs")) e.style.visibility = "hidden";
                });
              mask.querySelectorAll("text").forEach((e) => {
                e.style.fill = "#000";
                e.style.stroke = "none";
              });
              // Markers remain paintable even when their referring path is
              // hidden. They must not be mistaken for glyphs in this mask.
              mask
                .querySelectorAll("marker")
                .forEach((e) => (e.style.display = "none"));
              const glyphs = await render(mask, true);
              const ink = await render();
              clone.querySelectorAll("text").forEach((e) => {
                // Keep a glyph halo as background paint while removing its
                // foreground fill. Rectangular backings remain untouched.
                if (
                  getComputedStyle(originals[copies.indexOf(e)]).stroke !==
                  "none"
                )
                  e.style.fill = "none";
                else e.remove();
              });
              const background = await render();
              const luminance = (c) =>
                c
                  .map((v) => {
                    v /= 255;
                    return v <= 0.04045
                      ? v / 12.92
                      : ((v + 0.055) / 1.055) ** 2.4;
                  })
                  .reduce(
                    (sum, v, i) => sum + v * [0.2126, 0.7152, 0.0722][i],
                    0,
                  );
              results.push(
                ...[...root.querySelectorAll("text")]
                  .filter((t) => t.textContent.trim())
                  .map((t) => {
                    const box = t.getBoundingClientRect();
                    const style = getComputedStyle(t);
                    const color = style.fill
                      .match(/[\d.]+/g)
                      .slice(0, 3)
                      .map(Number);
                    const paintAlpha =
                      +style.opacity *
                      +style.fillOpacity *
                      (style.fill.startsWith("rgba")
                        ? +style.fill.match(/[\d.]+/g)[3]
                        : 1);
                    let minimum = Infinity,
                      count = 0,
                      visible = 0;
                    for (
                      let y = Math.floor(box.top - bounds.top);
                      y < Math.ceil(box.bottom - bounds.top);
                      y++
                    ) {
                      for (
                        let x = Math.floor(box.left - bounds.left);
                        x < Math.ceil(box.right - bounds.left);
                        x++
                      ) {
                        if (x < 0 || y < 0 || x >= width || y >= height)
                          continue;
                        const offset = (y * width + x) * 4;
                        if (glyphs[offset + 3] < 128) continue;
                        const behind = Array.from(
                          background.slice(offset, offset + 3),
                        );
                        const delta = color.map((v, i) => v - behind[i]);
                        const norm = delta.reduce((sum, v) => sum + v * v, 0);
                        const contribution =
                          norm > 0
                            ? delta.reduce(
                                (sum, v, i) =>
                                  sum + v * (ink[offset + i] - behind[i]),
                                0,
                              ) / norm
                            : 0;
                        if (contribution >= (glyphs[offset + 3] / 255) * 0.8)
                          visible++;
                        const foreground = color.map(
                          (v, i) =>
                            v * paintAlpha + behind[i] * (1 - paintAlpha),
                        );
                        const l = [
                          luminance(foreground),
                          luminance(behind),
                        ].sort((a, b) => a - b);
                        minimum = Math.min(
                          minimum,
                          (l[1] + 0.05) / (l[0] + 0.05),
                        );
                        count++;
                      }
                    }
                    return {
                      text: t.textContent,
                      count,
                      visibleFraction: visible / count,
                      contrast: minimum,
                    };
                  }),
              );
              if (kind === "architecture") {
                const edge = root.querySelector("path[marker-end]");
                const end = edge.getPointAtLength(edge.getTotalLength());
                const screen = new DOMPoint(end.x, end.y).matrixTransform(
                  edge.getScreenCTM(),
                );
                const color = getComputedStyle(edge)
                  .stroke.match(/[\d.]+/g)
                  .slice(0, 3)
                  .map(Number);
                let tipPixels = 0;
                for (let dy = -2; dy <= 2; dy++)
                  for (let dx = -2; dx <= 2; dx++) {
                    const x = Math.floor(screen.x - bounds.x) + dx,
                      y = Math.floor(screen.y - bounds.y) + dy;
                    const offset = (y * width + x) * 4;
                    if (
                      color.every((v, i) => Math.abs(v - ink[offset + i]) <= 4)
                    )
                      tipPixels++;
                  }
                if (tipPixels < 3)
                  throw new Error(
                    `Architecture arrow tip hidden: ${tipPixels} pixels`,
                  );
                results[results.length - 1].arrowTipPixels = tipPixels;
              }
            }
            return results;
          }, kind);
          console.log(JSON.stringify({ theme, kind, pixels }));
          assert(
            pixels.length > 0 &&
              pixels.every(
                (p) =>
                  p.count > 0 && p.contrast >= 4.5 && p.visibleFraction >= 0.9,
              ),
            "Glyphs must remain visible and contrast with actual background pixels, including borders",
          );
        }
        if (kind === "c4-long") {
          const clipping = await page.evaluate(() => {
            const text = [...document.querySelectorAll("svg text")].find((t) =>
              t.textContent.includes("WWW"),
            );
            const frame = text.closest("svg");
            text.style.fontSize = "36px";
            const box = text.getBoundingClientRect();
            const vb = frame.viewBox.baseVal,
              matrix = frame.getScreenCTM();
            const start = new DOMPoint(vb.x, vb.y).matrixTransform(matrix);
            const end = new DOMPoint(
              vb.x + vb.width,
              vb.y + vb.height,
            ).matrixTransform(matrix);
            const bounds = {
              right: end.x,
              top: start.y,
              width: end.x - start.x,
              height: end.y - start.y,
            };
            const point = [bounds.right + 2, bounds.top + bounds.height / 2];
            frame.style.overflow = "visible";
            const escaped = document.elementsFromPoint(...point).includes(text);
            frame.style.overflow = "hidden";
            const kept = document.elementsFromPoint(...point).includes(text);
            text.style.fontSize = "";
            return {
              width: box.width,
              frameWidth: bounds.width,
              escaped,
              kept,
            };
          });
          console.log(JSON.stringify({ theme, kind, clipping }));
          assert(
            clipping.width > clipping.frameWidth &&
              clipping.escaped &&
              !clipping.kept,
            "Different font metrics must remain inside their painted bounds",
          );
        }
        if (kind === "c4") {
          const labels = await page.locator("svg text").allTextContents();
          assert(
            labels.includes("[ContainerDb]") &&
              labels.includes("[ContainerQueue]") &&
              labels.includes("EXTERNAL DB"),
            "Database and queue tags that fit must remain complete",
          );
        }
        if (kind === "c4-unicode") {
          const labels = await page.locator("svg text").allTextContents();
          for (const label of ["éééééé", "éééééé"]) {
            assert(
              labels.includes(label),
              "Fitting Unicode labels must remain complete",
            );
          }
          assert(
            labels.some((label) => /^(?:é)+…$/.test(label)),
            "Truncated Unicode labels must preserve entire grapheme clusters",
          );
        }
        assert(samples.length > 0, `${kind} must contain labels`);
        failed ||= samples.some((s) => s.contrast < 4.5);
        console.log(
          JSON.stringify({
            theme,
            kind,
            minimum: Math.min(...samples.map((s) => s.contrast)),
            failures: samples.filter((s) => s.contrast < 4.5),
          }),
        );
        await page.screenshot({ path: path.join(out, `${kind}-${theme}.png`) });
      }
    assert(
      !failed,
      "Labels must meet 4.5:1 and visible pie strokes 3:1 in both themes",
    );
  } finally {
    await browser.close();
  }
})().catch((e) => {
  console.error(e);
  process.exit(1);
});
