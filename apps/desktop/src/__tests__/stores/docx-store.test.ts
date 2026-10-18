import { beforeEach, describe, expect, it } from "vitest";
import { modeFor, useDocxPrefsStore } from "@/stores/docx-store";

describe("the Word tab's mode, remembered per file", () => {
  beforeEach(() => {
    useDocxPrefsStore.setState({ modes: {} });
  });

  it("opens a file in Editing at first", () => {
    expect(modeFor({}, "/p/a.docx")).toBe("editing");
  });

  it("remembers the mode chosen for each file", () => {
    const { setMode } = useDocxPrefsStore.getState();
    setMode("/p/a.docx", "suggesting");
    setMode("/p/b.docx", "viewing");
    const { modes } = useDocxPrefsStore.getState();
    expect(modeFor(modes, "/p/a.docx")).toBe("suggesting");
    expect(modeFor(modes, "/p/b.docx")).toBe("viewing");
    expect(modeFor(modes, "/p/c.docx")).toBe("editing");
  });

  it("forgets a file set back to Editing, the default", () => {
    const { setMode } = useDocxPrefsStore.getState();
    setMode("/p/a.docx", "viewing");
    setMode("/p/a.docx", "editing");
    expect(useDocxPrefsStore.getState().modes).toEqual({});
  });

  it("is kept across launches", () => {
    useDocxPrefsStore.getState().setMode("/p/a.docx", "suggesting");
    const saved = localStorage.getItem("thelab-docx-tabs") ?? "";
    expect(JSON.parse(saved).state.modes).toEqual({
      "/p/a.docx": "suggesting",
    });
  });

  it("follows a file or folder renamed or moved", () => {
    const { setMode } = useDocxPrefsStore.getState();
    setMode("/p/sub/a.docx", "suggesting");
    setMode("/p/b.docx", "viewing");
    useDocxPrefsStore.getState().movePaths("/p/sub", "/p/renamed");
    useDocxPrefsStore.getState().movePaths("/p/b.docx", "/p/c.docx");
    expect(useDocxPrefsStore.getState().modes).toEqual({
      "/p/renamed/a.docx": "suggesting",
      "/p/c.docx": "viewing",
    });
  });

  it("treats anything unknown as Editing", () => {
    expect(modeFor({ "/p/a.docx": "typing" as never }, "/p/a.docx")).toBe(
      "editing",
    );
  });
});
