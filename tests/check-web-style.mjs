import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const testsDir = path.dirname(fileURLToPath(import.meta.url));
const root = path.dirname(testsDir);
const css = fs.readFileSync(path.join(root, "web/app.css"), "utf8");
const sourceRoot = path.join(root, "web/src");

function sourceFiles(directory) {
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const fullPath = path.join(directory, entry.name);
    if (entry.isDirectory()) return sourceFiles(fullPath);
    return entry.name.endsWith(".ts") ? [fullPath] : [];
  });
}

const typescript = sourceFiles(sourceRoot)
  .map((file) => fs.readFileSync(file, "utf8"))
  .join("\n");
const colorCount = (source) => [...source.matchAll(/#[0-9a-fA-F]{3,8}\b|rgba?\(/g)].length;
const actual = {
  css_color_literals_outside_tokens: css
    .split(/\r?\n/)
    .filter((line) => !line.includes("--"))
    .reduce((count, line) => count + colorCount(line), 0),
  typescript_color_literals: colorCount(typescript),
  backdrop_filters: (css.match(/backdrop-filter\s*:/g) || []).length,
  gradients: (css.match(/(?:linear|radial)-gradient\s*\(/g) || []).length,
  raw_svg_markup: (typescript.match(/<svg\b/g) || []).length
};
const baseline = JSON.parse(
  fs.readFileSync(path.join(testsDir, "cases/web-style-baseline.json"), "utf8")
);

const failures = Object.entries(actual).filter(([rule, count]) => count !== baseline[rule]);
if (failures.length > 0) {
  for (const [rule, count] of failures) {
    const expected = baseline[rule];
    const direction = count > expected ? "new violations" : "improved; lower the baseline";
    console.error(`${rule}: expected ${expected}, found ${count} (${direction})`);
  }
  process.exit(1);
}

console.log(`Web style ratchet passed (${Object.keys(actual).length} rules)`);
