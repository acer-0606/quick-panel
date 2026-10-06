// Build a self-contained runtime, failing closed on provider/auth/CLI inclusion.
import { build } from "esbuild";
import { writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
const root = fileURLToPath(new URL("../", import.meta.url));
const result = await build({ absWorkingDir: root, entryPoints: ["src/worker.mjs"], outfile: "dist/worker.mjs", bundle: true, platform: "node", target: "node24", format: "esm", minify: true, metafile: true });
const output = Object.values(result.metafile.outputs)[0];
for (const [path, input] of Object.entries(output.inputs)) {
  if (!input.bytesInOutput || path === "src/worker.mjs") continue;
  const allowed = path.includes("/typebox/") || /\/pi-agent-core\/dist\/(agent|agent-loop|stream-fn)\.js$/.test(path) || /\/pi-ai\/dist\/utils\/(event-stream|text|transcript|validation)\.js$/.test(path);
  if (!allowed) throw new Error(`Unexpected emitted runtime dependency: ${path}`);
}
for (const entry of output.imports) if (entry.external && entry.path !== "node:crypto") throw new Error(`Unexpected external dependency: ${entry.path}`);
await writeFile(new URL("../dist/metafile.json", import.meta.url), JSON.stringify(result.metafile, null, 2) + "\n");
console.log(`Pi worker built: ${output.bytes} bytes`);
