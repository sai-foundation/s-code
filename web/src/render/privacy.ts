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

export function renderPrivacyRequests(target: HTMLElement, requests: PrivacyRequest[]) {
  const expanded = new Set([...target.querySelectorAll<HTMLDetailsElement>("details[open][data-privacy-key]")].map((details) => details.dataset.privacyKey));
  const preserve = (details: HTMLDetailsElement, key: string) => {
    details.dataset.privacyKey = key;
    details.open = expanded.has(key);
  };
  target.replaceChildren();
  const files = privacyFileIndex(requests);
  const overview = document.createElement("section");
  overview.className = "privacy-file-index";
  const title = document.createElement("h3");
  title.textContent = `${files.length} identified source${files.length === 1 ? "" : "s"} · loaded requests`;
  overview.append(title);
  for (const file of files) {
    const row = document.createElement("details");
    preserve(row, `file:${file.source}`);
    const name = document.createElement("summary");
    name.textContent = file.source;
    const detail = document.createElement("p");
    detail.textContent = `${file.requests.size} requests · ${[...file.destinations].join(", ")} · ${[...file.statuses].join(" / ")}`;
    row.append(name, detail);
    overview.append(row);
  }
  target.append(overview);
  for (const request of requests) {
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
    const list = document.createElement("ul");
    list.className = "privacy-sources";
    for (const source of request.sources) {
      const item = document.createElement("li");
      const name = document.createElement("strong");
      name.textContent = source.source;
      const detail = document.createElement("small");
      detail.textContent = `${source.kind.replaceAll("_", " ")}${source.partial ? " · excerpt / partial context" : ""}`;
      item.append(name, detail);
      list.append(item);
    }
    card.append(list);
    if (request.unattributed.length) {
      const details = document.createElement("details");
      preserve(details, `context:${request.id}`);
      const summary = document.createElement("summary");
      summary.textContent = "Other context included";
      const text = document.createElement("p");
      text.textContent = request.unattributed.join(" · ");
      details.append(summary, text);
      card.append(details);
    }
    target.append(card);
  }
}
