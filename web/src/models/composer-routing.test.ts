import { describe, expect, it, vi } from "vitest";
import { dispatchComposer } from "./composer-routing";
const handlers = () => ({ commands: vi.fn<(content: string) => void>(), protection: vi.fn<(content: string) => void>(), queue: vi.fn<(content: string) => void>(), cancel: vi.fn<(content: string) => void>(), send: vi.fn<(content: string) => void>() });
describe("composer submission routing", () => {
  it("dispatches a completed typed slash command to local protection, never palette, model or busy queue", async () => {
    const text = "/protect /tmp/private notes.md";
    for (const running of [false, true]) {
      const routes = handlers(); let draft = "";
      for (const character of text) draft += character;
      await dispatchComposer(draft, running, routes);
      expect(routes.protection).toHaveBeenCalledExactlyOnceWith(text);
      for (const route of ["commands", "queue", "cancel", "send"] as const) expect(routes[route]).not.toHaveBeenCalled();
    }
  });
  it("keeps recognized plain-text and Chinese commands local while forwarding arguments untouched", async () => {
    for (const text of ['protect file "/tmp/a b"', "保护文件 /tmp/a", "保护目录/tmp/folder", "/protect"]) {
      const routes = handlers(); await dispatchComposer(text, true, routes);
      expect(routes.protection).toHaveBeenCalledExactlyOnceWith(text); expect(routes.queue).not.toHaveBeenCalled();
    }
  });
  it("preserves standalone slash palette and normal send, queue, cancel behavior without guessing intent", async () => {
    const routes = handlers();
    await dispatchComposer("/", false, routes); expect(routes.commands).toHaveBeenCalledOnce();
    await dispatchComposer("Please protect my files", false, routes); expect(routes.send).toHaveBeenCalledExactlyOnceWith("Please protect my files");
    await dispatchComposer("/protection is a topic", true, routes); expect(routes.queue).toHaveBeenCalledExactlyOnceWith("/protection is a topic");
    await dispatchComposer("", true, routes); expect(routes.cancel).toHaveBeenCalledOnce(); expect(routes.protection).not.toHaveBeenCalled();
  });
});
