import { describe, expect, it } from "vitest";
import { normalizeTheme, themeScheme } from "./themes";

describe("appearance preferences", () => {
  it("retains existing choices and fails unknown stored values back to the system", () => {
    expect(normalizeTheme("dark")).toBe("dark");
    expect(normalizeTheme("terminal")).toBe("terminal");
    expect(normalizeTheme("obsolete")).toBe("system");
    expect(normalizeTheme(null)).toBe("system");
  });
  it("follows the operating system only for System", () => {
    expect(themeScheme("system", true)).toBe("dark");
    expect(themeScheme("system", false)).toBe("light");
    expect(themeScheme("light", true)).toBe("light");
    for (const theme of ["dark", "terminal", "midnight", "nord"] as const) {
      expect(themeScheme(theme, false)).toBe("dark");
    }
  });
});
