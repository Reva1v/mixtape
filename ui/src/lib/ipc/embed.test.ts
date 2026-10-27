import { describe, expect, it } from "vitest";
import { install, islandFrame, type FromFrame } from "./embed";

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

  it("a once callback runs once", () => {
    const { receive, internals } = setup();
    let n = 0;
    const id = internals.transformCallback(() => n++, true);
    receive({ mt: 1, t: "cb", id, raw: null });
    receive({ mt: 1, t: "cb", id, raw: null });
    expect(n).toBe(1);
  });
});
