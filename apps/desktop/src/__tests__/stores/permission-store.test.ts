import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { PermissionRequest } from "@/lib/permission-requests";
import {
  selectPendingRequests,
  usePermissionStore,
} from "@/stores/permission-store";

const invokeMock = vi.mocked(invoke);

function request(
  requestId: string,
  toolName = "Edit",
  summary = `Edit main.tex (${requestId})`,
): PermissionRequest {
  return { requestId, toolName, displayName: toolName, summary, detail: null };
}

function pending(tabId: string) {
  return selectPendingRequests(tabId)(usePermissionStore.getState());
}

function answers() {
  return invokeMock.mock.calls
    .filter(([command]) => command === "respond_claude_permission")
    .map(([, args]) => args);
}

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(true);
  usePermissionStore.getState().reset();
});

describe("permission cards", () => {
  it("shows a new request as a pending card without answering it", () => {
    const outcome = usePermissionStore
      .getState()
      .receive("tab-1", request("r1"));

    expect(outcome).toBe("pending");
    expect(pending("tab-1").map((r) => r.requestId)).toEqual(["r1"]);
    expect(answers()).toEqual([]);
  });

  it("ignores a request it already shows", () => {
    const store = usePermissionStore.getState();
    store.receive("tab-1", request("r1"));
    store.receive("tab-1", request("r1"));
    expect(pending("tab-1")).toHaveLength(1);
  });

  it("Allow once answers allow and removes only that card", async () => {
    const store = usePermissionStore.getState();
    store.receive("tab-1", request("r1"));
    store.receive("tab-1", request("r2"));

    await store.answer("tab-1", "r1", "allow-once");

    expect(answers()).toEqual([
      { tabId: "tab-1", requestId: "r1", allow: true },
    ]);
    expect(pending("tab-1").map((r) => r.requestId)).toEqual(["r2"]);
    expect(usePermissionStore.getState().allowedToolsByTab["tab-1"]).toBe(
      undefined,
    );
  });

  it("Deny answers deny and removes the card", async () => {
    const store = usePermissionStore.getState();
    store.receive("tab-1", request("r1", "Bash", "Run a command"));

    await store.answer("tab-1", "r1", "deny");

    expect(answers()).toEqual([
      { tabId: "tab-1", requestId: "r1", allow: false },
    ]);
    expect(pending("tab-1")).toEqual([]);
  });

  it("answers each card only once", async () => {
    const store = usePermissionStore.getState();
    store.receive("tab-1", request("r1"));

    await store.answer("tab-1", "r1", "allow-once");
    await store.answer("tab-1", "r1", "deny");

    expect(answers()).toHaveLength(1);
  });

  it("drops the card even when the answer cannot be delivered", async () => {
    invokeMock.mockRejectedValueOnce(new Error("stdin closed"));
    const store = usePermissionStore.getState();
    store.receive("tab-1", request("r1"));

    await store.answer("tab-1", "r1", "allow-once");

    expect(pending("tab-1")).toEqual([]);
  });

  it("removes a request Claude withdraws", () => {
    const store = usePermissionStore.getState();
    store.receive("tab-1", request("r1"));
    store.receive("tab-1", request("r2"));

    store.withdraw("tab-1", "r1");

    expect(pending("tab-1").map((r) => r.requestId)).toEqual(["r2"]);
  });

  it("lets pending requests disappear when the run stops", () => {
    const store = usePermissionStore.getState();
    store.receive("tab-1", request("r1"));
    store.receive("tab-2", request("r2"));

    store.clearPending("tab-1");

    expect(pending("tab-1")).toEqual([]);
    expect(pending("tab-2")).toHaveLength(1);
    expect(answers()).toEqual([]);
  });
});

describe("Allow for this chat", () => {
  it("remembers the tool for this tab and answers later requests itself", async () => {
    const store = usePermissionStore.getState();
    store.receive("tab-1", request("r1", "Edit"));
    await store.answer("tab-1", "r1", "allow-chat");

    const outcome = usePermissionStore
      .getState()
      .receive("tab-1", request("r2", "Edit"));

    expect(outcome).toBe("auto-allowed");
    expect(pending("tab-1")).toEqual([]);
    expect(answers()).toEqual([
      { tabId: "tab-1", requestId: "r1", allow: true },
      { tabId: "tab-1", requestId: "r2", allow: true },
    ]);
  });

  it("covers only that tool", async () => {
    const store = usePermissionStore.getState();
    store.receive("tab-1", request("r1", "Edit"));
    await store.answer("tab-1", "r1", "allow-chat");

    const outcome = usePermissionStore
      .getState()
      .receive("tab-1", request("r2", "Bash"));

    expect(outcome).toBe("pending");
    expect(pending("tab-1").map((r) => r.toolName)).toEqual(["Bash"]);
  });

  it("covers only that chat tab", async () => {
    const store = usePermissionStore.getState();
    store.receive("tab-1", request("r1", "Edit"));
    await store.answer("tab-1", "r1", "allow-chat");

    const outcome = usePermissionStore
      .getState()
      .receive("tab-2", request("r2", "Edit"));

    expect(outcome).toBe("pending");
    expect(pending("tab-2")).toHaveLength(1);
  });

  it("also answers other waiting requests for the same tool", async () => {
    const store = usePermissionStore.getState();
    store.receive("tab-1", request("r1", "Edit"));
    store.receive("tab-1", request("r2", "Bash"));
    store.receive("tab-1", request("r3", "Edit"));

    await store.answer("tab-1", "r1", "allow-chat");

    expect(answers()).toEqual([
      { tabId: "tab-1", requestId: "r1", allow: true },
      { tabId: "tab-1", requestId: "r3", allow: true },
    ]);
    expect(pending("tab-1").map((r) => r.requestId)).toEqual(["r2"]);
  });

  it("ends when the tab starts another chat", async () => {
    const store = usePermissionStore.getState();
    store.receive("tab-1", request("r1", "Edit"));
    await store.answer("tab-1", "r1", "allow-chat");

    usePermissionStore.getState().forgetTab("tab-1");

    expect(
      usePermissionStore.getState().receive("tab-1", request("r2", "Edit")),
    ).toBe("pending");
  });
});
