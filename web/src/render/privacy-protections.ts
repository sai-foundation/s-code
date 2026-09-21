import type { PrivacyProtections } from "../models/privacy-protections";
import { protectionGroups, protectionPath } from "../models/protection-summary";

export function renderProtections(target: HTMLElement, store: PrivacyProtections, compact: boolean, remove: (id: string) => void, root: string | null = null) {
  target.replaceChildren();
  if (!store.policy) { const note = document.createElement("p"); note.textContent = store.error || "Loading protection policy…"; target.append(note); return; }
  const rules = store.policy.rules;
  if (!rules.length) { const note = document.createElement("p"); note.textContent = "No protected paths in this account."; target.append(note); return; }
  for (const group of protectionGroups(rules, root)) {
    const section = document.createElement("section"); section.className = "protection-group";
    const heading = document.createElement("h3"); heading.textContent = `${group.label} · ${group.rules.length}`; section.append(heading);
    const limit = compact ? 4 : 40;
    const list = document.createElement("ul"); list.className = "protection-list";
    const draw = (page: number) => {
      list.replaceChildren();
      for (const rule of group.rules.slice(page * limit, (page + 1) * limit)) {
        const view = protectionPath(rule, root);
        const row = document.createElement("li");
        const path = document.createElement("span"); path.title = view.fullPath; path.tabIndex = 0; path.setAttribute("aria-label", `${rule.kind}: ${view.fullPath}`);
        const name = document.createElement("strong"); name.textContent = view.name;
        const location = document.createElement("small"); location.textContent = view.location;
        path.append(name, location);
        if (view.resolvedPath) { const resolved = document.createElement("small"); resolved.textContent = `Resolves to ${view.resolvedPath}`; path.append(resolved); }
        row.append(path);
        if (!compact) {
          const button = document.createElement("button"); button.type = "button"; button.textContent = "Unprotect"; button.setAttribute("aria-label", `Unprotect ${rule.path}`); button.disabled = store.saving || store.loading;
          button.addEventListener("click", () => remove(rule.id)); row.append(button);
        }
        list.append(row);
      }
    };
    section.append(list); draw(0); target.append(section);
    if (group.rules.length > limit) {
      if (compact) { const note = document.createElement("p"); note.textContent = `Showing ${limit} of ${group.rules.length}`; section.append(note); }
      else {
        let page = 0; const nav = document.createElement("nav"); nav.className = "privacy-event-pagination"; nav.setAttribute("aria-label", `${group.label} protected paths`);
        const previous = document.createElement("button"); previous.type = "button"; previous.textContent = "Previous";
        const next = document.createElement("button"); next.type = "button"; next.textContent = "Next";
        const label = document.createElement("span"); label.setAttribute("role", "status");
        const update = () => { previous.disabled = page === 0; next.disabled = (page + 1) * limit >= group.rules.length; label.textContent = `Page ${page + 1} of ${Math.ceil(group.rules.length / limit)}`; draw(page); };
        previous.addEventListener("click", () => { page--; update(); }); next.addEventListener("click", () => { page++; update(); }); nav.append(previous, label, next); section.append(nav); update();
      }
    }
  }
}
