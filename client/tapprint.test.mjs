// Tests for the browser client, with `fetch` and `localStorage` faked.
// Run with: node --test client/tapprint.test.mjs
import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";

import { TapPrint, TapPrintError } from "./tapprint.js";

const PLAIN = "http://127.0.0.1:9240";
const SECURE = "https://127.0.0.1:9241";

/** Every call made, as "METHOD url". */
let calls;
/** Origins that answer. Any other origin fails the way a blocked fetch does. */
let up;
/** The reply every reachable origin gives, unless a test changes it. */
let reply;
let store;

beforeEach(() => {
  calls = [];
  up = [PLAIN, SECURE];
  reply = { status: 200, body: { ok: true, code: "fresh" } };
  store = new Map();
  globalThis.localStorage = {
    getItem: (key) => store.get(key) ?? null,
    setItem: (key, value) => store.set(key, value),
    removeItem: (key) => store.delete(key),
  };
  globalThis.fetch = async (url, init) => {
    calls.push(`${init.method} ${url}`);
    if (!up.includes(url.slice(0, url.indexOf("/v1")))) throw new TypeError("blocked");
    const { status, body } = reply;
    return { ok: status < 400, status, json: async () => body, headers: init.headers };
  };
});

const receipt = () => new Blob(["png"]);
const isUnreachable = (error) =>
  error instanceof TapPrintError && error.reason === "unreachable" && error.status === 0;

test("Chrome: plain HTTP answers, so it is found once and then used directly", async () => {
  const till = new TapPrint({ code: "x" });
  await till.print(receipt());
  await till.print(receipt());
  assert.deepEqual(calls, [`GET ${PLAIN}/v1/status`, `POST ${PLAIN}/v1/print`, `POST ${PLAIN}/v1/print`]);
});

test("Safari: plain HTTP is blocked, so HTTPS is used and the receipt is sent once", async () => {
  up = [SECURE];
  const till = new TapPrint({ code: "x" });
  await till.print(receipt());
  assert.deepEqual(calls, [`GET ${PLAIN}/v1/status`, `GET ${SECURE}/v1/status`, `POST ${SECURE}/v1/print`]);
});

test("when nothing answers, the receipt is never sent", async () => {
  up = [];
  const till = new TapPrint({ code: "x" });
  await assert.rejects(till.print(receipt()), isUnreachable);
  assert.deepEqual(calls, [`GET ${PLAIN}/v1/status`, `GET ${SECURE}/v1/status`]);
});

test("a receipt that failed on the way is not sent again to the other address", async () => {
  // The first try may have reached the printer. A second try could print twice.
  const till = new TapPrint({ code: "x" });
  await till.status();
  up = [SECURE];
  calls = [];
  await assert.rejects(till.print(receipt()), isUnreachable);
  assert.deepEqual(calls, [`POST ${PLAIN}/v1/print`]);
  // The next call looks for TapPrint again.
  await till.status();
  assert.equal(calls.at(-1), `GET ${SECURE}/v1/status`);
});

test("a fixed choice is used without looking", async () => {
  up.push("http://127.0.0.1:9000");
  await new TapPrint({ secure: true }).status();
  await new TapPrint({ secure: false, port: 9000 }).status();
  assert.deepEqual(calls, [`GET ${SECURE}/v1/status`, "GET http://127.0.0.1:9000/v1/status"]);
});

test("calls made at the same moment share one look", async () => {
  const till = new TapPrint();
  await Promise.all([till.status(), till.status()]);
  assert.equal(calls.length, 3);
});

test("pairing stores the code, and later calls send it", async () => {
  const till = new TapPrint({ clientName: "MyShop" });
  assert.equal(till.isPaired(), false);
  await till.pair();
  assert.equal(store.get("tapprint.code"), "fresh");
  assert.equal(till.isPaired(), true);
  const sent = [];
  const inner = globalThis.fetch;
  globalThis.fetch = (url, init) => (sent.push(init.headers), inner(url, init));
  await till.print(receipt(), { drawer: true, cut: false, paperMm: 58 });
  assert.deepEqual(sent.at(-1), {
    "Content-Type": "image/png",
    "X-Paper-Mm": "58",
    "X-Cut": "0",
    "X-Drawer": "1",
    "X-Till-Client": "MyShop",
    Authorization: "Bearer fresh",
  });
});

test("a shop name the header cannot carry is cleaned, not turned into a failure", async () => {
  // fetch refuses a header with non-Latin-1 text. Left as is, a name in
  // Khmer made every call report "Cannot reach TapPrint".
  const sent = [];
  const inner = globalThis.fetch;
  globalThis.fetch = (url, init) => (sent.push(init.headers), inner(url, init));
  await new TapPrint({ clientName: "ហាង Cafe 1" }).pair();
  assert.equal(sent.at(-1)["X-Till-Client"], "Cafe 1");
  await new TapPrint({ clientName: "ហាង" }).pair();
  assert.equal("X-Till-Client" in sent.at(-1), false);
});

test("a rejected code is forgotten, unless another tab stored a newer one", async () => {
  store.set("tapprint.code", "old");
  const till = new TapPrint();
  reply = { status: 401, body: { ok: false, reason: "unauthorized", error: "rejected" } };
  await assert.rejects(till.print(receipt()), (error) => error.reason === "unauthorized" && error.status === 401);
  assert.equal(till.isPaired(), false);

  store.set("tapprint.code", "old");
  const inner = globalThis.fetch;
  globalThis.fetch = (url, init) => {
    if (init.method === "POST") store.set("tapprint.code", "newer");
    return inner(url, init);
  };
  await assert.rejects(till.print(receipt()));
  assert.equal(store.get("tapprint.code"), "newer");
});

test("a code given in the options is never forgotten", async () => {
  const till = new TapPrint({ code: "manual" });
  reply = { status: 401, body: { ok: false, reason: "unauthorized" } };
  await assert.rejects(till.print(receipt()));
  till.forget();
  assert.equal(till.isPaired(), true);
});

test("unpair tells TapPrint, then forgets the code", async () => {
  const till = new TapPrint();
  await till.unpair();
  assert.deepEqual(calls, [], "nothing stored, nothing to do");

  store.set("tapprint.code", "mine");
  await till.unpair();
  assert.equal(calls.at(-1), `POST ${PLAIN}/v1/unpair`);
  assert.equal(till.isPaired(), false);
});

test("unpair keeps the code when TapPrint could not be told", async () => {
  store.set("tapprint.code", "mine");
  up = [];
  await assert.rejects(new TapPrint().unpair(), isUnreachable);
  assert.equal(store.get("tapprint.code"), "mine");
});

test("an error reply carries its status and reason", async () => {
  const till = new TapPrint({ code: "x" });
  await till.status();
  reply = { status: 502, body: { ok: false, reason: "maybe_printed", error: "Check the paper." } };
  await assert.rejects(
    till.print(receipt()),
    (error) => error.status === 502 && error.reason === "maybe_printed" && error.message === "Check the paper.",
  );
  reply = { status: 200, body: null };
  await assert.rejects(till.status(), (error) => error.reason === "bad_response");
});

test("an unreachable TapPrint says why, when the browser can tell", async () => {
  up = [];
  // The site's own Content-Security-Policy refused the call.
  const listeners = [];
  globalThis.addEventListener = (name, listener) => listeners.push(listener);
  const blocked = new TapPrint();
  listeners.forEach((listener) => listener({ blockedURI: "http://127.0.0.1:9240" }));
  await assert.rejects(blocked.status(), (error) => isUnreachable(error) && error.detail === "blocked_by_csp");
  delete globalThis.addEventListener;

  // A page on plain HTTP is never let through by Chrome.
  globalThis.isSecureContext = false;
  await assert.rejects(new TapPrint().status(), (error) => error.detail === "insecure_page" && /HTTPS/.test(error.message));
  delete globalThis.isSecureContext;

  // The person clicked Block when the browser asked.
  const real = Object.getOwnPropertyDescriptor(globalThis, "navigator");
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    value: { permissions: { query: async () => ({ state: "denied" }) } },
  });
  await assert.rejects(new TapPrint().status(), (error) => error.detail === "permission_denied");
  Object.defineProperty(globalThis, "navigator", real);

  // TapPrint is simply not running: no detail, the general message.
  await assert.rejects(new TapPrint().status(), (error) => isUnreachable(error) && error.detail === undefined);
});

test("print refuses something that is not an image", async () => {
  await assert.rejects(new TapPrint({ code: "x" }).print({}), (error) => error.reason === "bad_request");
  assert.deepEqual(calls, []);
});
