import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { createLogger } from "@/lib/debug/logger";
import type { PermissionRequest } from "@/lib/permission-requests";

/**
 * The Lab: permission requests waiting in each chat tab, and the tools the
 * user allowed for the rest of a chat. Nothing here is saved: "Allow for this
 * chat" lasts until the tab starts another chat, loads another session, or
 * closes.
 */

const log = createLogger("claude-permission");

export type PermissionChoice = "allow-once" | "allow-chat" | "deny";

const NO_REQUESTS: PermissionRequest[] = [];

interface PermissionState {
  /** tab id -> requests waiting for an answer, oldest first */
  pendingByTab: Record<string, PermissionRequest[]>;
  /** tab id -> tool names allowed for the rest of that chat */
  allowedToolsByTab: Record<string, string[]>;

  /** A new request arrived. Answers it at once if the tool is allowed. */
  receive: (
    tabId: string,
    request: PermissionRequest,
  ) => "pending" | "auto-allowed";
  /** The user clicked one of the card's buttons. */
  answer: (
    tabId: string,
    requestId: string,
    choice: PermissionChoice,
  ) => Promise<void>;
  /** Claude withdrew a request. */
  withdraw: (tabId: string, requestId: string) => void;
  /** The run stopped: its unanswered requests just disappear. */
  clearPending: (tabId: string) => void;
  /** The tab starts another chat or closes: forget its requests and allows. */
  forgetTab: (tabId: string) => void;
  reset: () => void;
}

async function sendAnswer(
  tabId: string,
  requestId: string,
  allow: boolean,
): Promise<boolean> {
  try {
    return await invoke<boolean>("respond_claude_permission", {
      tabId,
      requestId,
      allow,
    });
  } catch (err) {
    log.warn("failed to answer permission request", {
      tabId,
      requestId,
      error: String(err),
    });
    return false;
  }
}

function withoutKey<T>(record: Record<string, T>, key: string) {
  if (!(key in record)) return record;
  const next = { ...record };
  delete next[key];
  return next;
}

export const usePermissionStore = create<PermissionState>()((set, get) => ({
  pendingByTab: {},
  allowedToolsByTab: {},

  receive: (tabId, request) => {
    const allowed = get().allowedToolsByTab[tabId] ?? [];
    if (allowed.includes(request.toolName)) {
      log.info(`[${tabId}] auto-allowing ${request.toolName} for this chat`);
      void sendAnswer(tabId, request.requestId, true);
      return "auto-allowed";
    }
    set((state) => {
      const pending = state.pendingByTab[tabId] ?? [];
      if (pending.some((r) => r.requestId === request.requestId)) return {};
      return {
        pendingByTab: { ...state.pendingByTab, [tabId]: [...pending, request] },
      };
    });
    return "pending";
  },

  answer: async (tabId, requestId, choice) => {
    const pending = get().pendingByTab[tabId] ?? [];
    const request = pending.find((r) => r.requestId === requestId);
    if (!request) return;

    const allow = choice !== "deny";
    // "Allow for this chat" also covers other requests for the same tool that
    // are already waiting, e.g. two edits Claude asked for at once.
    const answered =
      choice === "allow-chat"
        ? pending.filter((r) => r.toolName === request.toolName)
        : [request];
    const answeredIds = new Set(answered.map((r) => r.requestId));

    set((state) => {
      const remaining = (state.pendingByTab[tabId] ?? []).filter(
        (r) => !answeredIds.has(r.requestId),
      );
      const allowedTools = state.allowedToolsByTab[tabId] ?? [];
      return {
        pendingByTab: { ...state.pendingByTab, [tabId]: remaining },
        allowedToolsByTab:
          choice === "allow-chat" && !allowedTools.includes(request.toolName)
            ? {
                ...state.allowedToolsByTab,
                [tabId]: [...allowedTools, request.toolName],
              }
            : state.allowedToolsByTab,
      };
    });

    await Promise.all(
      answered.map((r) => sendAnswer(tabId, r.requestId, allow)),
    );
  },

  withdraw: (tabId, requestId) => {
    set((state) => {
      const pending = state.pendingByTab[tabId];
      if (!pending?.some((r) => r.requestId === requestId)) return {};
      return {
        pendingByTab: {
          ...state.pendingByTab,
          [tabId]: pending.filter((r) => r.requestId !== requestId),
        },
      };
    });
  },

  clearPending: (tabId) => {
    set((state) => {
      if (!state.pendingByTab[tabId]?.length) return {};
      return { pendingByTab: withoutKey(state.pendingByTab, tabId) };
    });
  },

  forgetTab: (tabId) => {
    set((state) => ({
      pendingByTab: withoutKey(state.pendingByTab, tabId),
      allowedToolsByTab: withoutKey(state.allowedToolsByTab, tabId),
    }));
  },

  reset: () => set({ pendingByTab: {}, allowedToolsByTab: {} }),
}));

/** The requests a tab is waiting on (a stable empty list when none). */
export function selectPendingRequests(tabId: string) {
  return (state: PermissionState) => state.pendingByTab[tabId] ?? NO_REQUESTS;
}
