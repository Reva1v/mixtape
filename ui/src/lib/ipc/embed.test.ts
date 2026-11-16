import { describe, expect, it } from "vitest";
import { bridgeFor, install, islandFrame, type FromFrame } from "./embed";

type Internals = {
  transformCallback: (cb: (raw: unknown) => void, once?: boolean) => number;
  unregisterCallback: (id: number) => void;
  invoke: (cmd: string, args?: unknown) => Promise<unknown>;
};

function setup() {
  const w = {} as Window & typeof globalThis;
  const sent: FromFrame[] = [];
  const receive = install(w, (m) => sent.push(m));
  const internals = (w as unknown as { __TAURI_INTERNALS__: Internals }).__TAURI_INTERNALS__;
  return { w, sent, receive, internals };
}

describe("the island frame's Tauri stand-in", () => {
  it("is only on in a frame asked with ?host=island", () => {
    const self = { location: { search: "?host=island" } } as unknown as Window;
    (self as { parent: Window }).parent = self;
    expect(islandFrame(self)).toBe(false);
    const framed = { parent: {}, location: { search: "?host=island" } } as unknown as Window;
    expect(islandFrame(framed)).toBe(true);
    const other = { parent: {}, location: { search: "" } } as unknown as Window;
    expect(islandFrame(other)).toBe(false);
  });

  it("survives Tauri's own script assigning its internals later", () => {
    const { w, internals } = setup();
    const g = w as unknown as Record<string, unknown>;
    const tauri = { invoke: () => Promise.resolve("tauri"), transformCallback: () => 0 };
    // A plain assignment, as Tauri's init script does (throws only in strict mode).
    try {
      g.__TAURI_INTERNALS__ = tauri;
    } catch {
      /* strict mode: refused loudly, which is just as good */
    }
    try {
      g.__TAURI_EVENT_PLUGIN_INTERNALS__ = {};
    } catch {
      /* same */
    }
    expect(g.__TAURI_INTERNALS__).toBe(internals);
    expect((g.__TAURI_EVENT_PLUGIN_INTERNALS__ as { unregisterListener?: unknown }).unregisterListener).toBeTypeOf("function");
  });

  it("can still be replaced on purpose by redefining the property", () => {
    const { w } = setup();
    const mine = { invoke: () => Promise.resolve(1) };
    Object.defineProperty(w, "__TAURI_INTERNALS__", { value: mine, configurable: true });
    expect((w as unknown as Record<string, unknown>).__TAURI_INTERNALS__).toBe(mine);
  });

  it("sends invokes to the island and settles them by its answer", async () => {
    const { sent, receive, internals } = setup();
    const a = internals.invoke("plugin:mixtape|queue_get", { n: 1 });
    const b = internals.invoke("plugin:mixtape|play");
    const [ia, ib] = sent as Extract<FromFrame, { t: "invoke" }>[];
    expect(ia).toMatchObject({ t: "invoke", cmd: "plugin:mixtape|queue_get", args: { n: 1 } });
    receive({
      mt: 1,
      t: "result",
      id: ib.id,
      ok: false,
      value: { kind: "notFound", message: "x" },
    });
    receive({ mt: 1, t: "result", id: ia.id, ok: true, value: [1, 2] });
    await expect(a).resolves.toEqual([1, 2]);
    await expect(b).rejects.toEqual({ kind: "notFound", message: "x" });
  });

  it("passes channels as Tauri does and delivers their messages", () => {
    const { sent, receive, internals } = setup();
    const got: unknown[] = [];
    const id = internals.transformCallback((raw) => got.push(raw));
    const channel = { toJSON: () => `__CHANNEL__:${id}` };
    void internals.invoke("plugin:mixtape|player_subscribe", { channel });
    expect((sent[0] as { args: unknown }).args).toEqual({ channel: `__CHANNEL__:${id}` });
    receive({ mt: 1, t: "cb", id, raw: { index: 0, message: "s" } });
    expect(got).toEqual([{ index: 0, message: "s" }]);
    internals.unregisterCallback(id);
    receive({ mt: 1, t: "cb", id, raw: { index: 1, message: "late" } });
    expect(got).toHaveLength(1);
    expect(sent.at(-1)).toEqual({ mt: 1, t: "drop", id });
  });

  it("bridges in the island even when Tauri gave the frame its own IPC, but not under the mock", () => {
    expect(bridgeFor(true, "?host=island")).toBe(true);
    expect(bridgeFor(true, "?mock&host=island")).toBe(false);
    expect(bridgeFor(false, "")).toBe(false);
  });

  it("leaves a locked Tauri object alone without throwing, and the app's IPC still rides the bridge", async () => {
    const w = {} as Window & typeof globalThis;
    const tauriCalls: string[] = [];
    // What WebView2 injects into the island's frame: full, non-writable, non-configurable.
    const tauri = { invoke: (cmd: string) => (tauriCalls.push(cmd), Promise.resolve()), transformCallback: () => 0 };
    Object.defineProperty(w, "__TAURI_INTERNALS__", { value: tauri, configurable: false, writable: false });
    const sent: FromFrame[] = [];
    let receive!: (m: Parameters<ReturnType<typeof install>>[0]) => void;
    expect(() => (receive = install(w, (m) => sent.push(m)))).not.toThrow();
    expect((w as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__).toBe(tauri);

    const ipc = await import("./index");
    const answer = (value: unknown) => {
      const last = sent.at(-1) as { t: string; id: number };
      receive({ mt: 1, t: "result", id: last.id, ok: true, value });
    };

    // invoke
    const p = ipc.invoke<number>("library_search", { query: "x" });
    await Promise.resolve();
    await Promise.resolve();
    expect(sent.at(-1)).toMatchObject({ t: "invoke", cmd: "plugin:mixtape|library_search", args: { query: "x" } });
    answer(7);
    expect(await p).toBe(7);

    // Channel: in order, through `cb` messages.
    const got: string[] = [];
    const channel = new ipc.Channel<string>();
    channel.onmessage = (m) => got.push(m);
    void ipc.invoke("player_subscribe", { channel });
    await Promise.resolve();
    await Promise.resolve();
    const sub = sent.at(-1) as { args: { channel: string } };
    const id = Number(sub.args.channel.replace("__CHANNEL__:", ""));
    receive({ mt: 1, t: "cb", id, raw: { index: 1, message: "second" } });
    receive({ mt: 1, t: "cb", id, raw: { index: 0, message: "first" } });
    expect(got).toEqual(["first", "second"]);

    // on(): plugin:event|listen, payloads through `cb`, unlisten tells the island.
    const events: number[] = [];
    const un = ipc.on<number>("library:changed", (n) => events.push(n));
    await Promise.resolve();
    const listenMsg = sent.at(-1) as { cmd: string; args: { event: string; handler: number } };
    expect(listenMsg).toMatchObject({ cmd: "plugin:event|listen", args: { event: "library:changed" } });
    answer(42);
    const unlisten = await un;
    receive({ mt: 1, t: "cb", id: listenMsg.args.handler, raw: { event: "library:changed", id: 42, payload: 5 } });
    expect(events).toEqual([5]);
    unlisten();
    expect(sent.at(-1)).toMatchObject({ cmd: "plugin:event|unlisten", args: { event: "library:changed", eventId: 42 } });

    expect(tauriCalls).toEqual([]);
  });

  it("a once callback runs once", () => {
    const { receive, internals } = setup();
    let n = 0;
    const id = internals.transformCallback(() => n++, true);
    receive({ mt: 1, t: "cb", id, raw: null });
    receive({ mt: 1, t: "cb", id, raw: null });
    expect(n).toBe(1);
  });
});
