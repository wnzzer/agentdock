// Regenerates data/prices.json from LiteLLM's price list. Run by
// .github/workflows/prices-data.yml; it writes nothing when the prices are
// unchanged, and exits non-zero without writing when the result fails a check.
//
//   node scripts/prices/generate.mjs [path-to-a-downloaded-litellm-json]
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { build, nextFile, problems } from "./lib.mjs";

const SOURCE = "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
const root = fileURLToPath(new URL("../../", import.meta.url));
const output = `${root}data/prices.json`;
const overrides = JSON.parse(readFileSync(`${root}data/prices-overrides.json`, "utf8"));

const local = process.argv[2];
const litellm = local
  ? JSON.parse(readFileSync(local, "utf8"))
  : await fetch(SOURCE).then(response => {
    if (!response.ok) throw new Error(`LiteLLM answered ${response.status}`);
    return response.json();
  });

const current = existsSync(output) ? JSON.parse(readFileSync(output, "utf8")) : undefined;
const models = build(litellm, overrides);
const found = problems(models, { required: overrides.required, previous: current?.models });
if (found.length) {
  console.error(`Not publishing prices:\n  ${found.join("\n  ")}`);
  process.exit(1);
}
const file = nextFile(models, current, new Date());
if (!file) {
  console.log(`prices unchanged (${Object.keys(models).length} models)`);
} else {
  writeFileSync(output, `${JSON.stringify(file, null, 2)}\n`);
  console.log(`wrote ${Object.keys(models).length} models, updated ${file.updatedAt}`);
}
