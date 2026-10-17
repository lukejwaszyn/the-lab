import type { FC } from "react";
import { ShieldAlertIcon } from "lucide-react";
import type { PermissionRequest } from "@/lib/permission-requests";
import { usePermissionStore } from "@/stores/permission-store";

/**
 * The Lab: Claude asks before it acts. One card per waiting request, with
 * Allow once, Allow for this chat, and Deny.
 */
export const PermissionRequestCard: FC<{
  tabId: string;
  request: PermissionRequest;
}> = ({ tabId, request }) => {
  const answer = usePermissionStore((s) => s.answer);

  return (
    <div
      role="group"
      aria-label={`Claude asks permission: ${request.summary}`}
      className="fade-in slide-in-from-bottom-1 my-1.5 animate-in rounded-lg border border-amber-500/40 bg-amber-500/10 text-sm duration-150"
    >
      <div className="flex items-center gap-2 px-3 py-2">
        <ShieldAlertIcon className="size-3.5 shrink-0 text-amber-600 dark:text-amber-400" />
        <span className="font-medium text-amber-700 dark:text-amber-300">
          Claude asks to use {request.displayName}
        </span>
      </div>
      <div className="border-amber-500/20 border-t px-3 py-2">
        <p className="break-words text-foreground text-sm">{request.summary}</p>
        {request.detail && (
          <pre className="mt-1.5 max-h-32 overflow-auto whitespace-pre-wrap break-words rounded bg-background/60 px-2 py-1 font-mono text-muted-foreground text-xs">
            {request.detail}
          </pre>
        )}
      </div>
      <div className="flex flex-wrap gap-2 border-amber-500/20 border-t px-3 py-2">
        <button
          type="button"
          className="rounded-md bg-amber-500 px-2.5 py-1 font-medium text-background text-xs hover:bg-amber-500/90"
          onClick={() => void answer(tabId, request.requestId, "allow-once")}
        >
          Allow once
        </button>
        <button
          type="button"
          className="rounded-md border border-amber-500/30 px-2.5 py-1 text-amber-700 text-xs hover:bg-amber-500/10 dark:text-amber-300"
          title={`Allow ${request.displayName} without asking again in this chat`}
          onClick={() => void answer(tabId, request.requestId, "allow-chat")}
        >
          Allow for this chat
        </button>
        <button
          type="button"
          className="rounded-md border border-border px-2.5 py-1 text-foreground text-xs hover:bg-muted"
          onClick={() => void answer(tabId, request.requestId, "deny")}
        >
          Deny
        </button>
      </div>
    </div>
  );
};
