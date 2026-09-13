import { appendApprovalTarget } from "../render/approval-target";
import type { ApiRequestOptions } from "../api/client";
import type {
  AgentResultSummary,
  AgentRunSummary,
  ApprovalRequest,
  AuditEventPage,
  BackgroundTerminalOutput,
  BackgroundTerminalPreview,
  BackgroundTerminalSpec,
  BackgroundTerminalSummary,
  DurableTaskSummary,
  Scope,
  Session,
  TeamBudget,
  TeamCapacity,
  TeamDashboardSummary,
  TeamGoal,
  TeamGoalContinuation,
  TeamGoalRun,
  TeamGovernanceSummary,
  TeamOutcome,
  TeamOwnership,
  TeamTask,
  TeamTaskStatus,
} from "../models/protocol";
import {
  renderTeamAudit as renderTeamAuditInto,
  renderTeamGovernance as renderTeamGovernanceInto,
} from "./team-audit";

interface TeamElement extends HTMLElement {
  value: string;
  checked: boolean;
  disabled: boolean;
}

interface ActionField {
  name: string;
  label: string;
  type?: string;
  value?: string | number;
  placeholder?: string;
  min?: string | number;
  max?: string | number;
  maxlength?: number;
  multiline?: boolean;
  required?: boolean;
  options?: Array<[string, string]>;
}

interface ActionOptions {
  eyebrow?: string;
  title: string;
  description?: string;
  details?: string[];
  confirm?: string;
  danger?: boolean;
  fields?: ActionField[];
}

export interface TeamWorkPageContext {
  lookup(id: string): TeamElement;
  api<T = unknown>(path: string, options?: ApiRequestOptions): Promise<T>;
  state: { connected: boolean; generation: number; sessions: Session[] };
  scope(): Scope;
  catalogQuery(): URLSearchParams;
  requestAction(options: ActionOptions): Promise<Record<string, string> | null>;
  toast(message: string): void;
  addActivity(kind: string, payload: Record<string, unknown>): void;
  selectSession(session: Session): Promise<void>;
  refreshSessions(): Promise<void>;
  openDrawer(id: string): void;
  announce(message: string): void;
  showArtifacts(artifactId?: string | null): void;
  showWorkspace(): void;
  composerTextDraftKey(): string;
  resizePrompt(): void;
}

export function createTeamWorkPage(context: TeamWorkPageContext) {
  const {
    lookup: $,
    api,
    state,
    scope,
    catalogQuery,
    requestAction,
    toast,
    addActivity,
    selectSession,
    refreshSessions,
    openDrawer,
    announce,
    showArtifacts,
    showWorkspace,
    composerTextDraftKey,
    resizePrompt,
  } = context;

  function safePullRequestUrl(value: string): string | null {
    try {
      const parsed = new URL(value);
      return parsed.protocol === "https:" && !parsed.username && !parsed.password
        ? parsed.href
        : null;
    } catch {
      return null;
    }
  }

  function teamQuery(extra: Record<string, string> = {}) {
    const s = scope(); return new URLSearchParams({ organization_id: s.organization_id, actor_id: s.actor_id, ...extra });
  }

  function auditQueryParameters() {
    const query = teamQuery({ limit: "100" });
    [
      ["filter_actor_id", "audit-filter-actor"],
      ["kind", "audit-filter-action"],
      ["session_id", "audit-filter-session"],
    ].forEach(([parameter, id]) => {
      const value = $(id).value.trim();
      if (value) query.set(parameter, value);
    });
    [
      ["since", "audit-filter-since"],
      ["until", "audit-filter-until"],
    ].forEach(([parameter, id]) => {
      const value = $(id).value;
      if (value) query.set(parameter, new Date(value).toISOString());
    });
    return query;
  }

  async function exportTeamAudit() {
    if (!state.connected) throw new Error("Connect before exporting Team audit");
    const team = encodeURIComponent(scope().team_id);
    const response = await fetch(
      `/v1/teams/${team}/audit/export?${auditQueryParameters()}`,
      {
        credentials: "same-origin",
        headers: { Accept: "text/csv" },
      },
    );
    if (!response.ok) throw new Error(`Audit export failed (${response.status})`);
    const blob = await response.blob();
    const href = URL.createObjectURL(blob);
    try {
      const link = document.createElement("a");
      link.href = href;
      link.download = `s-code-team-audit-${new Date().toISOString().slice(0, 10)}.csv`;
      link.click();
    } finally {
      URL.revokeObjectURL(href);
    }
    toast("Content-free audit CSV exported");
  }

  async function refreshTeam() {
    const generation = state.generation;
    try {
      const team = encodeURIComponent(scope().team_id); const query = teamQuery();
      const [
        dashboard,
        tasks,
        goals,
        capacityResult,
        budgetsResult,
        ownershipResult,
        durableTasks,
        backgroundTerminals,
        agentRuns,
        approvals,
        outcomes,
        audit,
        governance,
      ] = await Promise.all([
        api<TeamDashboardSummary>(`/v1/teams/${team}/dashboard?${query}`),
        api<TeamTask[]>(`/v1/teams/${team}/tasks?${query}`),
        api<TeamGoal[]>(`/v1/teams/${team}/goals?${query}`),
        api<TeamCapacity>(`/v1/teams/${team}/capacity?${query}`).catch(() => null),
        api<TeamBudget[]>(`/v1/teams/${team}/budgets?${query}`).catch(() => []),
        api<TeamOwnership[]>(`/v1/teams/${team}/ownership?${query}`).catch(() => []),
        api<DurableTaskSummary[]>(`/v1/durable-task-summaries?${catalogQuery()}`).catch(() => []),
        api<BackgroundTerminalSummary[]>(`/v1/background-terminals?${catalogQuery()}`).catch(() => []),
        api<AgentRunSummary[]>(`/v1/agents?${catalogQuery()}`).catch(() => []),
        api<ApprovalRequest[]>(`/v1/teams/${team}/approvals?${query}`).catch(() => []),
        api<TeamOutcome[]>(`/v1/teams/${team}/outcomes?${query}`).catch(() => []),
        api<AuditEventPage>(`/v1/teams/${team}/audit?${auditQueryParameters()}`).catch(() => ({
          events: [],
          next_cursor: null,
        })),
        api<TeamGovernanceSummary>(`/v1/teams/${team}/governance?${query}`).catch(() => ({
          source: "unavailable",
          configuration_sequence: null,
          policy_sequence: null,
          issued_at: null,
          expires_at: null,
          audit_content_enabled: false,
          audit_retention_days: null,
          data_residency_region: null,
          audit_content_categories: [],
        })),
      ]);
      if (!state.connected || generation !== state.generation) return;
      const metrics = $("team-metrics"); metrics.replaceChildren();
      [["Outcomes", dashboard.verified_outcomes], ["Ready", dashboard.ready_tasks], ["In progress", dashboard.in_progress_tasks], ["Blocked", dashboard.blocked_tasks]].forEach(([label, value]) => { const card = document.createElement("div"); card.className = "metric"; const number = document.createElement("strong"); number.textContent = String(value); const name = document.createElement("span"); name.textContent = String(label); card.append(number, name); metrics.append(card); });
      if (capacityResult) [["WIP", `${dashboard.in_progress_tasks}/${capacityResult.wip_limit}`], ["Agents", capacityResult.agent_concurrency], ["Human h", capacityResult.human_available_hours]].forEach(([label, value]) => { const card = document.createElement("div"); card.className = "metric"; const number = document.createElement("strong"); number.textContent = String(value); const name = document.createElement("span"); name.textContent = String(label); card.append(number, name); metrics.append(card); });
      const activeBudget = budgetsResult.find((budget) => new Date(budget.period_start) <= new Date() && new Date(budget.period_end) > new Date());
      if (activeBudget) { const available = Math.max(0, activeBudget.model_limit_micros - activeBudget.model_consumed_micros - activeBudget.model_reserved_micros); const card = document.createElement("div"); card.className = "metric"; const number = document.createElement("strong"); number.textContent = `$${(available / 1e6).toFixed(2)}`; const name = document.createElement("span"); name.textContent = "Model available"; card.append(number, name); metrics.append(card); }
      const goalRuns = (await Promise.all(
        goals.map((goal) =>
          api<TeamGoalRun[]>(
            `/v1/teams/${team}/goals/${encodeURIComponent(goal.id)}/runs?${query}`,
          ).catch(() => []),
        ),
      )).flat();
      if (!state.connected || generation !== state.generation) return;
      renderGoals(goals, goalRuns);
      renderOwnership(ownershipResult);
      renderTasks(tasks);
      renderDurableTasks(durableTasks);
      renderBackgroundTerminals(backgroundTerminals);
      renderAgentRuns(agentRuns);
      renderCapacity(capacityResult, dashboard.in_progress_tasks);
      renderBudgets(budgetsResult);
      renderTeamApprovals(approvals);
      renderTeamOutcomes(outcomes);
      renderTeamAudit(audit);
      renderTeamGovernance(governance);
    } catch (error) { if (state.connected && generation === state.generation) addActivity("team.error", { error: error.message }); }
  }

  function renderCapacity(capacity: TeamCapacity | null, inProgress: number) {
    const container = $("capacity-summary");
    container.replaceChildren();
    if (!capacity) {
      const empty = document.createElement("p");
      empty.className = "empty";
      empty.textContent = "No capacity limits configured.";
      container.append(empty);
      return;
    }
    [
      ["Work in progress", `${inProgress}/${capacity.wip_limit}`],
      ["Agent concurrency", capacity.agent_concurrency],
      ["Human hours", capacity.human_available_hours],
    ].forEach(([label, value]) => {
      const card = document.createElement("div");
      card.className = "metric";
      const number = document.createElement("strong");
      number.textContent = String(value);
      const name = document.createElement("span");
      name.textContent = String(label);
      card.append(number, name);
      container.append(card);
    });
  }

  function renderBudgets(budgets: TeamBudget[]) {
    const container = $("team-budget-list");
    container.replaceChildren();
    container.className = "team-queue";
    if (!budgets.length) {
      container.textContent = "No budgets configured";
      container.classList.add("empty");
      return;
    }
    budgets.forEach((budget) => {
      const row = document.createElement("article");
      row.className = "task";
      const title = document.createElement("div");
      title.className = "task-title";
      title.textContent = `${budget.hard_limit ? "Hard" : "Advisory"} budget`;
      const meta = document.createElement("div");
      meta.className = "task-meta";
      meta.textContent = [
        `${new Date(budget.period_start).toLocaleDateString()}–${new Date(budget.period_end).toLocaleDateString()}`,
        `model $${(budget.model_consumed_micros / 1e6).toFixed(2)} / $${(budget.model_limit_micros / 1e6).toFixed(2)}`,
        `runner $${(budget.runner_consumed_micros / 1e6).toFixed(2)} / $${(budget.runner_limit_micros / 1e6).toFixed(2)}`,
      ].join(" · ");
      row.append(title, meta);
      container.append(row);
    });
  }

  function renderTeamApprovals(approvals: ApprovalRequest[]) {
    const container = $("team-approval-list");
    container.replaceChildren();
    container.className = "team-queue";
    if (!approvals.length) {
      container.textContent = "No approval decisions";
      container.classList.add("empty");
      return;
    }
    approvals.forEach((approval) => {
      const row = document.createElement("article");
      row.className = "task";
      const title = document.createElement("div");
      title.className = "task-title";
      title.textContent = approval.summary;
      const status = document.createElement("span");
      status.className = "task-status";
      status.textContent = approval.status;
      title.append(status);
      const meta = document.createElement("div");
      meta.className = "task-meta";
      meta.textContent = `${approval.risk} risk · ${approval.impact_scope} · ${new Date(approval.requested_at).toLocaleString()}`;
      const actions = document.createElement("div");
      actions.className = "task-buttons";
      const session = state.sessions.find((candidate) => candidate.id === approval.session_id);
      if (session) {
        const open = document.createElement("button");
        open.type = "button";
        open.textContent = "Open Session";
        open.addEventListener("click", () => selectSession(session));
        actions.append(open);
      }
      if (approval.status === "pending") {
        ([
          ["Approve once", true],
          ["Reject", false],
        ] as const).forEach(([label, approved]) => {
          const button = document.createElement("button");
          button.type = "button";
          button.textContent = label;
          button.classList.toggle("danger", !approved);
          button.addEventListener("click", async () => {
            await api(`/v1/approvals/${encodeURIComponent(approval.id)}`, {
              method: "POST",
              body: JSON.stringify({
                scope: scope(),
                approved,
                approval_scope: "once",
              }),
            });
            await refreshTeam();
            toast(approved ? "Approval granted once" : "Approval rejected");
          });
          actions.append(button);
        });
      }
      row.append(title, meta);
      appendApprovalTarget(row, approval);
      row.append(actions);
      container.append(row);
    });
  }

  function renderTeamOutcomes(outcomes: TeamOutcome[]) {
    const renderInto = (container: TeamElement, visible: TeamOutcome[]) => {
      container.replaceChildren();
      container.className = "team-queue";
      if (!visible.length) {
        container.textContent = "No verified outcomes";
        container.classList.add("empty");
        return;
      }
      visible.forEach((outcome) => {
        const row = document.createElement("article");
        row.className = "task";
        const title = document.createElement("div");
        title.className = "task-title";
        title.textContent = `Verified outcome · ${outcome.task_id}`;
        const meta = document.createElement("div");
        meta.className = "task-meta";
        meta.textContent = `${outcome.evidence.length} evidence item${outcome.evidence.length === 1 ? "" : "s"} · ${new Date(outcome.completed_at).toLocaleString()}`;
        const evidence = document.createElement("div");
        evidence.className = "task-meta";
        evidence.textContent = outcome.evidence
          .map((item) => `${item.kind}: ${item.result}`)
          .join(" · ");
        row.append(title, meta, evidence);
        const pullRequestUrl = outcome.pull_request_url
          ? safePullRequestUrl(outcome.pull_request_url)
          : null;
        if (pullRequestUrl) {
          const link = document.createElement("a");
          link.href = pullRequestUrl;
          link.target = "_blank";
          link.rel = "noopener noreferrer";
          link.textContent = "Open pull request";
          row.append(link);
        }
        container.append(row);
      });
    };
    renderInto($("team-outcome-list"), outcomes);
    renderInto($("outcome-summary"), outcomes.slice(0, 5));
  }

  function renderTeamAudit(page: AuditEventPage) {
    renderTeamAuditInto($("team-audit-list"), page);
  }

  function renderTeamGovernance(governance: TeamGovernanceSummary) {
    renderTeamGovernanceInto($("team-governance-summary"), governance);
  }

  function durableSession(task: DurableTaskSummary): Session | null {
    return state.sessions.find((session) => session.id === task.session_id) || null;
  }

  function renderDurableTasks(tasks: DurableTaskSummary[]) {
    const container = $("durable-task-list");
    container.replaceChildren();
    container.className = "team-queue";
    const visible = [...tasks]
      .sort((left, right) => String(right.updated_at).localeCompare(String(left.updated_at)))
      .slice(0, 12);
    if (!visible.length) {
      container.textContent = "No background tasks";
      container.classList.add("empty");
      return;
    }
    visible.forEach((task) => {
      const row = document.createElement("article");
      row.className = "task";
      const heading = document.createElement("div");
      heading.className = "task-title";
      const title = document.createElement("span");
      title.textContent = durableSession(task)?.title || task.kind;
      const status = document.createElement("span");
      status.className = "task-status";
      status.textContent = task.status;
      heading.append(title, status);
      const meta = document.createElement("div");
      meta.className = "task-meta";
      meta.textContent = `attempt ${task.attempt}/${task.max_attempts} · model $${(task.consumed_cost_micros / 1e6).toFixed(4)} · runner $${(task.consumed_runner_cost_micros / 1e6).toFixed(4)} · ${new Date(task.updated_at).toLocaleString()}`;
      const actions = document.createElement("div");
      actions.className = "task-buttons";
      const sourceSession = durableSession(task);
      if (sourceSession) {
        const open = document.createElement("button");
        open.type = "button";
        open.textContent = "Open in Chat";
        open.addEventListener("click", () => {
          selectSession(sourceSession).catch((error) => toast(error.message));
        });
        actions.append(open);
      }
      if (["queued", "leased", "running"].includes(task.status)) {
        const pause = document.createElement("button");
        pause.type = "button";
        pause.textContent = "Pause";
        pause.addEventListener("click", () => controlDurableTask(task, "pause"));
        actions.append(pause);
      }
      if (task.status === "paused") {
        const resume = document.createElement("button");
        resume.type = "button";
        resume.textContent = "Resume";
        resume.addEventListener("click", () => controlDurableTask(task, "resume"));
        actions.append(resume);
      }
      if (!["succeeded", "failed", "cancelled"].includes(task.status)) {
        const cancel = document.createElement("button");
        cancel.type = "button";
        cancel.textContent = "Cancel";
        cancel.addEventListener("click", () => controlDurableTask(task, "cancel"));
        actions.append(cancel);
      }
      row.append(heading, meta, actions);
      container.append(row);
    });
  }

  async function controlDurableTask(task: DurableTaskSummary, action: "pause" | "resume" | "cancel") {
    if (action === "cancel") {
      const values = await requestAction({
        eyebrow: "Background task",
        title: "Cancel this task?",
        description: "The current lease is revoked and the task will not retry.",
        details: [`Task: ${task.id}`, `Status: ${task.status}`, "Session history and audit evidence remain available."],
        confirm: "Cancel task",
        danger: true,
      });
      if (!values) return;
    }
    await api(`/v1/durable-tasks/${encodeURIComponent(task.id)}/${action}`, {
      method: "POST",
      body: JSON.stringify(scope()),
    });
    await refreshTeam();
    toast(`Background task ${action === "resume" ? "resumed" : `${action}d`}`);
  }

  function backgroundTerminalSession(terminal: BackgroundTerminalSummary): Session | null {
    return state.sessions.find((session) => session.id === terminal.session_id) || null;
  }

  async function readBackgroundTerminalOutput(terminal: BackgroundTerminalSummary): Promise<string> {
    const decoder = new TextDecoder();
    let text = "";
    let offset = 0;
    for (let page = 0; page < 16; page += 1) {
      const query = new URLSearchParams(catalogQuery());
      query.set("offset", String(offset));
      query.set("limit", "65536");
      const output = await api<BackgroundTerminalOutput>(
        `/v1/background-terminals/${encodeURIComponent(terminal.id)}/output?${query}`,
      );
      const binary = atob(output.content_base64);
      const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
      text += decoder.decode(bytes, { stream: !output.eof });
      if (output.eof || output.next_offset === offset) break;
      offset = output.next_offset;
    }
    text += decoder.decode();
    return text;
  }

  async function writeBackgroundTerminal(terminal: BackgroundTerminalSummary) {
    const values = await requestAction({
      eyebrow: "Background terminal",
      title: "Send terminal input",
      description: "A newline is appended so line-oriented programs receive the input immediately.",
      confirm: "Send input",
      fields: [{
        name: "content",
        label: "Input",
        multiline: true,
        required: true,
        maxlength: 65_000,
      }],
    });
    if (!values) return;
    const content = new TextEncoder().encode(`${values.content}\n`);
    let binary = "";
    content.forEach((byte) => { binary += String.fromCharCode(byte); });
    await api(`/v1/background-terminals/${encodeURIComponent(terminal.id)}/input`, {
      method: "POST",
      body: JSON.stringify({
        scope: scope(),
        content_base64: btoa(binary),
      }),
    });
    toast("Terminal input sent");
  }

  async function resizeBackgroundTerminal(terminal: BackgroundTerminalSummary) {
    const values = await requestAction({
      eyebrow: "Background terminal",
      title: "Resize terminal",
      description: "Resize the PTY without restarting the process.",
      confirm: "Resize",
      fields: [
        { name: "rows", label: "Rows", type: "number", min: 2, max: 500, value: terminal.rows, required: true },
        { name: "cols", label: "Columns", type: "number", min: 2, max: 500, value: terminal.cols, required: true },
      ],
    });
    if (!values) return;
    await api(`/v1/background-terminals/${encodeURIComponent(terminal.id)}/resize`, {
      method: "POST",
      body: JSON.stringify({
        scope: scope(),
        rows: Number(values.rows),
        cols: Number(values.cols),
      }),
    });
    await refreshTeam();
    toast("Terminal resized");
  }

  async function stopBackgroundTerminal(terminal: BackgroundTerminalSummary) {
    const values = await requestAction({
      eyebrow: "Background terminal",
      title: "Stop this terminal?",
      description: "The daemon stops the process and preserves retained output as an Artifact.",
      details: [`Terminal: ${terminal.id}`, `Program: ${terminal.program}`],
      confirm: "Stop terminal",
      danger: true,
    });
    if (!values) return;
    const latest = (await api<BackgroundTerminalSummary[]>(
      `/v1/background-terminals?${catalogQuery()}`,
    )).find((candidate) => candidate.id === terminal.id);
    if (!latest) throw new Error("The background terminal is no longer available");
    await api(`/v1/background-terminals/${encodeURIComponent(terminal.id)}/stop`, {
      method: "POST",
      body: JSON.stringify({
        scope: scope(),
        expected_revision: latest.revision,
      }),
    });
    toast("Terminal stop requested");
  }

  function renderBackgroundTerminals(terminals: BackgroundTerminalSummary[]) {
    const container = $("background-terminal-list");
    container.replaceChildren();
    container.className = "team-queue";
    const visible = [...terminals]
      .sort((left, right) => String(right.updated_at).localeCompare(String(left.updated_at)))
      .slice(0, 12);
    if (!visible.length) {
      container.textContent = "No background terminals";
      container.classList.add("empty");
      return;
    }
    visible.forEach((terminal) => {
      const row = document.createElement("article");
      row.className = "task background-terminal";
      row.dataset.terminalId = terminal.id;
      const heading = document.createElement("div");
      heading.className = "task-title";
      const title = document.createElement("span");
      title.textContent = terminal.program;
      const status = document.createElement("span");
      status.className = "task-status";
      status.textContent = terminal.status;
      heading.append(title, status);
      const meta = document.createElement("div");
      meta.className = "task-meta";
      meta.textContent = [
        `${terminal.cols}×${terminal.rows}`,
        `${terminal.argument_count} argument${terminal.argument_count === 1 ? "" : "s"}`,
        `${terminal.output_byte_length.toLocaleString()} output bytes`,
        terminal.exit_code == null ? null : `exit ${terminal.exit_code}`,
        terminal.output_truncated ? "retained tail only" : null,
      ].filter(Boolean).join(" · ");
      const actions = document.createElement("div");
      actions.className = "task-buttons";
      const sourceSession = backgroundTerminalSession(terminal);
      if (sourceSession) {
        const open = document.createElement("button");
        open.type = "button";
        open.textContent = "Open Session";
        open.addEventListener("click", () => {
          selectSession(sourceSession).catch((error) => toast(error.message));
        });
        actions.append(open);
      }
      const output = document.createElement("button");
      output.type = "button";
      output.textContent = "Output";
      const outputPanel = document.createElement("pre");
      outputPanel.className = "terminal-output";
      outputPanel.hidden = true;
      output.addEventListener("click", () => {
        if (!outputPanel.hidden) {
          outputPanel.hidden = true;
          output.textContent = "Output";
          return;
        }
        output.disabled = true;
        readBackgroundTerminalOutput(terminal)
          .then((content) => {
            outputPanel.textContent = content || "Terminal has not produced output.";
            outputPanel.hidden = false;
            output.textContent = "Hide output";
          })
          .catch((error) => toast(error.message))
          .finally(() => { output.disabled = false; });
      });
      actions.append(output);
      if (terminal.status === "running") {
        const input = document.createElement("button");
        input.type = "button";
        input.textContent = "Input";
        input.addEventListener("click", () => {
          writeBackgroundTerminal(terminal).catch((error) => toast(error.message));
        });
        const resize = document.createElement("button");
        resize.type = "button";
        resize.textContent = "Resize";
        resize.addEventListener("click", () => {
          resizeBackgroundTerminal(terminal).catch((error) => toast(error.message));
        });
        const stop = document.createElement("button");
        stop.type = "button";
        stop.textContent = "Stop";
        stop.className = "danger";
        stop.addEventListener("click", () => {
          stopBackgroundTerminal(terminal).catch((error) => toast(error.message));
        });
        actions.append(input, resize, stop);
      }
      if (terminal.artifact_id) {
        const artifact = document.createElement("button");
        artifact.type = "button";
        artifact.textContent = "Open Artifact";
        artifact.addEventListener("click", () => showArtifacts(terminal.artifact_id));
        actions.append(artifact);
      }
      row.append(heading, meta, actions, outputPanel);
      container.append(row);
    });
  }

  function agentDepth(agent: AgentRunSummary, byId: Map<string, AgentRunSummary>): number {
    let depth = 0;
    let parent = agent.parent_id;
    const seen = new Set<string>([agent.id]);
    while (parent && byId.has(parent) && !seen.has(parent) && depth < 8) {
      seen.add(parent);
      depth += 1;
      parent = byId.get(parent)?.parent_id || null;
    }
    return depth;
  }

  function renderAgentRuns(agents: AgentRunSummary[]) {
    const container = $("agent-run-list");
    container.replaceChildren();
    container.className = "team-queue";
    if (!agents.length) {
      container.textContent = "No agent runs";
      container.classList.add("empty");
      return;
    }
    const byId = new Map(agents.map((agent) => [agent.id, agent]));
    agents
      .sort((left, right) => String(left.created_at).localeCompare(String(right.created_at)))
      .forEach((agent) => {
        const row = document.createElement("article");
        row.className = "task agent-run";
        row.style.setProperty("--agent-indent", `${agentDepth(agent, byId) * 14}px`);
        const heading = document.createElement("div");
        heading.className = "task-title";
        const title = document.createElement("span");
        title.textContent = agent.parent_id ? `Child agent · ${agent.id}` : `Agent · ${agent.id}`;
        const status = document.createElement("span");
        status.className = "task-status";
        status.textContent = agent.cancel_requested ? `${agent.status} · stopping` : agent.status;
        heading.append(title, status);
        const meta = document.createElement("div");
        meta.className = "task-meta";
        meta.textContent = `${agent.model || "configured model"} · attempt ${agent.attempt} · $${((agent.consumed_cost_micros + agent.consumed_runner_cost_micros) / 1e6).toFixed(4)}`;
        const actions = document.createElement("div");
        actions.className = "task-buttons";
        const details = document.createElement("button");
        details.type = "button";
        details.textContent = "Details";
        details.addEventListener("click", () => {
          showAgentDetails(agent).catch((error) => toast(error.message));
        });
        actions.append(details);
        if (["succeeded", "failed", "cancelled"].includes(agent.status)) {
          const result = document.createElement("button");
          result.type = "button";
          result.textContent = "Result";
          result.addEventListener("click", () => {
            showAgentResult(agent).catch((error) => toast(error.message));
          });
          actions.append(result);
        }
        const sourceSession = state.sessions.find((session) => session.id === agent.session_id);
        if (sourceSession) {
          const open = document.createElement("button");
          open.type = "button";
          open.textContent = "Open in Chat";
          open.addEventListener("click", () => {
            selectSession(sourceSession).catch((error) => toast(error.message));
          });
          actions.append(open);
        }
        if (!agent.cancel_requested && !["failed", "cancelled"].includes(agent.status)) {
          const followUp = document.createElement("button");
          followUp.type = "button";
          followUp.textContent = "Follow-up";
          followUp.addEventListener("click", () => {
            followUpAgent(agent).catch((error) => toast(error.message));
          });
          actions.append(followUp);
        }
        if (!["succeeded", "failed", "cancelled"].includes(agent.status)) {
          const wait = document.createElement("button");
          wait.type = "button";
          wait.textContent = "Wait";
          wait.addEventListener("click", () => {
            waitForAgent(agent).catch((error) => toast(error.message));
          });
          actions.append(wait);
          const interrupt = document.createElement("button");
          interrupt.type = "button";
          interrupt.textContent = "Interrupt";
          interrupt.addEventListener("click", () => {
            interruptAgent(agent).catch((error) => toast(error.message));
          });
          actions.append(interrupt);
          const close = document.createElement("button");
          close.type = "button";
          close.textContent = "Close";
          close.addEventListener("click", () => {
            closeAgent(agent).catch((error) => toast(error.message));
          });
          actions.append(close);
        }
        row.append(heading, meta, actions);
        container.append(row);
      });
  }

  async function showAgentDetails(agent: AgentRunSummary) {
    await requestAction({
      eyebrow: agent.parent_id ? "Child Agent" : "Agent",
      title: agent.id,
      description: `${agent.status}${agent.cancel_requested ? " · close requested" : ""}`,
      details: [
        `Parent: ${agent.parent_id || "root"}`,
        `Session: ${agent.session_id || "none"}`,
        `Goal: ${agent.goal_id || "none"}`,
        `Team task: ${agent.team_task_id || "none"}`,
        `Model: ${agent.model || "configured model"}`,
        `Attempt: ${agent.attempt}`,
        `Cost: model $${(agent.consumed_cost_micros / 1e6).toFixed(4)} · runner $${(agent.consumed_runner_cost_micros / 1e6).toFixed(4)}`,
        `Updated: ${new Date(agent.updated_at).toLocaleString()}`,
      ],
      confirm: "Done",
    });
  }

  async function showAgentResult(agent: AgentRunSummary) {
    const result = await api<AgentResultSummary>(
      `/v1/agent-results/${encodeURIComponent(agent.id)}?${catalogQuery()}`,
    );
    await requestAction({
      eyebrow: agent.parent_id ? "Child Agent result" : "Agent result",
      title: result.agent_id,
      description: result.summary || "No final assistant message was recorded.",
      details: [
        `Status: ${result.status}`,
        `Session: ${result.session_id}`,
        `Final message: ${result.final_message_id || "none"}`,
        `Content: ${result.summary_byte_length} bytes${result.truncated ? " · showing first 16 KiB" : ""}`,
        `Artifacts: ${result.artifact_count}`,
        `Cost: model $${(result.consumed_cost_micros / 1e6).toFixed(4)} · runner $${(result.consumed_runner_cost_micros / 1e6).toFixed(4)}`,
      ],
      confirm: "Done",
    });
  }

  async function waitForAgent(agent: AgentRunSummary) {
    toast(`Waiting up to 30 seconds for ${agent.id}`);
    const result = await api<AgentRunSummary>(`/v1/agents/${encodeURIComponent(agent.id)}/wait`, {
      method: "POST",
      body: JSON.stringify({
        scope: {
          ...scope(),
          goal_id: agent.goal_id,
          task_id: agent.team_task_id,
        },
        timeout_seconds: 30,
      }),
    });
    await refreshTeam();
    const terminal = ["succeeded", "failed", "cancelled"].includes(result.status);
    toast(terminal
      ? `Agent finished · ${result.status}`
      : `Agent is still ${result.status}`);
  }

  async function closeAgent(agent: AgentRunSummary) {
    const values = await requestAction({
      eyebrow: "Agent control",
      title: "Close this Agent?",
      description: "Requests a safe shutdown. An active worker keeps its lease until it records the final cancellation state.",
      details: [`Agent: ${agent.id}`, `Status: ${agent.status}`, "Session history and audit evidence remain available."],
      confirm: "Close Agent",
      danger: true,
    });
    if (!values) return;
    const result = await api<AgentRunSummary>(`/v1/agents/${encodeURIComponent(agent.id)}/close`, {
      method: "POST",
      body: JSON.stringify({
        ...scope(),
        goal_id: agent.goal_id,
        task_id: agent.team_task_id,
      }),
    });
    await refreshTeam();
    toast(["succeeded", "failed", "cancelled"].includes(result.status)
      ? "Agent closed"
      : "Agent close requested");
  }

  async function followUpAgent(agent: AgentRunSummary) {
    const values = await requestAction({
      eyebrow: "Agent follow-up",
      title: "Send bounded follow-up work",
      description: "Creates a child Agent run in the same Session. The parent hierarchy, Team Task and Goal scope are preserved.",
      confirm: "Queue follow-up",
      fields: [
        {
          name: "content",
          label: "Message",
          multiline: true,
          maxlength: 8_000,
          placeholder: "Verify the edge case and report the exact test evidence.",
          required: true,
        },
        { name: "attempts", label: "Maximum attempts", type: "number", min: 1, value: 1, required: true },
        { name: "modelBudget", label: "Delegated model budget (USD)", type: "number", min: 0, value: 1, required: true },
      ],
    });
    if (!values) return;
    await api(`/v1/agents/${encodeURIComponent(agent.id)}/follow-up`, {
      method: "POST",
      body: JSON.stringify({
        scope: {
          ...scope(),
          goal_id: agent.goal_id,
          task_id: agent.team_task_id,
        },
        content: values.content.trim(),
        idempotency_key: `web-follow-up-${crypto.randomUUID()}`,
        max_attempts: Number(values.attempts),
        max_runtime_seconds: 3_600,
        max_cost_micros: Math.round(Number(values.modelBudget) * 1e6),
      }),
    });
    await refreshTeam();
    toast("Agent follow-up queued");
  }

  async function interruptAgent(agent: AgentRunSummary) {
    const values = await requestAction({
      eyebrow: "Agent control",
      title: "Interrupt this Agent?",
      description: "The active lease is revoked and no retry will start. Session history and audit evidence remain available.",
      details: [`Agent: ${agent.id}`, `Status: ${agent.status}`],
      confirm: "Interrupt Agent",
      danger: true,
    });
    if (!values) return;
    await api(`/v1/durable-tasks/${encodeURIComponent(agent.id)}/cancel`, {
      method: "POST",
      body: JSON.stringify({
        ...scope(),
        goal_id: agent.goal_id,
        task_id: agent.team_task_id,
      }),
    });
    await refreshTeam();
    toast("Agent interrupted");
  }

  async function createBackgroundTask() {
    if (!state.sessions.length) {
      toast("Create a Session before starting a background Agent task.");
      showWorkspace();
      return;
    }
    const values = await requestAction({
      eyebrow: "Background Agent",
      title: "Run a task in the background",
      description: "The task continues in the daemon after this page closes. Its log stays in the source Session, not the Team overview.",
      confirm: "Queue background task",
      fields: [
        {
          name: "session",
          label: "Source Session",
          options: state.sessions.slice(0, 50).map((session) => [session.id, session.title]),
        },
        {
          name: "content",
          label: "Task",
          multiline: true,
          maxlength: 8_000,
          placeholder: "Run the full test suite and fix the highest-impact failure.",
          required: true,
        },
        { name: "attempts", label: "Maximum attempts", type: "number", min: 1, value: 3, required: true },
        { name: "modelBudget", label: "Model budget (USD)", type: "number", min: 0, value: 5, required: true },
      ],
    });
    if (!values) return;
    const selected = state.sessions.find((session) => session.id === values.session);
    if (!selected) {
      toast("The selected Session is no longer available.");
      return;
    }
    const taskScope = {
      ...scope(),
      goal_id: selected.scope?.goal_id || null,
      task_id: selected.scope?.task_id || null,
    };
    await api("/v1/durable-tasks", {
      method: "POST",
      body: JSON.stringify({
        scope: taskScope,
        kind: "agent.turn",
        payload: { session_id: selected.id, content: values.content.trim() },
        idempotency_key: `web-${crypto.randomUUID()}`,
        max_attempts: Number(values.attempts),
        max_runtime_seconds: 3_600,
        max_cost_micros: Math.round(Number(values.modelBudget) * 1e6),
        max_runner_cost_micros: 0,
      }),
    });
    await refreshTeam();
    toast("Background Agent task queued");
  }

  async function createBackgroundTerminal() {
    if (!state.sessions.length) {
      toast("Create a Session before starting a background terminal.");
      showWorkspace();
      return;
    }
    const values = await requestAction({
      eyebrow: "Background terminal",
      title: "Start a terminal process",
      description: "The process runs in the local daemon and continues after this page closes. Output is retained separately and becomes an Artifact when the process exits.",
      confirm: "Preview permissions",
      fields: [
        {
          name: "session",
          label: "Source Session",
          options: state.sessions.slice(0, 50).map((session) => [session.id, session.title]),
        },
        {
          name: "program",
          label: "Absolute program path",
          value: "/bin/sh",
          placeholder: "/bin/sh",
          maxlength: 4_096,
          required: true,
        },
        {
          name: "arguments",
          label: "Arguments (one per line)",
          multiline: true,
          maxlength: 32_000,
          placeholder: "-c\nprintf 'hello from S-Code\\n'",
        },
        {
          name: "environment",
          label: "Environment handles (NAME=HANDLE, one per line)",
          multiline: true,
          maxlength: 8_000,
          placeholder: "API_TOKEN=OPENROUTER_API_KEY",
        },
        { name: "rows", label: "Rows", type: "number", min: 2, max: 500, value: 24, required: true },
        { name: "cols", label: "Columns", type: "number", min: 2, max: 500, value: 80, required: true },
        { name: "runtime", label: "Maximum runtime (seconds)", type: "number", min: 1, max: 86_400, value: 3_600, required: true },
      ],
    });
    if (!values) return;
    const session = state.sessions.find((candidate) => candidate.id === values.session);
    if (!session) throw new Error("The selected Session is no longer available");
    const environmentHandles: Record<string, string> = {};
    for (const entry of values.environment.split(/\r?\n/).map((line) => line.trim()).filter(Boolean)) {
      const separator = entry.indexOf("=");
      if (separator <= 0 || separator === entry.length - 1) {
        throw new Error("Environment handles must use NAME=HANDLE");
      }
      const name = entry.slice(0, separator);
      const handle = entry.slice(separator + 1);
      if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name) || !/^[A-Za-z_][A-Za-z0-9_]*$/.test(handle)) {
        throw new Error("Environment names and handles must use shell variable names");
      }
      if (environmentHandles[name]) throw new Error(`Duplicate environment name ${name}`);
      environmentHandles[name] = handle;
    }
    const terminal: BackgroundTerminalSpec = {
      session_id: session.id,
      program: values.program.trim(),
      args: values.arguments.split(/\r?\n/).filter((argument) => argument.length > 0),
      environment_handles: environmentHandles,
      working_directory_uri: session.workspace_uri,
      rows: Number(values.rows),
      cols: Number(values.cols),
      max_runtime_seconds: Number(values.runtime),
    };
    const preview = await api<BackgroundTerminalPreview>("/v1/background-terminals/preview", {
      method: "POST",
      body: JSON.stringify({ scope: scope(), terminal }),
    });
    const confirmed = await requestAction({
      eyebrow: "Permission preview",
      title: "Start this background terminal?",
      description: "Review the exact effective permissions. Credentials are referenced by handle and never shown or persisted in this preview.",
      details: [
        ...preview.permissions.map(
          (permission) => `${permission.kind}: ${permission.value} — ${permission.reason}`,
        ),
        `Confirmation digest: ${preview.permissions_sha256}`,
      ],
      confirm: "Confirm and start",
    });
    if (!confirmed) return;
    const started = await api<BackgroundTerminalSummary>("/v1/background-terminals", {
      method: "POST",
      body: JSON.stringify({
        scope: scope(),
        terminal,
        confirmation: {
          confirmed: true,
          permissions_sha256: preview.permissions_sha256,
        },
      }),
    });
    await refreshTeam();
    toast(`Background terminal started · ${started.id}`);
  }

  function renderGoals(goals: TeamGoal[], runs: TeamGoalRun[] = []) {
    const container = $("team-goals");
    container.replaceChildren();
    container.className = "team-queue";
    const active = goals.filter((goal) => !["achieved", "cancelled"].includes(goal.status));
    if (!active.length) {
      container.textContent = "No active goals";
      container.classList.add("empty");
      return;
    }
    active.forEach((goal) => {
      const run = runs
        .filter((candidate) => candidate.goal_id === goal.id)
        .sort((left, right) => String(right.updated_at).localeCompare(String(left.updated_at)))[0];
      const row = document.createElement("div");
      row.className = "task";
      const title = document.createElement("div");
      title.className = "task-title";
      title.textContent = goal.title;
      const meta = document.createElement("div");
      meta.className = "task-meta";
      meta.textContent = `${goal.status}${run ? ` · run ${run.status}` : ""} · ${goal.outcome_definition}`;
      const actions = document.createElement("div");
      actions.className = "task-buttons";
      if (!run || run.status === "cancelled") {
        const continueButton = document.createElement("button");
        continueButton.type = "button";
        continueButton.textContent = "Continue automatically";
        continueButton.addEventListener("click", () => {
          continueGoal(goal).catch((error) => toast(error.message));
        });
        actions.append(continueButton);
      } else {
        if (run.status === "active") {
          const pause = document.createElement("button");
          pause.type = "button";
          pause.textContent = "Pause run";
          pause.addEventListener("click", () => {
            controlGoalRun(goal, run, "paused").catch((error) => toast(error.message));
          });
          actions.append(pause);
        }
        if (run.status === "paused" || run.status === "awaiting_verification") {
          const resume = document.createElement("button");
          resume.type = "button";
          resume.textContent = "Resume run";
          resume.addEventListener("click", () => {
            controlGoalRun(goal, run, "active").catch((error) => toast(error.message));
          });
          actions.append(resume);
        }
        const cancel = document.createElement("button");
        cancel.type = "button";
        cancel.textContent = "Cancel run";
        cancel.addEventListener("click", () => {
          controlGoalRun(goal, run, "cancelled").catch((error) => toast(error.message));
        });
        actions.append(cancel);
      }
      const transitions: Array<[string, TeamGoal["status"]]> = [
        ["Achieve", "achieved"],
        ["Cancel", "cancelled"],
      ];
      transitions.forEach(([label, status]) => {
        const button = document.createElement("button");
        button.textContent = label;
        button.addEventListener("click", () => updateGoal(goal, status));
        actions.append(button);
      });
      row.append(title, meta, actions);
      container.append(row);
    });
  }

  async function controlGoalRun(
    goal: TeamGoal,
    run: TeamGoalRun,
    status: "active" | "paused" | "cancelled",
  ) {
    if (status === "cancelled") {
      const values = await requestAction({
        eyebrow: "Goal automation",
        title: "Cancel this Goal run?",
        description: "No further Tasks will start. Active Agent work receives a safe cancellation request; completed evidence remains available.",
        details: [`Goal: ${goal.title}`, `Run: ${run.id}`],
        confirm: "Cancel run",
        danger: true,
      });
      if (!values) return;
    }
    await api<TeamGoalRun>(
      `/v1/teams/${encodeURIComponent(scope().team_id)}/goals/${encodeURIComponent(goal.id)}/runs/${encodeURIComponent(run.id)}`,
      {
        method: "PATCH",
        body: JSON.stringify({
          scope: scope(),
          status,
          expected_revision: run.revision,
        }),
      },
    );
    await refreshTeam();
    toast(status === "active"
      ? "Goal run resumed"
      : status === "paused"
        ? "Goal run will pause after the current Task"
        : "Goal run cancelled");
  }

  async function continueGoal(goal: TeamGoal) {
    const workspace = $("workspace").value.trim();
    const model = $("model").value.trim();
    if (!workspace || !model) {
      toast("Set a workspace and model before continuing a Goal.");
      openDrawer("settings-drawer");
      return;
    }
    const values = await requestAction({
      eyebrow: "Goal automation",
      title: "Continue ready Tasks automatically",
      description: "Runs one ready unblocked Task at a time. Each result stops in Review; a verified Team Outcome is still required before the Goal can be achieved.",
      details: [
        `Goal: ${goal.title}`,
        "WIP and Agent concurrency remain enforced.",
        "Each Task reserves its maximum model budget before it starts.",
      ],
      confirm: "Start Goal run",
      fields: [
        { name: "attempts", label: "Maximum attempts per Task", type: "number", min: 1, value: 2, required: true },
        { name: "modelBudget", label: "Model budget per Task (USD)", type: "number", min: 0, value: 5, required: true },
      ],
    });
    if (!values) return;
    const continuation = await api<TeamGoalContinuation>(
      `/v1/teams/${encodeURIComponent(scope().team_id)}/goals/${encodeURIComponent(goal.id)}/continue`,
      {
        method: "POST",
        body: JSON.stringify({
          scope: scope(),
          workspace_uri: workspace,
          model,
          idempotency_key: `web-goal-${crypto.randomUUID()}`,
          max_attempts: Number(values.attempts),
          max_runtime_seconds: 3_600,
          max_cost_micros: Math.round(Number(values.modelBudget) * 1e6),
        }),
      },
    );
    await refreshSessions();
    await refreshTeam();
    toast(`Goal run started · ${continuation.task.title}`);
  }

  async function updateGoal(goal: TeamGoal, status: TeamGoal["status"]) {
    await api(`/v1/teams/${encodeURIComponent(scope().team_id)}/goals/${encodeURIComponent(goal.id)}`, { method: "PATCH", body: JSON.stringify({ scope: scope(), title: goal.title, outcome_definition: goal.outcome_definition, status, target_date: goal.target_date }) }); await refreshTeam();
  }

  function renderOwnership(items: TeamOwnership[]) {
    const resources = $("team-resources"); resources.replaceChildren(); resources.className = "team-queue";
    if (!items.length) { resources.textContent = "No owned resources"; resources.classList.add("empty"); return; }
    items.forEach((item) => { const row = document.createElement("div"); row.className = "task"; const title = document.createElement("div"); title.className = "task-title"; title.textContent = `${item.resource_type}: ${item.resource_uri}`; const meta = document.createElement("div"); meta.className = "task-meta"; meta.textContent = [item.service_tier, item.on_call].filter(Boolean).join(" · ") || "Team owned"; const actions = document.createElement("div"); actions.className = "task-buttons"; const transfer = document.createElement("button"); transfer.textContent = "Transfer"; transfer.addEventListener("click", () => updateOwnership(item, false)); const archive = document.createElement("button"); archive.textContent = "Archive"; archive.addEventListener("click", () => updateOwnership(item, true)); actions.append(transfer, archive); row.append(title, meta, actions); resources.append(row); });
  }

  async function updateOwnership(item: TeamOwnership, archived: boolean) {
    const values = await requestAction(archived ? {
      eyebrow: "Ownership", title: "Archive ownership", description: `Remove ${item.resource_uri} from active Team ownership?`, confirm: "Archive", danger: true,
    } : {
      eyebrow: "Ownership", title: "Transfer ownership", description: `Move ${item.resource_uri} to another Team.`,
      confirm: "Transfer", fields: [{ name: "team", label: "Destination Team ID", value: scope().team_id, required: true }],
    });
    if (!values) return;
    const newTeam = archived ? null : values.team;
    await api(`/v1/teams/${encodeURIComponent(scope().team_id)}/ownership/${encodeURIComponent(item.id)}`, { method: "PATCH", body: JSON.stringify({ scope: scope(), service_tier: item.service_tier, on_call: item.on_call, new_team_id: newTeam, archived }) }); await refreshTeam();
    toast(archived ? "Ownership archived" : "Ownership transferred");
  }

  async function setCapacity() {
    const values = await requestAction({
      eyebrow: "Team limits", title: "Set capacity", description: "Bound concurrent work so the queue stays legible and predictable.", confirm: "Save capacity",
      fields: [
        { name: "human", label: "Human available hours", type: "number", min: 0, value: 40, required: true },
        { name: "agents", label: "Maximum concurrent agents", type: "number", min: 0, value: 4, required: true },
        { name: "wip", label: "Team work-in-progress limit", type: "number", min: 1, value: 6, required: true },
      ],
    });
    if (!values) return;
    const human = Number(values.human); const agents = Number(values.agents); const wip = Number(values.wip); if (![human, agents, wip].every(Number.isFinite)) return;
    await api(`/v1/teams/${encodeURIComponent(scope().team_id)}/capacity`, { method: "PUT", body: JSON.stringify({ scope: scope(), human_available_hours: human, agent_concurrency: agents, wip_limit: wip }) }); await refreshTeam();
    toast("Team capacity saved");
  }

  async function createOwnership() {
    const values = await requestAction({
      eyebrow: "Ownership", title: "Add owned resource", description: "Make responsibility visible in the same place the Team works.", confirm: "Add ownership",
      fields: [
        { name: "resourceType", label: "Resource type", options: [["repository", "Repository"], ["service", "Service"], ["environment", "Environment"]] },
        { name: "resourceUri", label: "Absolute resource URI", placeholder: "https://github.example/org/repo", required: true },
        { name: "serviceTier", label: "Service tier (optional)", placeholder: "tier-1" },
        { name: "onCall", label: "On-call URI (optional)", placeholder: "https://oncall.example/team" },
      ],
    });
    if (!values) return;
    const { resourceType, resourceUri } = values; const serviceTier = values.serviceTier || null; const onCall = values.onCall || null;
    await api(`/v1/teams/${encodeURIComponent(scope().team_id)}/ownership`, { method: "POST", body: JSON.stringify({ scope: scope(), resource_type: resourceType, resource_uri: resourceUri, service_tier: serviceTier, on_call: onCall }) }); await refreshTeam();
    toast("Ownership added");
  }

  async function createBudget() {
    const values = await requestAction({
      eyebrow: "Spend guardrail", title: "Set 30-day budget", description: "A hard limit protects the Team without adding choices to every task.", confirm: "Set budget",
      fields: [
        { name: "model", label: "Model budget (USD)", type: "number", min: 0, value: 1000, required: true },
        { name: "runner", label: "Runner budget (USD)", type: "number", min: 0, value: 500, required: true },
      ],
    });
    if (!values) return;
    const modelDollars = Number(values.model); const runnerDollars = Number(values.runner); if (![modelDollars, runnerDollars].every((value) => Number.isFinite(value) && value >= 0)) return;
    const start = new Date(); const end = new Date(start.getTime() + 30 * 86400 * 1000);
    await api(`/v1/teams/${encodeURIComponent(scope().team_id)}/budgets`, { method: "POST", body: JSON.stringify({ scope: scope(), period_start: start.toISOString(), period_end: end.toISOString(), model_limit_micros: Math.round(modelDollars * 1e6), runner_limit_micros: Math.round(runnerDollars * 1e6), hard_limit: true }) }); await refreshTeam();
    toast("Budget guardrail set");
  }

  function renderTasks(tasks: TeamTask[]) {
    const queue = $("team-queue"); queue.replaceChildren(); queue.className = "team-queue";
    const activeTasks = tasks.filter((task) => !["verified", "cancelled"].includes(task.status));
    if (!activeTasks.length) { queue.textContent = "No queued work"; queue.classList.add("empty"); return; }
    activeTasks.forEach((task) => {
      const row = document.createElement("div"); row.className = "task";
      const title = document.createElement("div"); title.className = "task-title"; title.textContent = task.title;
      const meta = document.createElement("div"); meta.className = "task-meta"; meta.textContent = `${task.status} · P${task.priority} · ${task.source}`;
      const actions = document.createElement("div"); actions.className = "task-buttons";
      const open = document.createElement("button");
      open.type = "button";
      open.textContent = "Open in Chat";
      open.addEventListener("click", () => openTeamTaskInChat(task));
      actions.append(open);
      const states: TeamTaskStatus[] = task.status === "ready" ? ["in_progress", "blocked"] : task.status === "in_progress" ? ["review", "blocked"] : task.status === "blocked" ? ["ready", "in_progress"] : task.status === "review" ? ["in_progress"] : [];
      states.forEach((status) => { const button = document.createElement("button"); button.textContent = status.replace("_", " "); button.addEventListener("click", () => updateTask(task, status)); actions.append(button); });
      if (task.status === "review") { const verify = document.createElement("button"); verify.textContent = "verify outcome"; verify.addEventListener("click", () => verifyOutcome(task)); actions.append(verify); }
      row.append(title, meta, actions); queue.append(row);
    });
  }

  async function openTeamTaskInChat(task: TeamTask) {
    const existing = state.sessions.find((session) => session.scope?.task_id === task.id);
    if (existing) {
      await selectSession(existing);
    } else {
      const workspace = $("workspace").value.trim();
      const model = $("model").value.trim();
      if (!workspace || !model) {
        toast("Set a workspace and model before opening Team work in Chat.");
        openDrawer("settings-drawer");
        return;
      }
      const session = await api<Session>("/v1/sessions", {
        method: "POST",
        body: JSON.stringify({
          scope: {
            ...scope(),
            goal_id: task.goal_id,
            task_id: task.id,
          },
          workspace_uri: workspace,
          title: task.title,
          model,
        }),
      });
      await refreshSessions();
      await selectSession(session);
    }
    const acceptance = task.acceptance_criteria.length
      ? `\nAcceptance criteria:\n${task.acceptance_criteria.map((item) => `- ${item}`).join("\n")}`
      : "";
    const evidence = task.required_evidence.length
      ? `\nRequired evidence:\n${task.required_evidence.map((item) => `- ${item}`).join("\n")}`
      : "";
    $("prompt").value = `Work on Team task: ${task.title}${acceptance}${evidence}`;
    sessionStorage.setItem(composerTextDraftKey(), $("prompt").value);
    resizePrompt();
    $("prompt").focus();
    announce(`Team task ${task.title} opened in Chat`);
  }

  async function createGoal() {
    const values = await requestAction({
      eyebrow: "Team outcome", title: "Create a goal", description: "Define the observable result before creating work.", confirm: "Create goal",
      fields: [
        { name: "title", label: "Goal title", placeholder: "Ship passwordless sign-in", required: true },
        { name: "outcome", label: "Observable outcome", placeholder: "All supported clients sign in without passwords and the rollout has no P0 regressions.", multiline: true, required: true },
      ],
    });
    if (!values) return;
    const { title, outcome } = values;
    await api(`/v1/teams/${encodeURIComponent(scope().team_id)}/goals`, { method: "POST", body: JSON.stringify({ scope: scope(), title, outcome_definition: outcome, target_date: null }) }); await refreshTeam();
    toast("Goal created");
  }

  async function createTask() {
    const values = await requestAction({
      eyebrow: "Team work", title: "Create a task", description: "Capture the work source and the evidence required to call it done.", confirm: "Create task",
      fields: [
        { name: "title", label: "Task title", placeholder: "Fix token refresh race", required: true },
        { name: "source", label: "Source", options: [["manual", "Manual"], ["issue", "Issue"], ["review", "Review"], ["ci", "CI"], ["security", "Security"], ["incident", "Incident"]] },
        { name: "evidence", label: "Required evidence (comma separated)", value: "tests,review", required: true },
      ],
    });
    if (!values) return;
    const { title, source } = values;
    const goals = await api<TeamGoal[]>(`/v1/teams/${encodeURIComponent(scope().team_id)}/goals?${teamQuery()}`); const goalId = goals[0]?.id || null;
    const evidence = values.evidence.split(",").map((item) => item.trim()).filter(Boolean);
    await api(`/v1/teams/${encodeURIComponent(scope().team_id)}/tasks`, { method: "POST", body: JSON.stringify({ scope: scope(), goal_id: goalId, source, title, priority: 50, assignee_type: null, assignee_id: null, acceptance_criteria: [], required_evidence: evidence }) }); await refreshTeam();
    toast("Task created");
  }

  async function updateTask(task: TeamTask, status: TeamTaskStatus) {
    let blockers: string[] = [];
    if (status === "blocked") {
      const values = await requestAction({
        eyebrow: "Team work", title: "Mark task blocked", description: "Name the constraint so another person or agent can act on it.", confirm: "Mark blocked",
        fields: [{ name: "blocker", label: "Blocking condition", multiline: true, required: true }],
      });
      if (!values) return;
      blockers = [values.blocker];
    }
    await api(`/v1/teams/${encodeURIComponent(scope().team_id)}/tasks/${encodeURIComponent(task.id)}`, { method: "PATCH", body: JSON.stringify({ scope: scope(), status, assignee_type: task.assignee_type, assignee_id: task.assignee_id, blockers }) }); await refreshTeam();
    toast(`Task moved to ${status.replace("_", " ")}`);
  }

  async function verifyOutcome(task: TeamTask) {
    if (!task.goal_id) { addActivity("outcome.error", { error: "Task has no goal" }); return; }
    const now = new Date().toISOString(); const evidence = task.required_evidence.map((kind) => ({ kind, uri: `manual-review://${encodeURIComponent(task.id)}/${encodeURIComponent(kind)}`, result: "passed", collected_at: now }));
    const values = await requestAction({
      eyebrow: "Verification", title: "Verify outcome", description: `Record the evidence for “${task.title}”.`, confirm: "Verify outcome",
      fields: [{ name: "pr", label: "Pull request URL (optional)", type: "url", placeholder: "https://github.example/org/repo/pull/123" }],
    });
    if (!values) return;
    const pr = values.pr || null;
    await api(`/v1/teams/${encodeURIComponent(scope().team_id)}/outcomes`, { method: "POST", body: JSON.stringify({ scope: scope(), goal_id: task.goal_id, task_id: task.id, evidence, pull_request_url: pr }) }); await refreshTeam();
    toast("Outcome verified");
  }


  return {
    createBackgroundTask,
    createBackgroundTerminal,
    createBudget,
    createGoal,
    createOwnership,
    createTask,
    exportTeamAudit,
    refreshTeam,
    renderAgentRuns,
    renderBackgroundTerminals,
    renderBudgets,
    renderCapacity,
    renderDurableTasks,
    renderGoals,
    renderOwnership,
    renderTasks,
    renderTeamApprovals,
    renderTeamAudit,
    renderTeamGovernance,
    renderTeamOutcomes,
    setCapacity,
  };
}
