import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createMarkdownRenderer } from "./markdown";

describe("Markdown URL boundary", () => {
  beforeEach(() => vi.stubGlobal("window", { location: { href: "https://local.test/work/" } }));
  afterEach(() => vi.unstubAllGlobals());
  const renderer = createMarkdownRenderer({ copyText: async () => {}, highlight: () => false });

  it("rejects executable, local-file and embedded-document URLs", () => {
    for (const url of ["javascript:alert(1)", "JaVaScRiPt:alert(1)", "java\tscript:alert(1)",
      "java\nscript:alert(1)", "java\rscript:alert(1)", "\u0000javascript:alert(1)", "data:text/html,<script>alert(1)</script>",
      "data:image/svg+xml;base64,PHN2ZyBvbmxvYWQ9YWxlcnQoMSk+",
      "vbscript:msgbox(1)", "file:///etc/passwd", "blob:https://local.test/id"]) {
      expect(renderer.safeUrl(url)).toBeNull();
      expect(renderer.safeUrl(url, true)).toBeNull();
    }
    expect(renderer.safeUrl("mailto:hello@example.com", true)).toBeNull();
  });

  it("returns the same canonical URL it validated", () => {
    expect(renderer.safeUrl("<HTTPS://EXAMPLE.COM/a b>")).toBe("https://example.com/a%20b");
    expect(renderer.safeUrl("../guide")).toBe("https://local.test/guide");
    expect(renderer.safeUrl("#section")).toBe("https://local.test/work/#section");
    expect(renderer.safeUrl("javascript%3Aalert(1)")).toBe("https://local.test/work/javascript%3Aalert(1)");
    expect(renderer.safeUrl("javascript&colon;alert(1)")).toBe("https://local.test/work/javascript&colon;alert(1)");
    expect(renderer.safeUrl("//example.com/image.png", true)).toBe("https://example.com/image.png");
    expect(renderer.safeUrl("mailto:hello@example.com")).toBe("mailto:hello@example.com");
  });
});
