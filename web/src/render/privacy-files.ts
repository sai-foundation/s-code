import { fileRows, fileStatus, type PrivacyFiles, type FileRow } from "../models/privacy-files";

export function renderPrivacyFiles(target: HTMLElement, files: PrivacyFiles, query: string, selected: string | null, select: (row: FileRow) => void) {
  const focused = (document.activeElement as HTMLElement | null)?.dataset.privacyPath;
  const { rows, total } = fileRows(files.pages, files.evidence, files.expanded, query);
  target.replaceChildren();
  if (!rows.length) {
    const empty = document.createElement("p"); empty.className = "privacy-empty";
    empty.textContent = !files.root ? "No project folder to browse. Recorded file paths, attachments and other context remain available above and in Event record." : files.loading ? "Loading folder names…" : query ? "No matching loaded files. Clear search and open another folder to include its files." : "No files to show. Refresh to check for local changes.";
    target.append(empty);
  }
  for (const row of rows) {
    const button = document.createElement("button"); button.type = "button"; button.className = "privacy-file-row"; button.dataset.privacyPath = row.path;
    button.classList.toggle("selected", row.path === selected);
    button.title = `${row.path} · ${fileStatus(row)}`;
    button.setAttribute("aria-label", `${row.path}, ${fileStatus(row)}, ${row.evidence?.requests.size ?? 0} events`);
    if (row.kind === "directory") button.setAttribute("aria-expanded", String(row.expanded));
    const name = document.createElement("span"); name.className = "privacy-file-name"; name.style.paddingLeft = `${Math.min(row.depth, 12) * 16}px`;
    const icon = document.createElement("span"); icon.className = "privacy-file-icon"; icon.setAttribute("aria-hidden", "true"); icon.textContent = row.kind === "directory" ? row.expanded ? "▾ ▰" : "▸ ▰" : "  ▤";
    const label = document.createElement("span"); label.textContent = row.path.split("/").at(-1)!; name.append(icon, label);
    const status = document.createElement("span"); status.className = "privacy-file-state";
    if (row.kind !== "directory") { const dot = document.createElement("i"); dot.className = `privacy-dot ${row.evidence?.state ?? "none"}`; status.append(dot); }
    const detail = document.createElement("span"); detail.textContent = fileStatus(row); status.append(detail);
    const count = document.createElement("span"); count.className = "privacy-file-count"; count.textContent = row.evidence?.requests.size ? String(row.evidence.requests.size) : "—";
    button.append(name, status, count); button.addEventListener("click", () => select(row));
    button.addEventListener("keydown", event => { if (row.kind === "directory" && ((event.key === "ArrowRight" && !row.expanded) || (event.key === "ArrowLeft" && row.expanded))) { event.preventDefault(); select(row); } });
    target.append(button);
  }
  if (total > rows.length) { const notice = document.createElement("p"); notice.className = "privacy-caption"; notice.textContent = `Showing ${rows.length} of ${total} loaded entries. Search or collapse folders to narrow the list.`; target.append(notice); }
  if (focused) [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.dataset.privacyPath === focused)?.focus({ preventScroll: true });
  return rows.find(row => row.path === selected);
}
