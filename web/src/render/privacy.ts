import type { PrivacyRequest } from "../../generated/protocol";

export function privacyStatus(status: string): string {
  switch (status) {
    case "accepted": return "Accepted by endpoint";
    case "rejected": return "Rejected by endpoint · data may have been received";
    case "connection_error": return "Connection error · delivery unknown";
    default: return "Request started · delivery not confirmed";
  }
}

export function privacyFileIndex(requests: PrivacyRequest[]) {
  const files = new Map<string, { source: string; requests: Set<string>; destinations: Set<string>; statuses: Set<string> }>();
  for (const request of requests) {
    for (const source of request.sources) {
      const file = files.get(source.source) ?? { source: source.source, requests: new Set<string>(), destinations: new Set<string>(), statuses: new Set<string>() };
      file.requests.add(request.id);
      file.destinations.add(request.destination);
      file.statuses.add(privacyStatus(request.status));
      files.set(source.source, file);
    }
  }
  return [...files.values()].sort((a, b) => a.source.localeCompare(b.source));
}

export const PRIVACY_EVENT_PAGE_SIZE = 200;
export function privacyEventPage(requests: PrivacyRequest[], requestedPage: number) {
  const pages = Math.max(1, Math.ceil(requests.length / PRIVACY_EVENT_PAGE_SIZE));
  const page = Math.max(0, Math.min(Math.trunc(requestedPage), pages - 1));
  const start = page * PRIVACY_EVENT_PAGE_SIZE;
  return { page, pages, start, requests: requests.slice(start, start + PRIVACY_EVENT_PAGE_SIZE) };
}

export const PRIVACY_SOURCE_PAGE_SIZE = 100;
export function privacySourcePage<T>(sources: T[], requestedPage: number) {
  const pages = Math.max(1, Math.ceil(sources.length / PRIVACY_SOURCE_PAGE_SIZE));
  const page = Math.max(0, Math.min(Math.trunc(requestedPage), pages - 1));
  const start = page * PRIVACY_SOURCE_PAGE_SIZE;
  return { page, pages, start, sources: sources.slice(start, start + PRIVACY_SOURCE_PAGE_SIZE) };
}

/** Paging replaces the visible metadata slice, keeping controls and focus stable. */
function appendMetadataPages<T>(target: HTMLElement, values: T[], key: string, initialPage: number, label: string, render: (values: T[]) => HTMLElement) {
  const container = document.createElement("div"); container.dataset.privacyPageKey = key;
  const content = document.createElement("div");
  const navigation = document.createElement("nav"); navigation.className = "privacy-event-pagination"; navigation.setAttribute("aria-label", `${label} pages`);
  const previous = document.createElement("button"); previous.type = "button"; previous.textContent = `Previous ${label}`;
  const range = document.createElement("span"); range.setAttribute("role", "status");
  const next = document.createElement("button"); next.type = "button"; next.textContent = `Next ${label}`;
  let currentPage = initialPage;
  const update = () => {
    const page = privacySourcePage(values, currentPage); currentPage = page.page;
    container.dataset.privacyPageIndex = String(page.page);
    content.replaceChildren(render(page.sources));
    navigation.hidden = page.pages <= 1;
    range.textContent = `${page.start + 1}–${page.start + page.sources.length} of ${values.length}`;
    previous.disabled = page.page === 0; next.disabled = page.page + 1 === page.pages;
  };
  previous.addEventListener("click", () => { currentPage--; update(); });
  next.addEventListener("click", () => { currentPage++; update(); });
  navigation.append(previous, range, next); container.append(content, navigation); target.append(container); update();
}

export function renderPrivacyRequests(target: HTMLElement, requests: PrivacyRequest[]) {
  const metadataPages = new Map([...target.querySelectorAll<HTMLElement>("[data-privacy-page-key]")].map(element => [element.dataset.privacyPageKey, Number(element.dataset.privacyPageIndex) || 0]));
  const expanded = new Set([...target.querySelectorAll<HTMLDetailsElement>("details[open][data-privacy-key]")].map((details) => details.dataset.privacyKey));
  const preserve = (details: HTMLDetailsElement, key: string) => {
    details.dataset.privacyKey = key;
    details.open = expanded.has(key);
  };
  target.replaceChildren();
  for (const request of requests.slice(0, PRIVACY_EVENT_PAGE_SIZE)) {
    const card = document.createElement("details");
    card.className = "privacy-request";
    preserve(card, `request:${request.id}`);
    const heading = document.createElement("summary");
    heading.textContent = `${request.purpose === "session_title" ? "Conversation title" : "Agent request"} · ${new Date(request.started_at).toLocaleTimeString()} · ${privacyStatus(request.status)}`;
    const status = document.createElement("p");
    status.className = `privacy-outcome privacy-${request.status}`;
    status.textContent = privacyStatus(request.status);
    const destination = document.createElement("p");
    destination.className = "privacy-destination";
    destination.textContent = `${request.model} → ${request.destination}`;
    const meta = document.createElement("small");
    meta.textContent = `${new Date(request.started_at).toLocaleString()} · ${request.request_bytes.toLocaleString()} request bytes`;
    card.append(heading, status, destination, meta);
    if (!request.sources.length) {
      const empty = document.createElement("p");
      empty.textContent = "No individually attributed files in this request. Other context may still contain file data.";
      card.append(empty);
    }
    appendMetadataPages(card, request.sources, `sources:${request.id}`, metadataPages.get(`sources:${request.id}`) ?? 0, "sources", sources => {
      const list = document.createElement("ul"); list.className = "privacy-sources";
      for (const source of sources) {
        const item = document.createElement("li");
        const name = document.createElement("strong"); name.textContent = source.source;
        const detail = document.createElement("small");
        detail.textContent = `${source.kind.replaceAll("_", " ")}${source.partial ? " · excerpt / partial context" : ""}`;
        item.append(name, detail); list.append(item);
      }
      return list;
    });
    if (request.unattributed.length) {
      const details = document.createElement("details");
      preserve(details, `context:${request.id}`);
      const summary = document.createElement("summary");
      summary.textContent = "Other context included";
      details.append(summary);
      appendMetadataPages(details, request.unattributed, `context:${request.id}`, metadataPages.get(`context:${request.id}`) ?? 0, "context items", items => {
        const text = document.createElement("p"); text.textContent = items.join(" · "); return text;
      });
      card.append(details);
    }
    target.append(card);
  }
}
