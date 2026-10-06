import test from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { readFile, mkdtemp, readdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
const start = JSON.parse(await readFile(new URL("../fixtures/start.json", import.meta.url)));
const bundle = fileURLToPath(new URL("../dist/worker.mjs", import.meta.url));
const tool = (id, name = "organizer_submit", input = { index: 0, expected_version: "v1", alias: "\u6D4B\u8BD5", tags: [], reason: "\u6D4B\u8BD5\u6574\u7406" }) => ({ type: "tool_use", id, name, input });
const response = (content, stop_reason = content.some((c) => c.type === "tool_use") ? "tool_use" : "end_turn") => ({ content, stop_reason });
async function run(handle, input = start, setup = {}) {
  const dir = await mkdtemp(join(tmpdir(), "qp-pi-"));
  const child = spawn(process.execPath, [bundle], { cwd: dir, env: { PATH: process.env.PATH, HOME: dir, ANTHROPIC_API_KEY: "must-not-read-test-sentinel" }, stdio: ["pipe", "pipe", "pipe"] });
  const frames = [];
  let out = "", stderr = "", failure;
  const done = new Promise((resolve) => child.on("close", (code2) => resolve(code2)));
  const timer = setTimeout(() => {
    failure = new Error("worker did not terminate");
    child.kill();
  }, 7e3);
  child.stderr.on("data", (data) => stderr += data);
  const send = (f) => child.stdin.write(JSON.stringify(f) + "\n");
  child.stdout.on("data", (data) => {
    out += data;
    let idx;
    while ((idx = out.indexOf("\n")) >= 0) {
      const frame = JSON.parse(out.slice(0, idx));
      out = out.slice(idx + 1);
      frames.push(frame);
      try {
        if (frame.type === "ready") {
          if (setup.raw) child.stdin.write(setup.raw);
          else if (setup.eof) child.stdin.end();
          else send(input);
        } else handle(frame, send, child);
      } catch (e) {
        failure = e;
        child.kill();
      }
    }
  });
  const code = await done;
  clearTimeout(timer);
  const files = await readdir(dir, { recursive: true });
  await rm(dir, { recursive: true, force: true });
  if (failure) throw failure;
  assert.equal(stderr, "");
  assert.deepEqual(files, [], "worker must never persist session/config/log files");
  return { frames, code };
}
const reply = (frame, send, result, type = `${frame.type}_result`) => send({ type, request_id: frame.request_id, call_id: frame.call_id, result });
const ok = (value) => ({ ok: true, value });
const err = (code = "invalid_alias") => ({ ok: false, error: { code, message: "\u8BF7\u4FEE\u6B63\u540D\u79F0", retryable: true } });
const terminal = (r) => r.frames.at(-1);
test("real Pi corrects thrown host error and only finishes after accepted submission", async () => {
  let n = 0;
  const r = await run((f, send) => {
    if (f.type === "llm") {
      n++;
      if (n === 2) {
        const results = f.request.messages.flatMap((m) => m.content).filter((c) => c.type === "tool_result");
        assert.equal(results.at(-1).is_error, true);
        assert.match(results.at(-1).content, /invalid_alias/);
      }
      reply(f, send, ok(response([tool(`t${n}`)])));
    }
    if (f.type === "tool") reply(f, send, n === 1 ? err() : ok({ accepted: true, index: 0 }));
    if (f.type === "finish") reply(f, send, ok({ accepted: true, submitted_indices: [0] }));
  });
  assert.equal(terminal(r).type, "completed");
  assert.equal(n, 2);
  assert.equal(r.code, 0);
});
test("ordinary final chat is never a proposal", async () => {
  const r = await run((f, send) => {
    if (f.type === "llm") reply(f, send, ok(response([{ type: "text", text: "\u5DF2\u5B8C\u6210\uFF1A\u522B\u540D=\u5047\u7684" }])));
  });
  assert.equal(terminal(r).type, "failed");
  assert.equal(terminal(r).error.code, "incomplete");
  assert.equal(r.frames.filter((f) => f.type === "finish").length, 0);
});
test("full batch is staged in order and host completion validates exact coverage", async () => {
  let active = 0;
  const input = { ...start, item_refs: [...start.item_refs, { index: 1, kind: "text", title: "\u6D4B\u8BD5\u4E8C", version: "v2" }] };
  const r = await run((f, send) => {
    if (f.type === "llm") reply(f, send, ok(response([tool("a"), tool("b", "organizer_submit", { index: 1, expected_version: "v2", alias: null, tags: ["\u8D44\u6599"], reason: "\u5F52\u7C7B" })])));
    if (f.type === "tool") {
      assert.equal(f.arguments.index, active++);
      reply(f, send, ok({ accepted: true, index: f.arguments.index }));
    }
    if (f.type === "finish") reply(f, send, ok({ accepted: true, submitted_indices: [0, 1] }));
  }, input);
  assert.equal(terminal(r).type, "completed");
  assert.equal(active, 2);
  assert.equal(r.frames.filter((f) => f.type === "llm").length, 1);
});
test("host completion rejection cannot complete a staged batch", async () => {
  const r = await run((f, send) => {
    if (f.type === "llm") reply(f, send, ok(response([tool("a")])));
    if (f.type === "tool") reply(f, send, ok({ accepted: true, index: 0 }));
    if (f.type === "finish") reply(f, send, ok({ accepted: true, submitted_indices: [] }));
  });
  assert.equal(terminal(r).error.code, "incomplete");
});
test("partial batch plus final refusal remains incomplete", async () => {
  let n = 0;
  const r = await run((f, send) => {
    if (f.type === "llm") reply(f, send, ok(n++ === 0 ? response([tool("a")]) : response([{ type: "text", text: "\u65E0\u6CD5\u7EE7\u7EED" }], "refusal")));
    if (f.type === "tool") reply(f, send, ok({ accepted: true, index: 0 }));
  }, { ...start, item_refs: [...start.item_refs, { index: 1, kind: "text", title: "\u6D4B\u8BD5\u4E8C", version: "v2" }] });
  assert.equal(terminal(r).error.code, "incomplete");
});
test("unknown tool never reaches host and real Pi reports its error next round", async () => {
  let n = 0;
  const r = await run((f, send) => {
    if (f.type === "llm") {
      if (n++ === 0) {
        reply(f, send, ok(response([tool("bad", "shell", { command: "echo unsafe" })])));
      } else {
        assert.equal(f.request.messages.at(-1).content.at(-1).is_error, true);
        reply(f, send, ok(response([tool("good")])));
      }
    }
    if (f.type === "tool") {
      assert.equal(f.name, "organizer_submit");
      reply(f, send, ok({ accepted: true, index: 0 }));
    }
    if (f.type === "finish") reply(f, send, ok({ accepted: true, submitted_indices: [0] }));
  });
  assert.equal(terminal(r).type, "completed");
});
test("unknown argument fields are rejected before host mutation", async () => {
  let n = 0;
  const r = await run((f, send) => {
    if (f.type === "llm") {
      reply(f, send, ok(response([tool(`t${n}`, "organizer_submit", n++ === 0 ? { index: 0, expected_version: "v1", alias: "\u6D4B\u8BD5", tags: [], reason: "\u6D4B\u8BD5", shell: "unsafe" } : { index: 0, expected_version: "v1", alias: "\u6D4B\u8BD5", tags: [], reason: "\u6D4B\u8BD5" })])));
    }
    if (f.type === "tool") reply(f, send, ok({ accepted: true, index: 0 }));
    if (f.type === "finish") reply(f, send, ok({ accepted: true, submitted_indices: [0] }));
  });
  assert.equal(r.frames.filter((f) => f.type === "tool").length, 1);
  assert.equal(terminal(r).type, "completed");
});
test("duplicate replies fail the active process instead of being applied twice", async () => {
  const r = await run((f, send) => {
    if (f.type === "llm") {
      reply(f, send, ok(response([tool("a")])));
      reply(f, send, ok(response([tool("a")])));
    }
  });
  assert.equal(terminal(r).error.code, "protocol");
  assert.equal(r.frames.filter((f) => f.type === "completed").length, 0);
});
test("mismatched and late replies fail correlation", async () => {
  const r = await run((f, send) => {
    if (f.type === "llm") send({ type: "llm_result", request_id: f.request_id, call_id: "old-process:1", result: ok(response([])) });
  });
  assert.equal(terminal(r).error.code, "protocol");
});
test("resume retains transcript, rounds/calls and submits without replaying settled tools", async () => {
  let cp;
  const first = await run((f, send) => {
    if (f.type === "llm") {
      if (cp?.calls === 1) {
        send({ type: "cancel", request_id: f.request_id });
        return;
      }
      reply(f, send, ok(response([tool("context", "items_get_context", { index: 0 })])));
    }
    if (f.type === "tool") reply(f, send, ok({ title: "synthetic context", version: "v1" }));
    if (f.type === "checkpoint" && f.checkpoint.calls === 1 && JSON.parse(f.checkpoint.messages_json).at(-1).role === "toolResult") cp = f.checkpoint;
  });
  assert.equal(terminal(first).error.code, "cancelled");
  assert.ok(cp);
  const r = await run((f, send) => {
    if (f.type === "llm") {
      assert.match(JSON.stringify(f.request.messages), /synthetic context/);
      reply(f, send, ok(response([tool("submit")])));
    }
    if (f.type === "tool") {
      assert.equal(f.name, "organizer_submit");
      reply(f, send, ok({ accepted: true, index: 0 }));
    }
    if (f.type === "finish") reply(f, send, ok({ accepted: true, submitted_indices: [0] }));
  }, { ...start, checkpoint: cp });
  assert.equal(terminal(r).type, "completed");
  const last = r.frames.filter((f) => f.type === "checkpoint").at(-1).checkpoint;
  assert.equal(last.rounds, cp.rounds + 1);
  assert.equal(last.calls, cp.calls + 1);
  assert.ok(last.elapsed_ms >= cp.elapsed_ms);
});
test("resume completed transcript requires host finish again without another LLM call", async () => {
  let cp;
  await run((f, send) => {
    if (f.type === "llm") reply(f, send, ok(response([tool("a")])));
    if (f.type === "tool") reply(f, send, ok({ accepted: true, index: 0 }));
    if (f.type === "checkpoint" && JSON.parse(f.checkpoint.messages_json).at(-1).role === "toolResult") cp = f.checkpoint;
    if (f.type === "finish") send({ type: "cancel", request_id: f.request_id });
  });
  const r = await run((f, send) => {
    if (f.type === "finish") reply(f, send, ok({ accepted: true, submitted_indices: [0] }));
    if (f.type === "llm") reply(f, send, ok(response([])));
  }, { ...start, checkpoint: cp });
  assert.equal(r.frames.filter((f) => f.type === "llm").length, 0);
  assert.equal(terminal(r).type, "completed");
});
test("model round budget is bounded even for repeated invalid calls", async () => {
  let n = 0;
  const r = await run((f, send) => {
    if (f.type === "llm") reply(f, send, ok(response([tool(`bad${++n}`, "shell", { command: "x" })])));
  }, { ...start, max_rounds: 2 });
  assert.equal(n, 2);
  assert.equal(terminal(r).error.code, "round_limit");
});
test("more than 32 tool calls are rejected before any host tool executes", async () => {
  const r = await run((f, send) => {
    if (f.type === "llm") reply(f, send, ok(response(Array.from({ length: 33 }, (_, i) => tool(`id${i}`, "items_get_context", { index: 0 })))));
  });
  assert.equal(terminal(r).error.code, "call_limit");
  assert.equal(r.frames.filter((f) => f.type === "tool").length, 0);
});
test("EOF, invalid JSON and oversized unterminated frames have explicit terminal errors", async () => {
  for (const [setup, code] of [[{ eof: true }, "eof"], [{ raw: "{bad}\n" }, "protocol"], [{ raw: "x".repeat(256 * 1024 + 1) }, "frame_limit"]]) {
    const r = await run(() => {
    }, start, setup);
    assert.equal(terminal(r).error.code, code);
    assert.equal(r.frames.filter((f) => f.type === "failed").length, 1);
  }
});
test("oversized model and tool values terminate with their budget code", async () => {
  for (const [kind, expected] of [["llm", "response_limit"], ["tool", "tool_result_limit"]]) {
    const r = await run((f, send) => {
      if (f.type === "llm") reply(f, send, ok(kind === "llm" ? response([{ type: "text", text: "x".repeat(64 * 1024) }]) : response([tool("a", "items_get_context", { index: 0 })])));
      if (f.type === "tool") reply(f, send, ok({ context: "x".repeat(16 * 1024) }));
    });
    assert.equal(terminal(r).error.code, expected);
  }
});
test("LLM host network/auth errors retain code and retryable for Rust backoff", async () => {
  for (const [code, retryable] of [["TRANSIENT_NETWORK", true], ["AUTH", false]]) {
    const r = await run((f, send) => {
      if (f.type === "llm") reply(f, send, { ok: false, error: { code, message: "synthetic transport failure", retryable } });
    });
    assert.equal(terminal(r).error.code, code);
    assert.equal(terminal(r).error.retryable, retryable);
  }
});
test("cancel while waiting for host emits one terminal failure", async () => {
  const r = await run((f, send) => {
    if (f.type === "llm") send({ type: "cancel", request_id: f.request_id });
  });
  assert.equal(terminal(r).error.code, "cancelled");
  assert.equal(r.frames.filter((f) => f.type === "failed").length, 1);
});
test("resume rejects unsettled and oversized transcripts", async () => {
  let cp;
  await run((f, send) => {
    if (f.type === "checkpoint" && !cp) cp = f.checkpoint;
    if (f.type === "llm") send({ type: "cancel", request_id: f.request_id });
  });
  for (const [bad, expected] of [[{ ...cp, messages_json: "x".repeat(128 * 1024) }, "checkpoint_limit"], [{ ...cp, messages_json: JSON.stringify([...JSON.parse(cp.messages_json), { role: "assistant", content: [{ type: "toolCall", id: "pending", name: "items_get_context", arguments: { index: 0 } }], api: "anthropic-messages", provider: "host", model: "claude-fixture", usage: {}, stopReason: "toolUse", timestamp: 0 }]), rounds: 2, calls: 1 }, "protocol"]]) {
    const r = await run(() => {
    }, { ...start, checkpoint: bad });
    assert.equal(terminal(r).error.code, expected);
  }
});
test("checkpoint message count limit and preserved tool budget prevent fresh host work", async () => {
  let cp;
  await run((f, send) => {
    if (f.type === "checkpoint" && !cp) cp = f.checkpoint;
    if (f.type === "llm") send({ type: "cancel", request_id: f.request_id });
  });
  const over = { ...cp, messages_json: JSON.stringify([...JSON.parse(cp.messages_json), ...Array.from({ length: 63 }, () => ({ role: "user", content: "synthetic", timestamp: 0 }))]) };
  const r = await run(() => {
  }, { ...start, checkpoint: over });
  assert.equal(terminal(r).error.code, "checkpoint_limit");
  const budget = await run((f, send) => {
    if (f.type === "llm") reply(f, send, ok(response([tool("a")])));
  }, { ...start, checkpoint: { ...cp, calls: 32 } });
  assert.equal(terminal(budget).error.code, "call_limit");
  assert.equal(budget.frames.filter((f) => f.type === "tool").length, 0);
});
test("preserved active-time budget expires while blocked on host without disk writes", async () => {
  let cp;
  await run((f, send) => {
    if (f.type === "checkpoint" && !cp) cp = f.checkpoint;
    if (f.type === "llm") send({ type: "cancel", request_id: f.request_id });
  });
  const r = await run(() => {
  }, { ...start, checkpoint: { ...cp, elapsed_ms: 119980 } });
  assert.equal(terminal(r).error.code, "deadline");
});
test("JS consumes the same host/runtime fixture as Rust", async () => {
  const fixture = JSON.parse(await readFile(new URL("../fixtures/protocol-v1.json", import.meta.url)));
  const r = await run((f, send) => {
    if (["llm", "tool", "finish"].includes(f.type)) {
      const host = fixture.host.find((h) => h.type === `${f.type}_result`);
      reply(f, send, host.result);
      if (f.type === "llm") assert.deepEqual(f.request, fixture.runtime.find((h) => h.type === "llm").request);
    }
  }, fixture.host[0]);
  assert.deepEqual(terminal(r).receipt, { accepted: true, submitted_indices: [0] });
});
test("strict input types do not silently coerce index strings", async () => {
  let round = 0;
  const r = await run((f, send) => {
    if (f.type === "llm") reply(f, send, ok(response([tool(`t${round}`, "organizer_submit", { index: round++ === 0 ? "0" : 0, expected_version: "v1", alias: "\u6D4B\u8BD5", tags: [], reason: "\u6D4B\u8BD5" })])));
    if (f.type === "tool") reply(f, send, ok({ accepted: true, index: 0 }));
    if (f.type === "finish") reply(f, send, ok({ accepted: true, submitted_indices: [0] }));
  });
  assert.equal(r.frames.filter((f) => f.type === "tool").length, 1);
  assert.equal(terminal(r).type, "completed");
});
test("long settled transcript fails checkpoint bytes rather than trimming or resetting budget", async () => {
  let round = 0;
  const r = await run((f, send) => {
    if (f.type === "llm") reply(f, send, ok(response([{ type: "text", text: "x".repeat(60 * 1024) }, tool(`unknown${round++}`, "shell", { command: "x" })])));
  });
  assert.equal(terminal(r).type, "failed", JSON.stringify({ types: r.frames.map((f) => f.type), code: r.code }));
  assert.equal(terminal(r).error.code, "checkpoint_limit");
  assert.ok(round >= 2);
});
test('signed and redacted Anthropic thinking survives a real Pi tool round and checkpoint',async()=>{
 const thoughts=[{type:'thinking',thinking:'synthetic planning',signature:'signature-fixture'},{type:'redacted_thinking',data:'opaque-fixture'}];let round=0;
 const r=await run((f,send)=>{if(f.type==='llm'){if(round++===0)reply(f,send,ok(response([...thoughts,tool('context','items_get_context',{index:0})])));else{assert.deepEqual(f.request.messages.find(m=>m.role==='assistant').content.slice(0,2),thoughts);reply(f,send,ok(response([tool('submit')])));}}if(f.type==='tool')reply(f,send,ok(f.name==='organizer_submit'?{accepted:true,index:0}:{context:'synthetic'}));if(f.type==='finish')reply(f,send,ok({accepted:true,submitted_indices:[0]}));});assert.equal(terminal(r).type,'completed');
});
test('malformed structured content and invalid UTF-8 fail as protocol errors',async()=>{
 const r=await run((f,send)=>{if(f.type==='llm')reply(f,send,ok({content:[null],stop_reason:'end_turn'}));});assert.equal(terminal(r).error.code,'protocol');
 const invalid=await run(()=>{},start,{raw:Buffer.from([0xff,10])});assert.equal(terminal(invalid).error.code,'protocol');
});
test('escaped lone Unicode surrogates are rejected consistently with Rust JSON',async()=>{
 const raw=JSON.stringify(start).replace('claude-fixture','\\ud800')+'\n';const r=await run((f,send)=>{if(f.type==='llm')reply(f,send,ok(response([])));},start,{raw});assert.equal(terminal(r).error.code,'protocol');
});

test('failed model wait is checkpointed and automatic recovery exhausts the cumulative deadline', async () => {
  const fixture = JSON.parse(await readFile(new URL('../fixtures/protocol-v1.json', import.meta.url)));
  const checkpoint = {...fixture.runtime.find(f => f.type === 'checkpoint').checkpoint, elapsed_ms:119600};
  const first = await run((f, send, child) => {
    if (f.type === 'llm') {
      const timer = setTimeout(() => reply(f, send, {ok:false,error:{code:'TRANSIENT_NETWORK',message:'synthetic delayed failure',retryable:true}}),250);
      child.once('close', () => clearTimeout(timer));
    }
  }, {...start, checkpoint});
  assert.equal(terminal(first).error.code, 'TRANSIENT_NETWORK');
  const afterFailure = first.frames.filter(f => f.type === 'checkpoint').at(-1).checkpoint;
  assert.ok(afterFailure.elapsed_ms >= 119800, `failed wait must count: ${afterFailure.elapsed_ms}`);
  assert.equal(afterFailure.messages_json, checkpoint.messages_json);
  const recovered = await run((f, send, child) => {
    if (f.type === 'llm') {
      const timer = setTimeout(() => reply(f, send, ok(response([tool('after-failure')]))),250);
      child.once('close', () => clearTimeout(timer));
    }
    if (f.type === 'tool') reply(f,send,ok({accepted:true,index:0}));
    if (f.type === 'finish') reply(f,send,ok({accepted:true,submitted_indices:[0]}));
  }, {...start, checkpoint:afterFailure});
  assert.equal(terminal(recovered).error.code, 'deadline');
  assert.equal(recovered.frames.filter(f => f.type === 'completed').length, 0);
  const finalCheckpoint = recovered.frames.filter(f => f.type === 'checkpoint').at(-1).checkpoint;
  assert.equal(finalCheckpoint.elapsed_ms,120000);
  assert.ok(finalCheckpoint.rounds >= afterFailure.rounds);
});

test('failure halfway through tools refreshes only the last settled transcript and reserved counters', async () => {
  const fixture = JSON.parse(await readFile(new URL('../fixtures/protocol-v1.json', import.meta.url)));
  const checkpoint = {...fixture.runtime.find(f => f.type === 'checkpoint').checkpoint, elapsed_ms:40000};
  let toolCount=0;
  const result=await run((f,send,child)=>{
    if(f.type==='llm') reply(f,send,ok(response([tool('context-a','items_get_context',{index:0}),tool('usage-b','usage_get_summary',{index:0})])));
    if(f.type==='tool'){
      if(++toolCount===1)reply(f,send,ok({title:'synthetic settled host action'}));
      else {
        const timer=setTimeout(()=>reply(f,send,{ok:false,error:{code:'STALE_SOURCE',message:'synthetic invalidated batch',retryable:false}}),75);
        child.once('close',()=>clearTimeout(timer));
      }
    }
  },{...start,checkpoint});
  assert.equal(terminal(result).error.code,'STALE_SOURCE');
  const last=result.frames.filter(f=>f.type==='checkpoint').at(-1).checkpoint;
  assert.ok(last.elapsed_ms>=40060,`host wait must count: ${last.elapsed_ms}`);
  assert.equal(last.messages_json,checkpoint.messages_json,'must not checkpoint a partial assistant/tool batch');
  assert.equal(last.calls,checkpoint.calls+2);
  assert.equal(last.rounds,checkpoint.rounds+1);
  assert.equal(result.frames.at(-2).type,'checkpoint');
  assert.equal(result.frames.filter(f=>f.type==='failed').length,1);
});

test('failure awaiting completion receipt refreshes the fully staged transcript',async()=>{
  const result=await run((f,send,child)=>{
    if(f.type==='llm')reply(f,send,ok(response([tool('staged-before-failure')])));
    if(f.type==='tool')reply(f,send,ok({accepted:true,index:0}));
    if(f.type==='finish'){
      const timer=setTimeout(()=>reply(f,send,{ok:false,error:{code:'TRANSIENT_NETWORK',message:'synthetic completion wait failure',retryable:true}}),75);
      child.once('close',()=>clearTimeout(timer));
    }
  });
  assert.equal(terminal(result).error.code,'TRANSIENT_NETWORK');
  const checkpoints=result.frames.filter(f=>f.type==='checkpoint');
  const settled=checkpoints.at(-2).checkpoint;
  const failure=checkpoints.at(-1).checkpoint;
  assert.equal(failure.messages_json,settled.messages_json);
  assert.ok(failure.elapsed_ms>=settled.elapsed_ms+60);
  assert.equal(JSON.parse(failure.messages_json).at(-1).role,'toolResult');
  assert.equal(result.frames.at(-2).type,'checkpoint');
});

test('checkpoint envelope byte exhaustion emits one terminal without recursive refresh',async()=>{
  const fixture=JSON.parse(await readFile(new URL('../fixtures/protocol-v1.json',import.meta.url)));
  const base={...fixture.runtime.find(f=>f.type==='checkpoint').checkpoint,elapsed_ms:0,calls:0};
  const messages=JSON.parse(base.messages_json);
  messages[1].content='';
  const empty={...base,messages_json:JSON.stringify(messages)};
  const count=128*1024-Buffer.byteLength(JSON.stringify(empty));
  messages[1].content='x'.repeat(count);
  const checkpoint={...base,messages_json:JSON.stringify(messages)};
  assert.equal(Buffer.byteLength(JSON.stringify(checkpoint)),128*1024);
  const result=await run(()=>{}, {...start,checkpoint});
  assert.equal(terminal(result).error.code,'checkpoint_limit');
  assert.equal(result.frames.filter(f=>f.type==='failed').length,1);
  assert.equal(result.frames.filter(f=>f.type==='checkpoint'||f.type==='llm').length,0);
  assert.equal(result.code,1);
});
