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
  "pie-empty",
  "treemap-short",
  "c4-long",
  "treemap-numbers",
  "pie-palette",
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
          "pie-empty",
          "treemap-short",
          "c4-long",
          "treemap-numbers",
          "pie-palette",
          "treemap-wide",
        ];
        const inputs =
          kind === "treemap-palette"
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
                        !e.closest("defs") &&
                        getComputedStyle(e).fill !== "none",
                    );
                  let background = bg;
                  for (const e of layers.reverse()) {
                    const style = getComputedStyle(e);
                    background = composite(
                      rgb(style.fill),
                      background,
                      +style.fillOpacity * +style.opacity * alpha(style.fill),
                    );
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
                const adjacent =
                  s.fill === "none"
                    ? [bg, ...fills]
                    : [
                        rgb(s.fill).map(
                          (v, i) =>
                            v * +s.fillOpacity + bg[i] * (1 - +s.fillOpacity),
                        ),
                      ];
                return {
                  element: e.tagName,
                  contrast: Math.max(
                    ...adjacent.map((fill) => ratio(stroke, fill)),
                  ),
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
              labels.includes("[ContainerQueue]"),
            "Database and queue tags that fit must remain complete",
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
