import { describe, expect, it, vi } from "vitest";
import type { Track } from "$lib/ipc/bindings/Track";
import { LinkCardState, type LinkIo } from "./linkCardState.svelte";

const track = (id: string): Track => ({
  id,
  title: `Title ${id}`,
  artists: [{ name: "Artist" }],
  durationMs: 1000,
  state: "remote",
  liked: false,
});

function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function setup(over: Partial<LinkIo> = {}) {
  const io = {
    loadTrack: vi.fn((id: string) => Promise.resolve(track(id))),
    play: vi.fn(() => Promise.resolve()),
    queueAdd: vi.fn(() => Promise.resolve()),
    done: vi.fn(),
    failed: vi.fn(),
    ...over,
  };
  return { io, card: new LinkCardState(io) };
}

const flush = () => new Promise((r) => setTimeout(r, 0));
const link = (id: string) => ({ kind: "track" as const, id });

describe("LinkCardState", () => {
  it("starts idle and loads a track link: idle -> loading -> ready", async () => {
    const { card, io } = setup();
    expect(card.phase.status).toBe("idle");
    card.set(link("aaaaaaaaaaa"));
    expect(card.phase.status).toBe("loading");
    await flush();
    expect(card.phase).toEqual({ status: "ready", track: track("aaaaaaaaaaa") });
    expect(io.loadTrack).toHaveBeenCalledWith("aaaaaaaaaaa");
  });

  it("plays, queues next and at the end with the track's id and reports the queue ones", async () => {
    const { card, io } = setup();
    card.set(link("aaaaaaaaaaa"));
    expect(await card.act("now")).toBe(true);
    expect(await card.act("next")).toBe(true);
    expect(await card.act("end")).toBe(true);
    expect(io.play).toHaveBeenCalledWith("aaaaaaaaaaa");
    expect(io.queueAdd).toHaveBeenNthCalledWith(1, "aaaaaaaaaaa", "next");
    expect(io.queueAdd).toHaveBeenNthCalledWith(2, "aaaaaaaaaaa", "end");
    expect(vi.mocked(io.done as LinkIo["done"]).mock.calls.map((c) => c[0])).toEqual(["now", "next", "end"]);
    expect(card.busy).toBeNull();
  });

  it("Enter plays", async () => {
    const { card, io } = setup();
    card.set(link("aaaaaaaaaaa"));
    await card.play();
    expect(io.play).toHaveBeenCalledWith("aaaaaaaaaaa");
  });

  it("when link_track fails the card says so, and the action still runs once with the bare id", async () => {
    const { card, io } = setup({ loadTrack: vi.fn(() => Promise.reject(new Error("offline"))) });
    card.set(link("aaaaaaaaaaa"));
    await flush();
    expect(card.phase).toEqual({ status: "failed", message: "offline" });
    expect(await card.act("now")).toBe(true);
    expect(io.play).toHaveBeenCalledTimes(1);
    expect(io.play).toHaveBeenCalledWith("aaaaaaaaaaa");
    expect(io.failed).not.toHaveBeenCalled();
  });

  it("when link_track and the action both fail, the error goes out once", async () => {
    const boom = new Error("no such track");
    const { card, io } = setup({
      loadTrack: vi.fn(() => Promise.reject(new Error("offline"))),
      queueAdd: vi.fn(() => Promise.reject(boom)),
    });
    card.set(link("aaaaaaaaaaa"));
    await flush();
    expect(await card.act("end")).toBe(false);
    expect(io.failed).toHaveBeenCalledTimes(1);
    expect(io.failed).toHaveBeenCalledWith(boom);
    expect(io.done).not.toHaveBeenCalled();
    expect(card.busy).toBeNull();
  });

  it("an action pressed while loading waits for the answer, shows busy meanwhile", async () => {
    const load = deferred<Track>();
    const { card, io } = setup({ loadTrack: vi.fn(() => load.promise) });
    card.set(link("aaaaaaaaaaa"));
    const pressed = card.act("now");
    await flush();
    expect(io.play).not.toHaveBeenCalled();
    expect(card.busy).toBe("now");
    load.resolve(track("aaaaaaaaaaa"));
    expect(await pressed).toBe(true);
    expect(io.play).toHaveBeenCalledTimes(1);
    expect(card.busy).toBeNull();
    expect(card.phase.status).toBe("ready");
  });

  it("an action pressed while loading still runs when the loading then fails", async () => {
    const load = deferred<Track>();
    const { card, io } = setup({ loadTrack: vi.fn(() => load.promise) });
    card.set(link("aaaaaaaaaaa"));
    const pressed = card.act("next");
    load.reject(new Error("offline"));
    expect(await pressed).toBe(true);
    expect(io.queueAdd).toHaveBeenCalledWith("aaaaaaaaaaa", "next");
    expect(card.phase.status).toBe("failed");
  });

  it("a second press while one is running does nothing", async () => {
    const load = deferred<Track>();
    const { card, io } = setup({ loadTrack: vi.fn(() => load.promise) });
    card.set(link("aaaaaaaaaaa"));
    const first = card.act("now");
    expect(await card.act("now")).toBe(false);
    expect(await card.act("end")).toBe(false);
    load.resolve(track("aaaaaaaaaaa"));
    await first;
    expect(io.play).toHaveBeenCalledTimes(1);
    expect(io.queueAdd).not.toHaveBeenCalled();
  });

  it("a newer link replaces the older one and the older one's late answer is ignored", async () => {
    const first = deferred<Track>();
    const second = deferred<Track>();
    const answers = { aaaaaaaaaaa: first, bbbbbbbbbbb: second } as Record<
      string,
      ReturnType<typeof deferred<Track>>
    >;
    const { card } = setup({ loadTrack: vi.fn((id: string) => answers[id].promise) });
    card.set(link("aaaaaaaaaaa"));
    card.set(link("bbbbbbbbbbb"));
    second.resolve(track("bbbbbbbbbbb"));
    await flush();
    first.resolve(track("aaaaaaaaaaa"));
    await flush();
    expect(card.link).toEqual(link("bbbbbbbbbbb"));
    expect(card.phase).toEqual({ status: "ready", track: track("bbbbbbbbbbb") });
  });

  it("the older link's late failure does not turn the newer card into an error", async () => {
    const first = deferred<Track>();
    const { card } = setup({
      loadTrack: vi.fn((id: string) =>
        id === "aaaaaaaaaaa" ? first.promise : Promise.resolve(track(id)),
      ),
    });
    card.set(link("aaaaaaaaaaa"));
    card.set(link("bbbbbbbbbbb"));
    await flush();
    first.reject(new Error("late"));
    await flush();
    expect(card.phase.status).toBe("ready");
  });

  it("an action waits for the newest link's answer, not the replaced one's", async () => {
    const first = deferred<Track>();
    const { card, io } = setup({
      loadTrack: vi.fn((id: string) =>
        id === "aaaaaaaaaaa" ? first.promise : Promise.resolve(track(id)),
      ),
    });
    card.set(link("aaaaaaaaaaa"));
    card.set(link("bbbbbbbbbbb"));
    expect(await card.act("now")).toBe(true);
    expect(io.play).toHaveBeenCalledWith("bbbbbbbbbbb");
  });

  it("the same link again doesn't reload", async () => {
    const { card, io } = setup();
    card.set(link("aaaaaaaaaaa"));
    card.set(link("aaaaaaaaaaa"));
    await flush();
    expect(io.loadTrack).toHaveBeenCalledTimes(1);
  });

  it("clearing the link hides the card; an album link needs no loading and no actions", async () => {
    const { card, io } = setup();
    card.set(link("aaaaaaaaaaa"));
    card.set(null);
    expect(card.link).toBeNull();
    expect(card.phase.status).toBe("idle");
    card.set({ kind: "album", id: "MPREb_x" });
    expect(card.phase.status).toBe("idle");
    expect(io.loadTrack).toHaveBeenCalledTimes(1);
    expect(await card.act("now")).toBe(false);
    expect(io.play).not.toHaveBeenCalled();
  });

  it("acting with no link does nothing", async () => {
    const { card, io } = setup();
    expect(await card.play()).toBe(false);
    expect(io.play).not.toHaveBeenCalled();
  });
});
