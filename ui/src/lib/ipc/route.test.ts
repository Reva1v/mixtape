import { describe, expect, it } from "vitest";
import { APP_COMMANDS, bare, route } from "./route";

describe("route", () => {
  it("sends session commands to the mixtape plugin", () => {
    expect(route("player_toggle")).toBe("plugin:mixtape|player_toggle");
    expect(route("library_view")).toBe("plugin:mixtape|library_view");
    expect(route("host_caps")).toBe("plugin:mixtape|host_caps");
  });

  it("keeps the app's own commands and already routed names", () => {
    for (const cmd of APP_COMMANDS) expect(route(cmd)).toBe(cmd);
    expect(route("plugin:window-state|save_window_state")).toBe("plugin:window-state|save_window_state");
  });

  it("bare undoes the prefix", () => {
    expect(bare(route("queue_get"))).toBe("queue_get");
    expect(bare("app_ready")).toBe("app_ready");
  });
});
