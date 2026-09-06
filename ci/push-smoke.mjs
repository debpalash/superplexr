// Web Push, end to end against an independent implementation: node plays
// the push service (TLS, VAPID verification, RFC 8291 decryption) and the
// browser (a P-256 key pair and an auth secret); the runtime encrypts and
// delivers with its own code. What node decrypts must be what was sent.
import { execSync } from "node:child_process";
import { createServer } from "node:https";
import { readFileSync } from "node:fs";
import crypto from "node:crypto";
process.env.NODE_TLS_REJECT_UNAUTHORIZED = "0";
const [,, base, socket, bin, certPem, keyPem, pushPort] = process.argv;
const modOf = (file) => import(`file://${process.cwd()}/web/${file}`);
const { Kind, FrameReader, Sequencer, encodeJson, hello, json, uuid, PROTOCOL_VERSION } = await modOf("wire.js");
const step = (s) => console.log(`▸ ${s}`);
const fail = (s) => { console.log(`   FAIL: ${s}`); process.exit(1); };
process.on("unhandledRejection", (e) => fail(`${e.code ?? ""} ${e.message ?? JSON.stringify(e)}`));
const host = (args) => { try { return JSON.parse(execSync(`${bin} --socket ${socket} ${args}`, { stdio: ["ignore", "pipe", "pipe"] }).toString()); } catch (e) { fail(`host \`${args}\`: ${e.stderr?.toString().trim() || e.message}`); } };
const b64u = (buf) => Buffer.from(buf).toString("base64url");

function connect({ pairingCode = null, deviceId = uuid() } = {}) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(`${base.replace(/^http/, "ws")}/ws`); ws.binaryType = "arraybuffer";
    const reader = new FrameReader(); const sequencer = new Sequencer(); const waiters = new Map(); const clientId = uuid(); let welcome = null;
    ws.onopen = () => ws.send(encodeJson(Kind.Hello, 0, hello(clientId, deviceId, { pairingCode }), sequencer));
    ws.onmessage = (ev) => { reader.push(new Uint8Array(ev.data)); for (const f of reader.take()) { if (!welcome) { if (f.kind === Kind.Welcome) { welcome = json(f); resolve({ request }); } else if (f.kind === Kind.Close) reject(json(f)); continue; } if (f.streamId === 0 && f.kind === Kind.Response) { const r = json(f); const w = waiters.get(r.request_id); waiters.delete(r.request_id); if (!w) continue; if (r.result.status === "success") w.resolve(r.result.body); else w.reject(r.result); } } };
    ws.onclose = (ev) => reject({ code: "closed", message: ev.reason });
    function request(action, extra = {}) { const id = uuid(); ws.send(encodeJson(Kind.Request, 0, { version: PROTOCOL_VERSION, client_id: clientId, request_id: id, surface_id: null, control_epoch: null, ...extra, action }, sequencer)); return new Promise((res, rej) => waiters.set(id, { resolve: res, reject: rej })); }
  });
}

step("1. the runtime has one application server key");
const owner = await connect({ pairingCode: host("device-pair --label push-owner").code });
const info = await owner.request({ type: "push_info" });
if (!info.public_key) fail("no public key");
const vapidPublic = Buffer.from(info.public_key, "base64url");
if (vapidPublic.length !== 65 || vapidPublic[0] !== 4) fail(`key ${vapidPublic.length} bytes`);
console.log(`   key ok (${vapidPublic.length} bytes) · ${info.subscriptions.length} subscriptions`);

step("2. a browser subscribes: its key pair and auth secret, the push service's endpoint");
const browser = crypto.generateKeyPairSync("ec", { namedCurve: "prime256v1" });
const jwk = browser.publicKey.export({ format: "jwk" });
const p256dh = Buffer.concat([Buffer.from([4]), Buffer.from(jwk.x, "base64url"), Buffer.from(jwk.y, "base64url")]);
const auth = crypto.randomBytes(16);
const endpoint = `https://127.0.0.1:${pushPort}/send/${uuid()}`;
await owner.request({ type: "register_push_subscription", subscription: { endpoint, p256dh: b64u(p256dh), auth: b64u(auth), label: "node-phone" } });
const bad = await owner.request({ type: "register_push_subscription", subscription: { endpoint: "http://plain", p256dh: b64u(p256dh), auth: b64u(auth), label: "x" } }).then(() => "accepted", (e) => e.message);
console.log(`   registered · plain http refused: ${/https/.test(bad)}`);
if (!/https/.test(bad)) fail("validation");

step("3. the push service receives a VAPID-signed, aes128gcm-encrypted message and reads it");
let received = null;
// TLS 1.2 only: macOS's curl (LibreSSL) cannot complete a 1.3 handshake with node's
// server, though it does with real push services. The stand-in bends, the runtime does not.
const service = createServer({ cert: readFileSync(certPem), key: readFileSync(keyPem), minVersion: "TLSv1.2", maxVersion: "TLSv1.2" }, (req, res) => {
  const chunks = [];
  req.on("data", (c) => chunks.push(c));
  req.on("end", () => {
    const body = Buffer.concat(chunks);
    const authz = req.headers["authorization"] ?? "";
    const m = /^vapid t=([^,]+), k=([^,\s]+)$/.exec(authz);
    if (!m) { res.writeHead(400); res.end(); received = { error: `authorization: ${authz}` }; return; }
    const [h, c, sig] = m[1].split(".");
    const k = Buffer.from(m[2], "base64url");
    const pub = crypto.createPublicKey({ key: { kty: "EC", crv: "P-256", x: b64u(k.subarray(1, 33)), y: b64u(k.subarray(33, 65)) }, format: "jwk" });
    const verified = crypto.verify("sha256", Buffer.from(`${h}.${c}`), { key: pub, dsaEncoding: "ieee-p1363" }, Buffer.from(sig, "base64url"));
    const claims = JSON.parse(Buffer.from(c, "base64url").toString());
    // RFC 8291 decryption with the browser's private key
    const salt = body.subarray(0, 16); const rs = body.readUInt32BE(16); const idlen = body[20]; const asPub = body.subarray(21, 21 + idlen); const ciphertext = body.subarray(21 + idlen);
    const ecdh = crypto.createECDH("prime256v1"); ecdh.setPrivateKey(Buffer.from(browser.privateKey.export({ format: "jwk" }).d, "base64url"));
    const shared = ecdh.computeSecret(asPub);
    const info = Buffer.concat([Buffer.from("WebPush: info\0"), p256dh, asPub]);
    const ikm = Buffer.from(crypto.hkdfSync("sha256", shared, auth, info, 32));
    const cek = Buffer.from(crypto.hkdfSync("sha256", ikm, salt, Buffer.from("Content-Encoding: aes128gcm\0"), 16));
    const nonce = Buffer.from(crypto.hkdfSync("sha256", ikm, salt, Buffer.from("Content-Encoding: nonce\0"), 12));
    const decipher = crypto.createDecipheriv("aes-128-gcm", cek, nonce);
    decipher.setAuthTag(ciphertext.subarray(ciphertext.length - 16));
    let plain;
    try { plain = Buffer.concat([decipher.update(ciphertext.subarray(0, ciphertext.length - 16)), decipher.final()]); } catch (e) { received = { error: `decrypt: ${e.message}` }; res.writeHead(400); res.end(); return; }
    const delimiter = plain[plain.length - 1];
    received = { verified, aud: claims.aud, contentEncoding: req.headers["content-encoding"], ttl: req.headers["ttl"], rs, delimiter, notice: JSON.parse(plain.subarray(0, plain.length - 1).toString()), sameKey: k.equals(vapidPublic) };
    res.writeHead(201); res.end();
  });
});
await new Promise((r) => service.listen(Number(pushPort), "127.0.0.1", r));
const sent = await owner.request({ type: "test_push", title: "Fault opened", body: "cargo test exited 101 in ~/src/ultraplexr" });
await new Promise((r) => setTimeout(r, 300));
console.log(`   runtime saw status ${sent.outcomes.map((o) => o[1]).join(",")} · service: ${received ? JSON.stringify({ verified: received.verified, aud: received.aud, enc: received.contentEncoding, rs: received.rs, delimiter: received.delimiter, sameKey: received.sameKey, error: received.error }) : "nothing"}`);
if (!received || received.error) fail(received?.error ?? "no push arrived");
if (!received.verified || !received.sameKey || received.aud !== `https://127.0.0.1:${pushPort}` || received.contentEncoding !== "aes128gcm" || received.rs !== 4096 || received.delimiter !== 2) fail("push envelope");
if (received.notice.title !== "Fault opened" || !received.notice.body.includes("exited 101")) fail(`notice ${JSON.stringify(received.notice)}`);
if (sent.outcomes[0][1] !== 201) fail("status");

step("4. a dropped endpoint (410) is forgotten; forgetting is explicit too");
service.removeAllListeners("request");
service.on("request", (req, res) => { res.writeHead(410); res.end(); });
await owner.request({ type: "test_push", title: "again", body: "" });
const after = await owner.request({ type: "push_info" });
console.log(`   subscriptions after a 410: ${after.subscriptions.length}`);
if (after.subscriptions.length !== 0) fail("gone endpoint kept");
await owner.request({ type: "register_push_subscription", subscription: { endpoint, p256dh: b64u(p256dh), auth: b64u(auth), label: "node-phone" } });
await owner.request({ type: "forget_push_subscription", endpoint });
if ((await owner.request({ type: "push_info" })).subscriptions.length !== 0) fail("forget");
service.close();
console.log("all push steps pass");
process.exit(0);
