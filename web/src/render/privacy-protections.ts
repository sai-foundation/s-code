import type { PrivacyProtections } from "../models/privacy-protections";
export function renderProtections(target: HTMLElement, store: PrivacyProtections, compact: boolean, remove: (id: string) => void) {
  target.replaceChildren();
  if (!store.policy) { const note = document.createElement("p"); note.textContent = store.error || "Loading protection policy…"; target.append(note); return; }
  const rules = store.policy.rules;
  if (!rules.length) { const note = document.createElement("p"); note.textContent = "No protected paths in this account."; target.append(note); return; }
  const limit = compact ? 8 : 40;
  const list = document.createElement("ul"); list.className = "protection-list";
  const draw = (page: number) => {
    list.replaceChildren();
    for (const rule of rules.slice(page * limit, (page + 1) * limit)) {
      const row = document.createElement("li"); const path = document.createElement("span"); path.textContent = rule.path; path.title = rule.canonical_path || rule.path; row.append(path);
      if (!compact) { const button = document.createElement("button"); button.type = "button"; button.textContent = "Remove"; button.setAttribute("aria-label", `Remove protection for ${rule.path}`); button.disabled = store.saving || store.loading; button.addEventListener("click", () => remove(rule.id)); row.append(button); }
      list.append(row);
    }
  };
  target.append(list); draw(0);
  if (rules.length > limit) {
    if (compact) { const note = document.createElement("p"); note.textContent = `Showing 8 of ${rules.length}. Manage in Privacy.`; target.append(note); }
    else {
      let page = 0; const nav = document.createElement("div"); nav.className = "privacy-event-pagination";
      const previous = document.createElement("button"); previous.type = "button"; previous.textContent = "Previous";
      const next = document.createElement("button"); next.type = "button"; next.textContent = "Next";
      const label = document.createElement("span");
      const update = () => { previous.disabled = page === 0; next.disabled = (page + 1) * limit >= rules.length; label.textContent = `Page ${page + 1} of ${Math.ceil(rules.length / limit)}`; draw(page); };
      previous.addEventListener("click", () => { page--; update(); }); next.addEventListener("click", () => { page++; update(); }); nav.append(previous, label, next); target.append(nav); update();
    }
  }
}
