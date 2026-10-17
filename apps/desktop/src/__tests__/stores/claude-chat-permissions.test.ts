import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import {
  CLAUDE_CODE_PROVIDER_ID,
  type TabState,
  useClaudeChatStore,
} from "@/stores/claude-chat-store";
import { usePermissionStore } from "@/stores/permission-store";

function tab(id: string, overrides: Partial<TabState> = {}): TabState {
  return {
    id,
    title: "New Chat",
    projectPath: "/project",
    sessionId: null,
    providerKey: CLAUDE_CODE_PROVIDER_ID,
    sessionProviderKey: null,
    messages: [],
    isStreaming: false,
    streamingStartedAt: null,
    error: null,
    totalInputTokens: 0,
    totalOutputTokens: 0,
    draft: { input: "", pinnedContexts: [] },
    ...overrides,
  };
}

function allowEdits(tabId: string) {
  usePermissionStore.setState((state) => ({
    allowedToolsByTab: { ...state.allowedToolsByTab, [tabId]: ["Edit"] },
  }));
}

function waitingRequest(tabId: string) {
  usePermissionStore.getState().receive(tabId, {
    requestId: `req-${tabId}`,
    toolName: "Bash",
    displayName: "Bash",
    summary: "Run a command",
    detail: "ls",
  });
}

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockResolvedValue(undefined);
  usePermissionStore.getState().reset();
  useClaudeChatStore.setState({
    tabs: [tab("tab-a"), tab("tab-b")],
    activeTabId: "tab-a",
    activeProjectPath: "/project",
    isStreaming: false,
  });
});

describe("permissions follow the chat's lifetime", () => {
  it("New Chat in a tab ends its Allow for this chat", () => {
    allowEdits("tab-a");
    allowEdits("tab-b");

    useClaudeChatStore.getState().newSession();

    const allowed = usePermissionStore.getState().allowedToolsByTab;
    expect(allowed["tab-a"]).toBeUndefined();
    expect(allowed["tab-b"]).toEqual(["Edit"]);
  });

  it("closing a tab forgets its allows", () => {
    allowEdits("tab-b");

    useClaudeChatStore.getState().closeTab("tab-b");

    expect(
      usePermissionStore.getState().allowedToolsByTab["tab-b"],
    ).toBeUndefined();
  });

  it("Stop makes a waiting request disappear", async () => {
    useClaudeChatStore.setState({
      tabs: [tab("tab-a", { isStreaming: true }), tab("tab-b")],
      isStreaming: true,
    });
    waitingRequest("tab-a");
    waitingRequest("tab-b");

    await useClaudeChatStore.getState().cancelExecution("tab-a");

    const pending = usePermissionStore.getState().pendingByTab;
    expect(pending["tab-a"]).toBeUndefined();
    expect(pending["tab-b"]).toHaveLength(1);
  });

  it("closing or switching the project stops runs left waiting", () => {
    useClaudeChatStore.setState({
      tabs: [tab("tab-a", { isStreaming: true }), tab("tab-b")],
      isStreaming: true,
    });
    waitingRequest("tab-a");

    useClaudeChatStore.getState().resetForProject(null);

    expect(vi.mocked(invoke)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("cancel_claude_execution", {
      tabId: "tab-a",
    });
    expect(usePermissionStore.getState().pendingByTab["tab-a"]).toBeUndefined();
  });
});
