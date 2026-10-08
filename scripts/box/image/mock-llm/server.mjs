#!/usr/bin/env node
// Scripted OpenAI-compatible chat-completions server for the box (no deps).
//
// pi talks to it as the custom provider "mock" (api: openai-completions).
// It replays a scenario file: a JSON object with an ordered list of assistant
// turns. Which turn to answer is derived from the request itself:
//   turn index = number of assistant messages already in the conversation.
// So the first reply to a prompt is turns[0]; after pi runs the tool calls of
// turns[0] and sends the results back, the reply is turns[1]; and so on. A new
// pi session starts again at turns[0]. Past the end -> `fallback` text.
//
// Scenario format (see scripts/box/README.md):
//   {
//     "fallback": "optional text for requests past the last turn",
//     "turns": [
//       { "text": "I'll add the file.",
//         "tools": [ { "name": "write", "args": { "path": "notes.md", "content": "hi\n" } } ] },
//       { "text": "Done." }
//     ]
//   }
// Optional per-turn: "delayMs" (pause before replying), "chunkMs" (delay
// between streamed text chunks, default 25).
//
// The scenario file is re-read on every request, so it can be swapped live.
// Env: MOCK_LLM_PORT (4141), MOCK_LLM_SCENARIO (/proof/scenario.json, falls
// back to /opt/box/default-scenario.json), MOCK_LLM_LOG (request log, JSONL).

import http from "node:http";
import fs from "node:fs";

const PORT = Number(process.env.MOCK_LLM_PORT || 4141);
const SCENARIO = process.env.MOCK_LLM_SCENARIO || "/proof/scenario.json";
const DEFAULT_SCENARIO = "/opt/box/default-scenario.json";
const LOG = process.env.MOCK_LLM_LOG || "";
const DEFAULT_FALLBACK = "(mock LLM: scenario has no more turns)";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function loadScenario() {
  const file = fs.existsSync(SCENARIO) ? SCENARIO : DEFAULT_SCENARIO;
  try {
    const s = JSON.parse(fs.readFileSync(file, "utf8"));
    if (!Array.isArray(s.turns)) throw new Error(`"turns" must be an array`);
    return { file, ...s };
  } catch (e) {
    return { file, turns: [], fallback: `(mock LLM: invalid scenario ${file}: ${e.message})` };
  }
}

function log(entry) {
  if (!LOG) return;
  try {
    fs.appendFileSync(LOG, JSON.stringify({ t: new Date().toISOString(), ...entry }) + "\n");
  } catch {}
}

function chunk(id, model, delta, finish = null, usage) {
  const c = {
    id,
    object: "chat.completion.chunk",
    created: Math.floor(Date.now() / 1000),
    model,
    choices: [{ index: 0, delta, finish_reason: finish }],
  };
  if (usage) {
    c.choices = [];
    c.usage = usage;
  }
  return `data: ${JSON.stringify(c)}\n\n`;
}

function splitText(text) {
  // Stream word-ish pieces so the TUI visibly streams.
  return text.match(/\S+\s*|\s+/g) || [];
}

async function reply(req, res, body) {
  const messages = Array.isArray(body.messages) ? body.messages : [];
  const assistantCount = messages.filter((m) => m.role === "assistant").length;
  const lastUser = [...messages].reverse().find((m) => m.role === "user");
  const scenario = loadScenario();
  const turn = scenario.turns[assistantCount];
  const text = turn ? turn.text || "" : scenario.fallback || DEFAULT_FALLBACK;
  const tools = turn && Array.isArray(turn.tools) ? turn.tools : [];
  const model = body.model || "mock";
  const id = `chatcmpl-mock-${Date.now()}`;

  log({
    event: "request",
    scenario: scenario.file,
    turn: turn ? assistantCount : "fallback",
    messages: messages.length,
    lastUser: lastUser?.content,
    tools: (body.tools || []).map((t) => t.function?.name || t.custom?.name),
  });

  if (turn?.delayMs) await sleep(turn.delayMs);
  const chunkMs = turn?.chunkMs ?? 25;

  if (body.stream === false) {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(
      JSON.stringify({
        id,
        object: "chat.completion",
        created: Math.floor(Date.now() / 1000),
        model,
        choices: [
          {
            index: 0,
            message: {
              role: "assistant",
              content: text || null,
              tool_calls: tools.length ? tools.map((t, i) => toolCall(t, i)) : undefined,
            },
            finish_reason: tools.length ? "tool_calls" : "stop",
          },
        ],
        usage: { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 },
      }),
    );
    return;
  }

  res.writeHead(200, {
    "content-type": "text/event-stream",
    "cache-control": "no-cache",
    connection: "keep-alive",
  });
  res.write(chunk(id, model, { role: "assistant", content: "" }));
  for (const piece of splitText(text)) {
    res.write(chunk(id, model, { content: piece }));
    if (chunkMs) await sleep(chunkMs);
  }
  tools.forEach((t, i) => {
    res.write(chunk(id, model, { tool_calls: [{ index: i, ...toolCall(t, i) }] }));
  });
  res.write(chunk(id, model, {}, tools.length ? "tool_calls" : "stop"));
  res.write(
    chunk(id, model, {}, null, {
      prompt_tokens: 0,
      completion_tokens: 0,
      total_tokens: 0,
    }),
  );
  res.write("data: [DONE]\n\n");
  res.end();
}

let callSeq = 0;
function toolCall(t, i) {
  return {
    id: t.id || `call_mock_${++callSeq}_${i}`,
    type: "function",
    function: { name: t.name, arguments: JSON.stringify(t.args ?? {}) },
  };
}

const server = http.createServer((req, res) => {
  const url = new URL(req.url, "http://x");
  if (req.method === "GET" && (url.pathname === "/health" || url.pathname === "/")) {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({ ok: true, scenario: loadScenario().file }));
    return;
  }
  if (req.method === "GET" && url.pathname.endsWith("/models")) {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({ object: "list", data: [{ id: "mock-1", object: "model", owned_by: "box" }] }));
    return;
  }
  if (req.method === "POST" && url.pathname.endsWith("/chat/completions")) {
    let raw = "";
    req.on("data", (d) => (raw += d));
    req.on("end", () => {
      let body;
      try {
        body = JSON.parse(raw || "{}");
      } catch (e) {
        res.writeHead(400, { "content-type": "application/json" });
        res.end(JSON.stringify({ error: { message: `invalid JSON: ${e.message}` } }));
        return;
      }
      reply(req, res, body).catch((e) => {
        log({ event: "error", message: String(e) });
        try {
          res.end();
        } catch {}
      });
    });
    return;
  }
  log({ event: "unknown", method: req.method, path: url.pathname });
  res.writeHead(404, { "content-type": "application/json" });
  res.end(JSON.stringify({ error: { message: `mock LLM: no route ${req.method} ${url.pathname}` } }));
});

server.listen(PORT, "127.0.0.1", () => {
  console.log(`mock LLM listening on http://127.0.0.1:${PORT}/v1 (scenario: ${SCENARIO})`);
});
