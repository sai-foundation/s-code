import { recordedSourceState, type RecordedSource, type SourceOverviewState } from "../models/privacy-sources";
import { privacyStatus } from "./privacy";

/** Ledger-only disclosure: no links, path opens or filesystem requests. */
export function renderRecordedSources(target: HTMLElement, records: RecordedSource[], root: string | null, query: string, state: SourceOverviewState) {
  const openItems = new Set([...target.querySelectorAll<HTMLDetailsElement>("details[data-source-key][open]")].map(item => item.dataset.sourceKey));
  const focused = (document.activeElement as HTMLElement | null)?.dataset.sourceFocus;
  target.replaceChildren();
  for (const [category, title, note] of [
    ["external", root ? "Outside project" : "Recorded file paths", "Recorded paths only. Files outside the project are never browsed or read here."],
    ["attachments", "Attachments", "These are recorded labels, not unique file identities. The same name may represent different files; attachment names do not prove their location."],
    ["other", "Other sources and context", "Remote URIs, unresolved paths and context without file attribution remain visible here."],
  ] as const) {
    const group = document.createElement("details"); group.className = "privacy-source-group";
    group.open = state.expanded.has(category) || Boolean(query.trim());
    const summary = document.createElement("summary"); summary.dataset.sourceFocus = category;
    const content = document.createElement("div");
    const render = () => {
      const page = state.page(records, category, query);
      const events = new Set(page.all.flatMap(record => [...record.requests]));
      summary.textContent = `${title} · ${page.all.length} recorded labels · ${events.size} event${events.size === 1 ? "" : "s"}${query.trim() ? ` · ${page.matching.length} matches` : ""}`;
      content.replaceChildren();
      if (!group.open) return;
      const description = document.createElement("p"); description.className = "privacy-caption"; description.textContent = note; content.append(description);
      if (!page.rows.length) { const empty = document.createElement("p"); empty.className = "privacy-caption"; empty.textContent = query.trim() ? "No matching loaded records." : "No sources in this category in loaded records."; content.append(empty); }
      for (const record of page.rows) {
        const item = document.createElement("details"); item.className = "privacy-source-item"; item.dataset.sourceKey = record.key; item.open = openItems.has(record.key);
        item.addEventListener("toggle", () => { if (item.open) openItems.add(record.key); else openItems.delete(record.key); });
        const heading = document.createElement("summary"); heading.dataset.sourceFocus = record.key;
        const dot = document.createElement(record.contentBytes === 0 ? "span" : "i"); dot.className = record.contentBytes === 0 ? "privacy-source-meta" : `privacy-dot ${record.state}`; dot.setAttribute("aria-hidden", "true"); if (record.contentBytes === 0) dot.textContent = "◇";
        const name = document.createElement("span"); name.className = "privacy-source-label"; name.textContent = record.path || record.source; name.title = record.path || record.source;
        const badge = document.createElement("span"); badge.className = "privacy-source-badge"; badge.textContent = `${record.contentBytes === 0 ? record.unattributed ? "Context" : "Name / metadata only" : record.state === "entire" ? "Full captured content" : "Partial / unknown"} · ${record.requests.size} event${record.requests.size === 1 ? "" : "s"}`;
        heading.append(dot, name, badge); item.append(heading);
        const path = document.createElement("p"); path.className = "privacy-source-path"; path.textContent = record.source; item.append(path);
        if (record.path && record.path !== record.source) { const decoded = document.createElement("p"); decoded.className = "privacy-source-path"; decoded.textContent = `Recorded path: ${record.path}`; item.append(decoded); }
        const detail = document.createElement("p"); detail.textContent = `${record.kind.replaceAll("_", " ")} · ${recordedSourceState(record)}. ${record.reason}`; item.append(detail);
        const deliveries = document.createElement("p"); deliveries.textContent = [...record.statuses].map(privacyStatus).join(" · "); item.append(deliveries);
        const destinations = document.createElement("p"); const shown = [...record.destinations].slice(0, 20);
        destinations.textContent = `Destinations: ${shown.join(" · ")}${record.destinations.size > shown.length ? ` · showing 20 of ${record.destinations.size}; see Event record for every request destination.` : ""}`; item.append(destinations);
        content.append(item);
      }
      if (page.pages > 1) {
        const nav = document.createElement("nav"); nav.className = "privacy-event-pagination"; nav.setAttribute("aria-label", `${title} pages`);
        const previous = document.createElement("button"); previous.type = "button"; previous.textContent = "Previous"; previous.disabled = page.page === 0; previous.dataset.sourceFocus = `${category}:previous`;
        const label = document.createElement("span"); label.textContent = `Page ${page.page + 1} of ${page.pages} · ${page.matching.length} labels`;
        const next = document.createElement("button"); next.type = "button"; next.textContent = "Next"; next.disabled = page.page + 1 === page.pages; next.dataset.sourceFocus = `${category}:next`;
        for (const [button, delta] of [[previous, -1], [next, 1]] as const) button.addEventListener("click", () => { state.pages.set(category, page.page + delta); render(); [...content.querySelectorAll<HTMLButtonElement>("button")].find(item => item.dataset.sourceFocus === button.dataset.sourceFocus)?.focus({ preventScroll: true }); });
        nav.append(previous, label, next); content.append(nav);
      }
    };
    group.addEventListener("toggle", () => { if (!target.contains(group)) return; if (group.open) state.expanded.add(category); else state.expanded.delete(category); render(); });
    group.append(summary, content); target.append(group); render();
  }
  if (focused) [...target.querySelectorAll<HTMLElement>("[data-source-focus]")].find(element => element.dataset.sourceFocus === focused)?.focus({ preventScroll: true });
}
