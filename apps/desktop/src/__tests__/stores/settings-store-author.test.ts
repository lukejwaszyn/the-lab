import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  fillTrackChangesAuthorOnce,
  useSettingsStore,
} from "@/stores/settings-store";

describe("Your name for tracked changes", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    useSettingsStore.setState({
      trackChangesAuthor: "",
      trackChangesAuthorOffered: false,
    });
  });

  it("is filled once from the account's full name", async () => {
    vi.mocked(invoke).mockResolvedValue("Ada Example");
    await fillTrackChangesAuthorOnce();
    expect(invoke).toHaveBeenCalledWith("account_full_name");
    expect(useSettingsStore.getState().trackChangesAuthor).toBe("Ada Example");
    // Cleared by the user, it stays cleared.
    useSettingsStore.getState().setTrackChangesAuthor("");
    await fillTrackChangesAuthorOnce();
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(useSettingsStore.getState().trackChangesAuthor).toBe("");
  });

  it("stays empty when the name cannot be read", async () => {
    vi.mocked(invoke).mockRejectedValue(new Error("no id"));
    await fillTrackChangesAuthorOnce();
    expect(useSettingsStore.getState().trackChangesAuthor).toBe("");
    expect(useSettingsStore.getState().trackChangesAuthorOffered).toBe(true);
  });

  it("does not replace a name the user set first", async () => {
    useSettingsStore.setState({ trackChangesAuthor: "A. Writer" });
    vi.mocked(invoke).mockResolvedValue("Ada Example");
    await fillTrackChangesAuthorOnce();
    expect(useSettingsStore.getState().trackChangesAuthor).toBe("A. Writer");
  });
});
