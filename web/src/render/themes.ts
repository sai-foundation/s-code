import { themes, type Theme } from "../models/themes";
import light from "../theme-previews/light.png";
import dark from "../theme-previews/dark.png";
import terminal from "../theme-previews/terminal.png";
import midnight from "../theme-previews/midnight.png";
import nord from "../theme-previews/nord.png";

const previews = { light, dark, terminal, midnight, nord };
export function renderThemeOptions(container: HTMLElement, current: Theme, systemDark: boolean, select: (theme: Theme) => void) {
  // Keep the focused button alive when a selection updates every preview.
  if (!container.children.length) for (const theme of themes) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "theme-option";
    button.dataset.themeChoice = theme.id;
    button.setAttribute("aria-label", theme.label);
    const preview = document.createElement("img");
    preview.alt = `${theme.label} conversation preview`;
    preview.width = 960; preview.height = 600;
    const label = document.createElement("span");
    label.className = "theme-option-label";
    label.textContent = theme.label;
    const check = document.createElement("span");
    check.className = "theme-option-check";
    check.textContent = "✓";
    check.setAttribute("aria-hidden", "true");
    label.append(check);
    button.append(preview, label);
    button.addEventListener("click", () => select(theme.id));
    container.append(button);
  }
  container.querySelectorAll<HTMLButtonElement>("button[data-theme-choice]").forEach(button => {
    const theme = button.dataset.themeChoice as Theme;
    button.setAttribute("aria-pressed", String(theme === current));
    button.querySelector<HTMLElement>(".theme-option-check")!.hidden = theme !== current;
    button.querySelector<HTMLImageElement>("img")!.src = previews[theme === "system" ? systemDark ? "dark" : "light" : theme];
  });
}
