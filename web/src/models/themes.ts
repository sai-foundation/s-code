export const themes = [
  { id: "system", label: "System" },
  { id: "light", label: "Light" },
  { id: "dark", label: "Dark" },
  { id: "terminal", label: "Terminal" },
  { id: "midnight", label: "Midnight" },
  { id: "nord", label: "Nord" },
] as const;
export type Theme = typeof themes[number]["id"];
export function normalizeTheme(value: string | null): Theme {
  return themes.find(theme => theme.id === value)?.id ?? "system";
}
export function themeScheme(theme: Theme, systemDark: boolean): "dark" | "light" {
  return theme === "light" || (theme === "system" && !systemDark) ? "light" : "dark";
}
