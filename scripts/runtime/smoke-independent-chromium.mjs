import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { once } from "node:events";
import { createReadStream, promises as fs } from "node:fs";
import http from "node:http";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import { parseArgs } from "node:util";

const { values } = parseArgs({
  options: Object.fromEntries(
    ["probe", "binary", "version", "sha256", "output", "seed"].map((name) => [
      name,
      { type: "string" },
    ]),
  ),
});
for (const name of ["probe", "binary", "version", "sha256", "output"]) {
  assert.ok(values[name], `--${name} is required`);
}
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const digest = createHash("sha256");
for await (const bytes of createReadStream(values.binary)) digest.update(bytes);
assert.equal(digest.digest("hex"), values.sha256, "native executable checksum");
const root = await fs.mkdtemp(
  path.join(os.tmpdir(), "buglogin-independent-native-"),
);
const active = new Set();
const evidence = {
  engineVersion: values.version,
  platform: process.platform,
  checks: {},
};
let cookieSeen = "";
let proxyRequests = 0;
const fixture = http.createServer((request, response) => {
  cookieSeen = request.headers.cookie ?? "";
  response.writeHead(200, {
    "content-type": "text/html",
    "cache-control": "no-store",
  });
  response.end(
    "<!doctype html><html><head><title>BugLogin adapter fixture</title></head><body>local fixture</body></html>",
  );
});
fixture.listen(0, "127.0.0.1");
await once(fixture, "listening");
const origin = `http://127.0.0.1:${fixture.address().port}`;
const proxy = http.createServer((request, response) => {
  proxyRequests += 1;
  const target = new URL(request.url);
  if (target.origin !== origin) {
    response.writeHead(403);
    response.end();
    return;
  }
  const upstream = http.request(
    target,
    { method: request.method, headers: request.headers },
    (result) => {
      response.writeHead(result.statusCode, result.headers);
      result.pipe(response);
    },
  );
  upstream.on("error", () => {
    response.writeHead(502);
    response.end();
  });
  request.pipe(upstream);
});
proxy.listen(0, "127.0.0.1");
await once(proxy, "listening");

async function freePort() {
  const server = net.createServer();
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const port = server.address().port;
  await new Promise((resolve) => server.close(resolve));
  return port;
}

async function connectCdp(port) {
  let url;
  for (let attempt = 0; attempt < 150; attempt += 1) {
    try {
      const targets = await (
        await fetch(`http://127.0.0.1:${port}/json/list`, {
          signal: AbortSignal.timeout(1000),
        })
      ).json();
      url = targets.find(
        (target) => target.type === "page",
      )?.webSocketDebuggerUrl;
      if (url) break;
    } catch {
      /* The process may still be starting. */
    }
    await pause(100);
  }
  assert.ok(url, "browser exposes a local CDP page");
  const ws = new WebSocket(url);
  await once(ws, "open");
  let sequence = 0;
  const pending = new Map();
  ws.addEventListener("message", (event) => {
    const reply = JSON.parse(event.data);
    if (reply.method === "Inspector.targetCrashed")
      console.error("Native CDP target crashed");
    const waiter = pending.get(reply.id);
    if (!waiter) return;
    pending.delete(reply.id);
    clearTimeout(waiter.timer);
    if (reply.error)
      waiter.reject(new Error(`${waiter.method}: ${reply.error.message}`));
    else waiter.resolve(reply.result);
  });
  return {
    call(method, params = {}) {
      return new Promise((resolve, reject) => {
        const id = ++sequence;
        const timer = setTimeout(() => {
          pending.delete(id);
          reject(new Error(`${method}: timeout`));
        }, 10000);
        pending.set(id, { resolve, reject, timer, method });
        ws.send(JSON.stringify({ id, method, params }));
      });
    },
    close() {
      ws.close();
    },
  };
}

async function launch(label, extra = []) {
  const port = await freePort();
  const profile = path.join(root, label);
  if (values.seed && label === "profile-a") {
    const dir = path.join(profile, "BugLogin");
    await fs.mkdir(dir, { recursive: true });
    const file = path.join(dir, "independent-chromium.json");
    try {
      await fs.access(file);
    } catch {
      await fs.writeFile(
        file,
        JSON.stringify({
          schemaVersion: 1,
          engine: "fingerprint-chromium",
          engineVersion: values.version,
          seed: Number(values.seed),
          target: {
            operatingSystem:
              process.platform === "win32"
                ? "windows"
                : process.platform === "darwin"
                  ? "macos"
                  : "linux",
          },
        }),
      );
    }
  }
  const child = spawn(
    values.probe,
    [
      values.binary,
      values.version,
      values.sha256,
      profile,
      "--headless=new",
      `--remote-debugging-port=${port}`,
      "--remote-debugging-address=127.0.0.1",
      ...extra,
    ],
    { windowsHide: true, stdio: ["pipe", "pipe", "pipe"] },
  );
  let errors = "";
  child.stderr.on("data", (bytes) => {
    errors = (errors + bytes).slice(-4000);
  });
  const messages = [];
  const lines = createInterface({ input: child.stdout });
  lines.on("line", (line) => {
    messages.push(JSON.parse(line));
  });
  const exited = once(child, "exit");
  active.add(child);
  for (
    let attempt = 0;
    attempt < 200 && !messages.some((message) => message.status === "launched");
    attempt += 1
  ) {
    assert.equal(
      child.exitCode,
      null,
      `native Rust launcher exited early: ${errors}`,
    );
    await pause(100);
  }
  assert.ok(
    messages.some((message) => message.status === "launched"),
    `native launcher timeout: ${errors}`,
  );
  const cdp = await connectCdp(port);
  await cdp.call("Page.enable");
  return {
    cdp,
    async stop({ graceful = true } = {}) {
      const started = performance.now();
      if (graceful) await cdp.call("Browser.close").catch(() => {});
      cdp.close();
      child.stdin.end("stop\n");
      // The shared Windows controller allows 15s for graceful shutdown before
      // terminating its job. Give that policy time to finish and report it.
      const timeout = setTimeout(() => child.kill(), 30000);
      const [code] = await exited;
      clearTimeout(timeout);
      active.delete(child);
      assert.equal(code, 0, `native stop failed: ${errors}`);
      assert.ok(
        messages.some((message) => message.status === "stopped"),
        "Rust confirms stopped process tree",
      );
      evidence.stopMilliseconds ??= [];
      evidence.stopMilliseconds.push({
        graceful,
        elapsed: Math.round(performance.now() - started),
      });
    },
  };
}

async function evaluate(cdp, expression) {
  const result = await cdp.call("Runtime.evaluate", {
    expression,
    returnByValue: true,
    awaitPromise: true,
  });
  assert.ok(
    !result.exceptionDetails,
    `page evaluation: ${JSON.stringify(result.exceptionDetails)}`,
  );
  return result.result.value;
}

async function navigate(cdp, url) {
  await cdp.call("Page.navigate", { url });
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if (
      await evaluate(
        cdp,
        `location.href === ${JSON.stringify(url)} && document.readyState === 'complete'`,
      )
    )
      return;
    await pause(100);
  }
  throw new Error("fixture navigation timed out");
}

async function fingerprint(cdp) {
  console.error("Native fingerprint: navigator/screen");
  const observed = await evaluate(
    cdp,
    `(() => {
    return { platform: navigator.platform, userAgent: navigator.userAgent,
      cores: navigator.hardwareConcurrency, memory: navigator.deviceMemory,
      width: screen.width, height: screen.height };
  })()`,
  );
  console.error("Native fingerprint: canvas");
  const canvas = await evaluate(
    cdp,
    `(() => {
    const canvas=document.createElement('canvas'); canvas.width=240; canvas.height=60;
    const ctx=canvas.getContext('2d'); ctx.font='18px Arial';
    ctx.fillStyle='#26a'; ctx.fillRect(0,0,240,60); ctx.fillStyle='#ef9'; ctx.fillText('BugLogin adapter 012345',4,28);
    return canvas.toDataURL();
  })()`,
  );
  observed.canvas = createHash("sha256").update(canvas).digest("hex");
  console.error("Native fingerprint: WebGL");
  observed.gpu = await evaluate(
    cdp,
    `(() => {
    const gl=document.createElement('canvas').getContext('webgl');
    const info=gl?.getExtension('WEBGL_debug_renderer_info');
    return info ? gl.getParameter(info.UNMASKED_RENDERER_WEBGL) : null;
  })()`,
  );
  console.error("Native fingerprint: audio/worker");
  const audio = await evaluate(
    cdp,
    `(async () => {
    const context = new OfflineAudioContext(1, 5000, 44100);
    const oscillator = context.createOscillator(); oscillator.type='triangle'; oscillator.frequency.value=1000;
    const compressor=context.createDynamicsCompressor(); compressor.threshold.value=-50; compressor.knee.value=40;
    compressor.ratio.value=12; compressor.attack.value=0; compressor.release.value=.25;
    oscillator.connect(compressor); compressor.connect(context.destination); oscillator.start(0);
    const buffer=await context.startRendering(); return Array.from(buffer.getChannelData(0).slice(4500));
  })()`,
  );
  observed.audio = createHash("sha256")
    .update(JSON.stringify(audio))
    .digest("hex");
  const worker = await evaluate(
    cdp,
    `new Promise((resolve,reject) => {
    const source='postMessage({cores:navigator.hardwareConcurrency,userAgent:navigator.userAgent})';
    const url=URL.createObjectURL(new Blob([source],{type:'text/javascript'}));
    const worker=new Worker(url); worker.onmessage=e=>{worker.terminate();URL.revokeObjectURL(url);resolve(e.data)};
    worker.onerror=()=>{worker.terminate();URL.revokeObjectURL(url);reject(new Error('worker failed'))};
  })`,
  );
  assert.equal(
    worker.cores,
    observed.cores,
    "worker and window hardware concurrency agree",
  );
  assert.equal(
    worker.userAgent,
    observed.userAgent,
    "worker and window user-agent agree",
  );
  evidence.checks.workerConsistency = "pass";
  return observed;
}

try {
  const extension = path.join(root, "extension");
  await fs.mkdir(extension);
  await fs.writeFile(
    path.join(extension, "manifest.json"),
    JSON.stringify({
      manifest_version: 3,
      name: "BugLogin adapter smoke",
      version: "1.0",
      content_scripts: [
        {
          matches: ["http://127.0.0.1/*"],
          js: ["content.js"],
          run_at: "document_end",
        },
      ],
    }),
  );
  await fs.writeFile(
    path.join(extension, "content.js"),
    "document.documentElement.setAttribute('data-buglogin-extension','loaded');",
  );
  const extensionArgs = [
    `--load-extension=${extension}`,
    `--disable-extensions-except=${extension}`,
  ];
  const first = await launch("profile-a", extensionArgs);
  console.error("Native stage: initial JavaScript");
  assert.equal(await evaluate(first.cdp, "1 + 1"), 2);
  console.error("Native stage: navigate fixture");
  await navigate(first.cdp, `${origin}/first`);
  console.error("Native stage: fingerprint and extension");
  const firstIdentity = await fingerprint(first.cdp);
  assert.equal(
    await evaluate(
      first.cdp,
      `(() => {
    for(let i=0;i<25;i++) {
      const c=document.createElement('canvas');c.width=240;c.height=60;
      const x=c.getContext('2d');x.fillStyle='#26a';x.fillRect(0,0,240,60);x.font='18px Arial';x.fillText('BugLogin stress',4,28);
      x.getImageData(0,0,240,60);c.toDataURL();
    }
    const c=document.createElement('canvas');c.width=128;c.height=64;const g=c.getContext('webgl');
    if(g)for(let i=0;i<25;i++){g.clearColor(i/25,.5,.2,1);g.clear(g.COLOR_BUFFER_BIT);g.readPixels(0,0,128,64,g.RGBA,g.UNSIGNED_BYTE,new Uint8Array(128*64*4));}
    return 25;
  })()`,
    ),
    25,
  );
  evidence.checks.canvasReadbackStress = "pass";
  for (let attempt = 0; attempt < 40; attempt += 1) {
    if (
      await evaluate(
        first.cdp,
        "document.documentElement.getAttribute('data-buglogin-extension') === 'loaded'",
      )
    )
      break;
    await pause(100);
  }
  assert.equal(
    await evaluate(
      first.cdp,
      "document.documentElement.getAttribute('data-buglogin-extension')",
    ),
    "loaded",
  );
  evidence.checks.extension = "pass";
  const cookie = await first.cdp.call("Network.setCookie", {
    name: "adapter_session",
    value: "retained",
    url: origin,
    httpOnly: true,
    expires: Date.now() / 1000 + 86400,
  });
  assert.equal(cookie.success, true);
  await first.stop();
  const second = await launch("profile-a", extensionArgs);
  await navigate(second.cdp, `${origin}/restart`);
  assert.deepEqual(
    await fingerprint(second.cdp),
    firstIdentity,
    "identity surfaces remain stable after restart",
  );
  evidence.checks.fingerprintRestart = "pass";
  assert.ok(
    cookieSeen.includes("adapter_session=retained"),
    "cookie survives native process restart",
  );
  evidence.checks.cookieRestart = "pass";
  await second.stop();
  const other = await launch("profile-b", [
    `--proxy-server=http://127.0.0.1:${proxy.address().port}`,
    "--proxy-bypass-list=<-loopback>",
  ]);
  await navigate(other.cdp, `${origin}/other`);
  assert.notDeepEqual(
    await fingerprint(other.cdp),
    firstIdentity,
    "different profile gets a different fingerprint",
  );
  assert.ok(
    !cookieSeen.includes("adapter_session="),
    "profile cookies are isolated",
  );
  assert.ok(proxyRequests > 0, "fixture request used configured proxy");
  evidence.checks.profileIsolation = "pass";
  evidence.checks.proxyRouting = "pass";
  await other.stop({ graceful: false });
  evidence.checks.processTreeStop = "pass";
  evidence.observedFingerprint = firstIdentity;
  await fs.writeFile(values.output, `${JSON.stringify(evidence, null, 2)}\n`);
  console.log(JSON.stringify(evidence, null, 2));
} catch (error) {
  console.error(`Native adapter smoke failed. Disposable evidence: ${root}`);
  throw error;
} finally {
  for (const child of active) child.stdin.end("stop\n");
  fixture.closeAllConnections();
  fixture.close();
  proxy.closeAllConnections();
  proxy.close();
}
