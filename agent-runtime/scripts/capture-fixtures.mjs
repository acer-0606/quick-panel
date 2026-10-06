// Regenerate both transport directions through the real Agent production bundle.
import { spawn } from "node:child_process";
import { readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
const start = JSON.parse(await readFile(new URL("../fixtures/start.json",import.meta.url), "utf8"));
const host = [start], runtime = [];
const child = spawn(process.execPath, [fileURLToPath(new URL("../dist/worker.mjs",import.meta.url))], { stdio: ["pipe", "pipe", "inherit"] });
let buffer = "";
function send(frame) {
  host.push(frame);
  child.stdin.write(JSON.stringify(frame) + "\n");
}
child.stdout.on("data", (data) => {
  buffer += data;
  let pos;
  while ((pos = buffer.indexOf("\n")) >= 0) {
    const f = JSON.parse(buffer.slice(0, pos));
    buffer = buffer.slice(pos + 1);
    runtime.push(f);
    if (f.type === "ready") child.stdin.write(JSON.stringify(start) + "\n");
    if (f.type === "llm") send({ type: "llm_result", request_id: f.request_id, call_id: f.call_id, result: { ok: true, value: { content: [{ type: "tool_use", id: "submit-fixture", name: "organizer_submit", input: { index: 0, expected_version: "v1", alias: "\u6D4B\u8BD5", tags: [], reason: "\u6D4B\u8BD5\u6574\u7406" } }], stop_reason: "tool_use" } } });
    if (f.type === "tool") send({ type: "tool_result", request_id: f.request_id, call_id: f.call_id, result: { ok: true, value: { accepted: true, index: 0 } } });
    if (f.type === "finish") send({ type: "finish_result", request_id: f.request_id, call_id: f.call_id, result: { ok: true, value: { accepted: true, submitted_indices: [0] } } });
  }
});
const code = await new Promise((resolve) => child.on("close", resolve));
if (code !== 0) throw new Error("fixture worker failed");
const ids = /* @__PURE__ */ new Map();
for (const f of runtime) {
  if (f.call_id) ids.set(f.call_id, `fixture-call:${ids.size + 1}`);
  if (f.type === "checkpoint") {
    const m = JSON.parse(f.checkpoint.messages_json);
    for (const message of m) message.timestamp = 0;
    f.checkpoint.messages_json = JSON.stringify(m);
    f.checkpoint.elapsed_ms = 0;
  }
}
for (const f of [...host, ...runtime]) if (f.call_id) f.call_id = ids.get(f.call_id);
await writeFile(new URL("../fixtures/protocol-v1.json",import.meta.url), JSON.stringify({ host, runtime }, null, 2) + "\n");
