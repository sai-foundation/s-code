import type {
  AuditEventPage,
  TeamGovernanceSummary,
} from "../models/protocol";

export function renderTeamAudit(
  container: HTMLElement,
  page: AuditEventPage,
): void {
  container.replaceChildren();
  container.className = "team-queue";
  if (!page.events.length) {
    container.textContent = "No audit activity";
    container.classList.add("empty");
    return;
  }
  [...page.events].reverse().forEach((event) => {
    const row = document.createElement("article");
    row.className = "task";
    const title = document.createElement("div");
    title.className = "task-title";
    title.textContent = event.kind.replaceAll(".", " · ");
    const sequence = document.createElement("span");
    sequence.className = "task-status";
    sequence.textContent = `#${event.sequence}`;
    title.append(sequence);
    const meta = document.createElement("div");
    meta.className = "task-meta";
    meta.textContent = [
      new Date(event.timestamp).toLocaleString(),
      `actor ${event.actor_id}`,
      event.goal_id ? `goal ${event.goal_id}` : null,
      event.task_id ? `task ${event.task_id}` : null,
      event.session_id ? `session ${event.session_id}` : null,
      event.turn_id ? `turn ${event.turn_id}` : null,
      event.item_id ? `item ${event.item_id}` : null,
      event.request_id ? `request ${event.request_id}` : null,
    ].filter(Boolean).join(" · ");
    row.append(title, meta);
    container.append(row);
  });
}

export function renderTeamGovernance(
  container: HTMLElement,
  governance: TeamGovernanceSummary,
): void {
  container.replaceChildren();
  container.className = "governance-summary";
  const rows: Array<[string, string]> = [
    ["Source", governance.source.replaceAll("_", " ")],
    [
      "Audit content",
      governance.audit_content_enabled
        ? "Encrypted under the signed Team policy"
        : "Metadata only",
    ],
    [
      "Retention",
      governance.audit_retention_days == null
        ? "No content-retention policy"
        : `${governance.audit_retention_days} days · Legal Hold takes precedence`,
    ],
    [
      "Data residency",
      governance.data_residency_region || "Local Team storage",
    ],
  ];
  if (governance.configuration_sequence != null) {
    rows.push([
      "Signed revision",
      `configuration ${governance.configuration_sequence} · policy ${governance.policy_sequence}`,
    ]);
  }
  if (governance.expires_at) {
    rows.push(["Valid until", new Date(governance.expires_at).toLocaleString()]);
  }
  rows.forEach(([label, value]) => {
    const row = document.createElement("article");
    const heading = document.createElement("strong");
    heading.textContent = label;
    const detail = document.createElement("span");
    detail.textContent = value;
    row.append(heading, detail);
    container.append(row);
  });
}
