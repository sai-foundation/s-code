import { afterEach, describe, expect, it, vi } from "vitest";
import { prepareAttachment } from "./attachments";

class DeferredFileReader extends EventTarget {
  static pending: DeferredFileReader[] = [];
  result: string | null = null;
  error: Error | null = null;
  readAsDataURL() { DeferredFileReader.pending.push(this); }
  complete() { this.result = "data:text/plain;base64,cHJpdmF0ZQ=="; this.dispatchEvent(new Event("load")); }
}
afterEach(() => { vi.unstubAllGlobals(); DeferredFileReader.pending = []; });

describe("attachment account boundary", () => {
  it("does not submit or render old account content after a deferred FileReader completes", async () => {
    vi.stubGlobal("FileReader", DeferredFileReader);
    let account = "alice";
    const submitted: unknown[] = [];
    const rendered: unknown[] = [];
    const preparation = prepareAttachment(new File(["private"], "alice.txt", { type: "text/plain" }), () => account === "alice")
      .then((payload) => { submitted.push(payload); rendered.push("Alice message"); });
    const rejected = expect(preparation).rejects.toMatchObject({ name: "AbortError" });
    account = "bob";
    DeferredFileReader.pending[0]!.complete();
    await rejected;
    expect(submitted).toEqual([]);
    expect(rendered).toEqual([]);
  });

  it("also rejects switching sessions within one account, and allows an unchanged session", async () => {
    vi.stubGlobal("FileReader", DeferredFileReader);
    let session = "first";
    const stale = prepareAttachment(new File(["private"], "first.txt"), () => session === "first");
    const rejected = expect(stale).rejects.toMatchObject({ name: "AbortError" });
    session = "second";
    DeferredFileReader.pending[0]!.complete();
    await rejected;
    const current = prepareAttachment(new File(["private"], "second.txt"), () => session === "second");
    DeferredFileReader.pending[1]!.complete();
    await expect(current).resolves.toEqual({ file_name: "second.txt", media_type: "application/octet-stream", content_base64: "cHJpdmF0ZQ==" });
  });
});
