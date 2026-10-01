// Copyright (c) 2026 Joel Baumert. All Rights Reserved.
// Exercise the actual CLI PNG path, with renderer-matched glyph/background masks.
const fs = require("node:fs");
const path = require("node:path");
const assert = require("node:assert/strict");
const { execFileSync, spawnSync } = require("node:child_process");
const os = require("node:os");
const { chromium } = require("playwright");
const root = path.resolve(__dirname, "../..");
const outputIndex = process.argv.indexOf("--out");
const out = path.resolve(
  outputIndex >= 0 ? process.argv[outputIndex + 1] : "/tmp/mermaid-png",
);
fs.mkdirSync(out, { recursive: true });
const cli =
  process.env.MERMAID_CLI || path.join(root, "target/debug/mermaid_cli");
const cases = [
  ...["treemap", "pie", "xy-chart", "quadrant-chart", "architecture", "c4"].map(
    (x) => `golden/${x}`,
  ),
  ...[
    "c4",
    "c4-long",
    "c4-narrow",
    "c4-hangul",
    "c4-fallback",
    "c4-indic",
    "c4-tall-queue",
    "c4-tall-wide",
    "treemap-fallback-leaf",
    "treemap-fallback-header",
    "architecture-accent",
    "architecture-wide",
    "c4-unicode",
    "c4-arabic",
    "c4-fitting",
    "treemap-wide",
    "treemap-numbers",
    "treemap-leaf-short",
    "treemap-wide-m",
    "pie-thin",
    "quadrant-wide",
    "quadrant-marker-overlap",
    "quadrant-ring-axis",
    "quadrant-marker-covered",
    "quadrant-literal",
  ].map((x) => `fixtures/contrast-${x}`),
];
const cliEnv = {
  ...process.env,
  ...(process.env.MERMAID_CLI_PATH
    ? { PATH: process.env.MERMAID_CLI_PATH }
    : {}),
};
function raster(svg, filename) {
  if (process.env.REFERENCE_MAGICK) {
    fs.writeFileSync(filename + ".svg", svg);
    execFileSync(
      process.env.REFERENCE_MAGICK,
      ["-density", "192", filename + ".svg", filename],
      { env: cliEnv },
    );
  } else
    execFileSync(
      "rsvg-convert",
      ["--dpi-x", "192", "--dpi-y", "192", "--zoom", "2", "--output", filename],
      { input: svg },
    );
  return (
    "data:image/png;base64," + fs.readFileSync(filename).toString("base64")
  );
}
(async () => {
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROME_BIN
      ? { executablePath: process.env.CHROME_BIN }
      : {}),
  });
  const page = await browser.newPage();
  try {
    for (const theme of ["light", "dark"])
      for (const input of cases) {
        const name = theme + "-" + path.basename(input),
          base = path.join(out, name);
        const source = path.join(root, "crates/mermaid/tests", input + ".mmd");
        const svg = execFileSync(cli, ["--theme", theme, source], {
          encoding: "utf8",
          env: cliEnv,
        });
        execFileSync(cli, ["--theme", theme, source, "--out", base + ".png"], {
          env: cliEnv,
        });
        await page.setContent(svg);
        const variants = await page.evaluate(() => {
          const root = document.querySelector("svg");
          const texts = [...root.querySelectorAll("text")].filter(
            (t) =>
              t.textContent.trim() &&
              !t.closest("defs") &&
              !t.hasAttribute("data-label-underlay"),
          );
          const xml = (e) => new XMLSerializer().serializeToString(e);
          const bg = root.cloneNode(true);
          bg.querySelectorAll("text").forEach((t) => {
            if (t.closest("defs") || t.hasAttribute("data-label-underlay"))
              return;
            if (t.getAttribute("stroke") && t.getAttribute("stroke") !== "none")
              t.setAttribute("fill", "none");
            else t.remove();
          });
          return {
            markers: [...root.querySelectorAll("circle")]
              .filter((circle) => getComputedStyle(circle).fill !== "none")
              .map((circle) => ({
                x: +circle.getAttribute("cx"),
                y: +circle.getAttribute("cy"),
                radius: +(
                  circle.getAttribute("data-indicator-radius") ||
                  circle.getAttribute("r")
                ),
                color: getComputedStyle(circle).fill,
              })),
            arrows: [...root.querySelectorAll("path[marker-end]")]
              .filter((e) => getComputedStyle(e).stroke !== "none")
              .map((e) => {
                const point = e.getPointAtLength(e.getTotalLength());
                return {
                  x: point.x,
                  y: point.y,
                  color: getComputedStyle(e).stroke,
                };
              }),
            background: xml(bg),
            labels: texts.map((text, index) => {
              const mask = root.cloneNode(true);
              // Do not let the expected mask repeat a clipping defect.
              mask
                .querySelectorAll("svg")
                .forEach((e) =>
                  e.style.setProperty("overflow", "visible", "important"),
                );
              mask
                .querySelectorAll(
                  "rect,path,circle,ellipse,line,polyline,polygon,use,image",
                )
                .forEach((e) => {
                  if (!e.closest("defs"))
                    e.setAttribute("visibility", "hidden");
                });
              mask
                .querySelectorAll("marker")
                .forEach((e) => e.setAttribute("display", "none"));
              mask
                .querySelectorAll("text[data-label-underlay]")
                .forEach((t) => t.setAttribute("visibility", "hidden"));
              [...mask.querySelectorAll("text")]
                .filter(
                  (t) =>
                    t.textContent.trim() &&
                    !t.closest("defs") &&
                    !t.hasAttribute("data-label-underlay"),
                )
                .forEach((t, i) => {
                  if (i !== index) t.setAttribute("visibility", "hidden");
                  else {
                    t.setAttribute("fill", "#000");
                    t.setAttribute("stroke", "none");
                    t.removeAttribute("opacity");
                    t.removeAttribute("fill-opacity");
                  }
                });
              return {
                text: text.textContent,
                fill: getComputedStyle(text).fill,
                opacity: +getComputedStyle(text).opacity,
                svg: xml(mask),
              };
            }),
          };
        });
        const ink =
          "data:image/png;base64," +
          fs.readFileSync(base + ".png").toString("base64");
        const background = raster(
          variants.background,
          base + "-background.png",
        );
        const labels = variants.labels.map((label, i) => ({
          ...label,
          mask: raster(label.svg, base + `-mask-${i}.png`),
        }));
        const results = await page.evaluate(
          async ({ ink, background, labels, arrows, markers }) => {
            async function decode(src) {
              const image = new Image();
              image.src = src;
              await image.decode();
              const canvas = document.createElement("canvas");
              canvas.width = image.width;
              canvas.height = image.height;
              const ctx = canvas.getContext("2d");
              ctx.drawImage(image, 0, 0);
              return {
                width: image.width,
                height: image.height,
                pixels: ctx.getImageData(0, 0, image.width, image.height).data,
              };
            }
            const actual = await decode(ink),
              bg = await decode(background);
            const lum = (c) =>
              c
                .map((v) => {
                  v /= 255;
                  return v <= 0.04045
                    ? v / 12.92
                    : ((v + 0.055) / 1.055) ** 2.4;
                })
                .reduce((s, v, i) => s + v * [0.2126, 0.7152, 0.0722][i], 0);
            const results = [];
            for (const label of labels) {
              const mask = await decode(label.mask);
              if (
                mask.width !== actual.width ||
                mask.height !== actual.height ||
                bg.width !== actual.width ||
                bg.height !== actual.height
              )
                throw new Error("Raster dimensions disagree");
              const color = label.fill
                .match(/[\d.]+/g)
                .slice(0, 3)
                .map(Number);
              let count = 0,
                visible = 0,
                contrast = Infinity;
              for (let o = 0; o < mask.pixels.length; o += 4) {
                if (mask.pixels[o + 3] < 200) continue;
                count++;
                const behind = [...bg.pixels.slice(o, o + 3)];
                const delta = color.map(
                  (v, i) => (v - behind[i]) * label.opacity,
                );
                const norm = delta.reduce((s, v) => s + v * v, 0);
                const contribution = norm
                  ? delta.reduce(
                      (s, v, i) => s + v * (actual.pixels[o + i] - behind[i]),
                      0,
                    ) / norm
                  : 0;
                if (contribution >= 0.65) visible++;
                const foreground = behind.map((v, i) => v + delta[i]);
                const l = [lum(foreground), lum(behind)].sort((a, b) => a - b);
                contrast = Math.min(contrast, (l[1] + 0.05) / (l[0] + 0.05));
              }
              results.push({
                text: label.text,
                count,
                visibleFraction: visible / count,
                contrast,
              });
            }
            for (const marker of markers) {
              const color = marker.color
                .match(/[\d.]+/g)
                .slice(0, 3)
                .map(Number);
              let visible = 0;
              const radius = Math.ceil(marker.radius * 2);
              for (let dy = -radius; dy <= radius; dy++)
                for (let dx = -radius; dx <= radius; dx++) {
                  const x = Math.floor(marker.x * 2) + dx,
                    y = Math.floor(marker.y * 2) + dy;
                  if (x < 0 || y < 0 || x >= actual.width || y >= actual.height)
                    continue;
                  const offset = (y * actual.width + x) * 4;
                  if (
                    color.every(
                      (v, i) => Math.abs(v - actual.pixels[offset + i]) <= 4,
                    )
                  )
                    visible++;
                }
              if (!visible) throw new Error("PNG point marker hidden");
            }
            for (const arrow of arrows) {
              const color = arrow.color
                .match(/[\d.]+/g)
                .slice(0, 3)
                .map(Number);
              let visible = 0;
              // CLI exports preserve 2x dimensions; sample the marker core.
              for (let dy = -4; dy <= 4; dy++)
                for (let dx = -4; dx <= 4; dx++) {
                  const x = Math.floor(arrow.x * 2) + dx,
                    y = Math.floor(arrow.y * 2) + dy;
                  if (x < 0 || y < 0 || x >= actual.width || y >= actual.height)
                    continue;
                  const offset = (y * actual.width + x) * 4;
                  if (
                    color.every(
                      (v, i) => Math.abs(v - actual.pixels[offset + i]) <= 4,
                    )
                  )
                    visible++;
                }
              if (visible < 3)
                throw new Error(`PNG arrow tip hidden: ${visible} pixels`);
            }
            return results;
          },
          {
            ink,
            background,
            labels,
            arrows: input.includes("architecture") ? variants.arrows : [],
            markers: input.includes("quadrant") ? variants.markers : [],
          },
        );
        console.log(JSON.stringify({ theme, input, results }));
        for (const result of results) {
          assert(result.count > 0, `${name}: no glyphs for ${result.text}`);
          assert(
            result.visibleFraction >= 0.99,
            `${name}: missing label ${JSON.stringify(result)}`,
          );
          assert(
            result.contrast >= 4.5,
            `${name}: contrast ${JSON.stringify(result)}`,
          );
        }
      }
    if (process.env.MERMAID_CLI_PATH) {
      const temporary = fs.mkdtempSync(
        path.join(os.tmpdir(), "mermaid-png-errors-"),
      );
      try {
        const result = spawnSync(
          cli,
          [
            path.join(root, "crates/mermaid/tests/fixtures/contrast-c4.mmd"),
            "--out",
            path.join(temporary, "missing", "diagram.png"),
          ],
          { env: { ...cliEnv, TMPDIR: temporary }, encoding: "utf8" },
        );
        assert.equal(
          result.status,
          1,
          "Failed PNG export must report an error",
        );
        assert.equal(
          fs.readdirSync(temporary).filter((name) => name.endsWith(".svg"))
            .length,
          0,
          "Failed export must remove temporary SVGs",
        );
        console.log("PNG export failure cleanup: PASS");
      } finally {
        fs.rmSync(temporary, { recursive: true, force: true });
      }
    }
  } finally {
    await browser.close();
  }
})().catch((e) => {
  console.error(e);
  process.exitCode = 1;
});
