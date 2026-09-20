import { providerSetup, type Catalog as ProviderSetupCatalog } from "./onboarding/setup";
import { prepareAttachment } from "./models/attachments";
import { accountDraftContext, accountKey, accountPermissionKey, accountPresenceClientId, guardAccountResponse, ownsSession } from "./models/account-state";
import { applyWorkTransition, hasWorkspace, newSessionWorkspace, sessionMode, workTransitionNotice } from "./models/session-mode";
import type { ConversationMode } from "./models/session-mode";
import { canBindToolProposal } from "./render/tool-step";
import { appendApprovalTarget } from "./render/approval-target";
import { requestJson } from "./api/client";
import type { ApiRequestOptions } from "./api/client";
import { parseClientEvent, parseTranscriptSnapshot } from "./models/runtime";
import type {
  AgentResultSummary,
  AgentRunSummary,
  ApprovalRequest,
  Artifact,
  ArtifactIndexEntry,
  ArtifactPage,
  AttachmentMetadata,
  AuditEventPage,
  ClientPresence,
  BackgroundTerminalOutput,
  BackgroundTerminalPreview,
  BackgroundTerminalSpec,
  BackgroundTerminalSummary,
  CompactSessionResult,
  ContextSummary,
  DurableTaskSummary,
  ExtensionDescriptor,
  ExtensionInstallPreview,
  HookSpec,
  MemoryItem,
  MemoryScope,
  McpResourcePage,
  McpResourceRead,
  McpResourceTemplatePage,
  McpHttpServerSpec,
  McpOAuthDiscovery,
  McpOAuthLaunch,
  McpOAuthStatus,
  McpServerSpec,
  MarketplaceInstallation,
  MarketplaceSource,
  Message,
  ModelCatalogEntry,
  PermissionMode,
  PermissionProfile,
  PluginDetail,
  PluginSummary,
  QuestionRequest,
  RetryTurnResult,
  ReviewReport,
  Scope,
  Session,
  SessionUsage,
  SessionBranchTree,
  SessionExport,
  SessionGoal,
  SessionGoalStatus,
  SessionImpactPreview,
  SessionPreferences,
  SkillInstallation,
  SkillSpec,
  SideConversation,
  SideConversationStart,
  TeamBudget,
  TeamCapacity,
  TeamDashboardSummary,
  TeamGovernanceSummary,
  TeamGoal,
  TeamGoalContinuation,
  TeamGoalRun,
  TeamOutcome,
  TeamOwnership,
  TeamTask,
  TeamTaskStatus,
  TranscriptSnapshot,
  Turn,
  TurnUndoImpactPreview,
  TurnInput,
} from "./models/protocol";
import {
  emptyTranscriptProjection,
  selectTranscriptSession,
  applyTranscriptSnapshot,
  isTranscriptSnapshotStale,
  reduceClientEvent
} from "./state/transcript";
import {
  artifactRoute,
  extensionsRoute,
  parseRoute,
  projectRoute,
  sessionRoute,
  teamRoute,
} from "./router";
import type { TeamSection } from "./router";
import {
  isMessageItem,
  isToolItem,
  transcriptItemIdentity
} from "./render/transcript";
import { HighlightClient } from "./render/highlight";
import { createMarkdownRenderer } from "./render/markdown";
import { createExtensionsPage } from "./pages/extensions";
import { createTeamWorkPage } from "./pages/team-work";
import { createWorkspaceLibrary } from "./pages/workspace-library";

declare global {
  interface HTMLElement {
    _countdownTimer?: number;
  }
}

type JsonObject = Record<string, any>;
type NotificationMode = "off" | "background" | "always";

function isJsonObject(value: unknown): value is JsonObject {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
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

interface ViewOptions {
  replace?: boolean;
  updateRoute?: boolean;
}

interface CommandDefinition {
  label: string;
  detail: string;
  shortcut: string;
  enabled?: () => boolean;
  run: () => void | Promise<void>;
}

interface WorkspacePathMatch {
  path: string;
}

interface ContextSourceSummary {
  kind: string;
  source_uri: string;
  estimated_tokens: number;
  trust_level: string;
}

interface ContextSummaryResponse {
  items: ContextSourceSummary[];
}

interface DaemonSettings {
  organization_id: string;
  team_id: string;
  actor_id: string;
  workspace_uri: string;
  default_model: string;
  default_title: string;
  telemetry_enabled: boolean;
  max_context_tokens: number;
}

interface ToolSubmissionResponse extends JsonObject {
  outcome?: string;
  tool_call?: {
    request?: {
      turn_id?: string | null;
    };
  };
}

type QuestionRequestView = Pick<
  QuestionRequest,
  "id" | "turn_id" | "item_id" | "questions" | "allow_other" | "expires_at" | "status" | "answers"
>;

interface AppElement extends HTMLElement {
  value: any;
  checked: boolean;
  disabled: boolean;
  open: boolean;
  content: string;
  files: FileList | null;
  name: string;
  required: boolean;
  type: string;
  placeholder: string;
  min: string;
  max: string;
  maxLength: number;
  selectedIndex: number;
  close(returnValue?: string): void;
  showModal(): void;
  reportValidity(): boolean;
  requestSubmit(submitter?: HTMLElement | null): void;
}

function $(id: string): AppElement {
  const element = document.getElementById(id);
  if (!element) throw new Error(`missing required UI element #${id}`);
  return element as AppElement;
}

interface WebState {
  session: Session | null;
  sessions: Session[];
  turn: string | null;
  turnRunning: boolean;
  pendingInputs: TurnInput[];
  draftFiles: File[];
  draftFilesByContext: Map<string, File[]>;
  after: number;
  abort: AbortController | null;
  reconnectTimer: ReturnType<typeof window.setTimeout> | null;
  approvals: Set<string>;
  questions: Set<string>;
  toolSteps: Map<string, HTMLElement>;
  itemsById: Map<string, HTMLElement>;
  capabilities: Set<string>;
  connected: boolean;
  connecting: boolean;
  authenticatedScope: Scope | null;
  generation: number;
  permissionMode: PermissionMode;
  assistantAlias: string;
  usage: SessionUsage;
  usageTurns: Set<string>;
  goal: SessionGoal | null;
  sideConversation: SideConversation | null;
}

const state: WebState = { session: null, sessions: [], turn: null, turnRunning: false, pendingInputs: [], draftFiles: [], draftFilesByContext: new Map(), after: 0, abort: null, reconnectTimer: null, approvals: new Set(), questions: new Set(), toolSteps: new Map(), itemsById: new Map(), capabilities: new Set(), connected: false, connecting: false, authenticatedScope: null, generation: 0, permissionMode: "manual", assistantAlias: "S-Code", usage: { input_tokens: 0, output_tokens: 0, total_tokens: 0, model_calls: 0, tool_calls: 0, turns: 0 }, usageTurns: new Set(), goal: null, sideConversation: null };
let transcriptProjection = emptyTranscriptProjection();
let loadedTranscriptSnapshot: TranscriptSnapshot | null = null;
const TRANSCRIPT_WINDOW_SIZE = 600;
let transcriptWindowStart = 0;
const SESSION_WINDOW_SIZE = 100;
let sessionWindowStart = 0;
let newConversationMode: ConversationMode = "chat";
let workTransitionPending = false;
// This identity belongs to the displayed composer, even while settings inputs change.
let composerAccount = accountKey(formScope());
let composerAccountEstablished = false;

const fields = ["organization", "team", "actor", "workspace", "model", "title"];
const identityFields = ["organization", "team", "actor"];
let settingsDirty = false;
let actionResolver: ((value: Record<string, string> | null) => void) | null = null;
let commandSelection = 0;
let drawerReturnFocus: HTMLElement | null = null;
let developmentInstanceId: string | null = null;
let developmentReloadTimer: ReturnType<typeof window.setInterval> | null = null;
let mentionAbort: AbortController | null = null;
let mentionTimer: ReturnType<typeof window.setTimeout> | null = null;
let mentionSelection = 0;
let contextPickerSelection = 0;
let contextPickerOptions: ContextPickerOption[] = [];
let transcriptFollowing = true;
let composerSubmissionPending = false;
let clientPresence: ClientPresence[] = [];
let presenceTimer: ReturnType<typeof window.setInterval> | null = null;
const highlightClient = new HighlightClient();
const {
  appendBlocks: appendMarkdownBlocks,
  createCodeBlock,
  renderMessageContent,
} = createMarkdownRenderer({
  copyText,
  highlight: (code, language, resolve) =>
    highlightClient.request(code, language, resolve),
});
const {
  addHook,
  addMcpServer,
  addSkill,
  loadExtensionCatalog,
  managePluginMarketplaces,
  renderExtensionList,
  showExtensions,
} = createExtensionsPage({
  lookup: $,
  api,
  state,
  scope,
  requestAction,
  toast,
  copyText,
  closeDrawers,
  closeUserMenu,
  catalogQuery,
  routePath,
});
const {
  loadArtifactPage,
  loadMoreArtifacts,
  renderArtifactList,
  renderProjects,
  resetAccountLibrary,
  showArtifacts,
  showProjects,
} = createWorkspaceLibrary({
  lookup: $,
  api,
  state,
  catalogQuery,
  isCurrent,
  selectSession,
  closeDrawers,
  closeUserMenu,
  routePath,
  workspaceName,
  copyText,
  toast,
  announce,
  appendMarkdownBlocks,
  createCodeBlock,
  renderReviewReport,
});
const {
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
} = createTeamWorkPage({
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
});
function currentPresenceClientId(): string | null {
  // Identity is bound to the authenticated account, never an edited settings form.
  if (!state.connected || !state.authenticatedScope) return null;
  return accountPresenceClientId(sessionStorage, accountKey(state.authenticatedScope), () =>
    `web:${globalThis.crypto?.randomUUID?.() || `${Date.now()}-${Math.random().toString(16).slice(2)}`}`
  );
}
const themeOrder = ["system", "light", "dark"];
const permissionLabels: Record<PermissionMode, string> = {
  manual: "Manual",
  accept_edits: "Accept edits",
  workspace: "Workspace",
  plan: "Plan",
};

interface ContextPickerOption {
  id: string;
  label: string;
  detail: string;
  meta: string[];
  current?: boolean;
  disabled?: boolean;
  disabledReason?: string | null;
  select: () => Promise<void> | void;
}

function currentTheme(): string {
  const saved = localStorage.getItem("oc.theme");
  return saved && themeOrder.includes(saved) ? saved : "system";
}

function applyTheme(theme = currentTheme()) {
  document.documentElement.dataset.theme = theme;
  localStorage.setItem("oc.theme", theme);
  $("theme-toggle").textContent = `Theme: ${theme.charAt(0).toUpperCase()}${theme.slice(1)}`;
  const dark = theme === "dark" || (theme === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.querySelector('meta[name="theme-color"]')?.setAttribute("content", dark ? "#1c1b19" : "#f7f6f2");
}

function closeUserMenu() {
  const menu = document.querySelector<HTMLDetailsElement>(".user-menu");
  if (menu) menu.open = false;
}

function routePath(path: string, replace = false) {
  if (window.location.pathname === path) return;
  window.history[replace ? "replaceState" : "pushState"]({}, "", path);
}

function showWorkspace({ replace = false, updateRoute = true }: ViewOptions = {}) {
  document.body.classList.remove("team-mode");
  $("team-view").hidden = true;
  $("projects-view").hidden = true;
  $("artifacts-view").hidden = true;
  $("extensions-view").hidden = true;
  $("workspace-shell").hidden = false;
  $("open-team").classList.remove("active");
  $("open-team").removeAttribute("aria-current");
  closeUserMenu();
  if (updateRoute) routePath(sessionRoute(state.session?.id || null), replace);
}

const teamSectionTargets: Record<TeamSection, string> = {
  overview: "team-panel",
  work: "team-work-page",
  goals: "team-goals-page",
  agents: "team-agents-page",
  capacity: "team-capacity-page",
  ownership: "team-ownership-page",
  budgets: "team-budgets-page",
  approvals: "team-approvals-page",
  outcomes: "team-outcomes-page",
  audit: "team-audit-page",
};

function selectTeamSection(section: TeamSection, scroll: ScrollBehavior = "auto") {
  document.querySelectorAll<HTMLElement>(".team-tabs button").forEach((button) => {
    const selected = button.dataset.teamSection === section;
    button.classList.toggle("active", selected);
    if (selected) button.setAttribute("aria-current", "page");
    else button.removeAttribute("aria-current");
  });
  document.querySelectorAll<HTMLElement>(".team-page[data-team-page]").forEach((page) => {
    page.hidden = page.dataset.teamPage !== section;
  });
  $("team-view").scrollTo({ top: 0, behavior: scroll });
}

function showTeam({
  replace = false,
  updateRoute = true,
  section = "overview",
}: ViewOptions & { section?: TeamSection } = {}) {
  closeDrawers(false);
  document.body.classList.add("team-mode");
  $("team-view").hidden = false;
  $("projects-view").hidden = true;
  $("artifacts-view").hidden = true;
  $("extensions-view").hidden = true;
  $("workspace-shell").hidden = true;
  $("open-team").classList.add("active");
  $("open-team").setAttribute("aria-current", "page");
  document.body.classList.remove("mobile-sidebar-open");
  closeUserMenu();
  if (updateRoute) routePath(teamRoute(section), replace);
  if (state.connected) refreshTeam().catch((error) => toast(error.message));
  selectTeamSection(section);
  $("team-title").focus({ preventScroll: true });
}

async function restoreRoute() {
  const route = parseRoute(window.location.pathname);
  if (route.type === "team") {
    showTeam({ replace: true, updateRoute: false, section: route.section });
    return;
  }
  if (route.type === "projects") {
    showProjects(null, { replace: true, updateRoute: false });
    return;
  }
  if (route.type === "project") {
    showProjects(route.projectId, { replace: true, updateRoute: false });
    return;
  }
  if (route.type === "artifacts") {
    showArtifacts(null, { replace: true, updateRoute: false });
    return;
  }
  if (route.type === "artifact") {
    showArtifacts(route.artifactId, { replace: true, updateRoute: false });
    return;
  }
  if (route.type === "extensions") {
    showExtensions({ replace: true, updateRoute: false });
    return;
  }
  if (route.type === "session" && state.connected) {
    const session = state.sessions.find((candidate) => candidate.id === route.sessionId);
    if (session) {
      await selectSession(session, { updateRoute: false });
      return;
    }
  }
  showWorkspace({ replace: true, updateRoute: false });
}

async function pollDevelopmentInstance() {
  try {
    const response = await fetch("/v1/health", {
      cache: "no-store",
      credentials: "same-origin",
    });
    if (!response.ok) return;
    const health = await response.json();
    if (!health.development_instance_id) return;
    if (developmentInstanceId && developmentInstanceId !== health.development_instance_id) {
      window.location.reload();
      return;
    }
    developmentInstanceId = health.development_instance_id;
  } catch (_) {
    // A watcher restart briefly makes the loopback service unavailable. The
    // next successful poll observes its new instance ID and reloads the page.
  }
}

function enableDevelopmentAutoReload() {
  if (!document.querySelector<HTMLMetaElement>('meta[name="s-code-bootstrap"]')) return;
  pollDevelopmentInstance();
  developmentReloadTimer = window.setInterval(pollDevelopmentInstance, 750);
}

function workspaceName(uri: string) {
  const clean = String(uri || "").replace(/\/$/, "");
  if (!clean) return "No workspace";
  try { return decodeURIComponent(clean.split("/").filter(Boolean).pop() || clean); } catch (_) { return clean; }
}

function actorIdentity(actorId: string) {
  const actor = actorId.trim();
  if (!actor || actor === "user_local") return { label: "Local user", initials: "L" };
  const words = actor
    .replace(/^user[-_]/i, "")
    .split(/[-_\s]+/)
    .filter(Boolean);
  const label = words.length
    ? words.map((word) => word.charAt(0).toUpperCase() + word.slice(1)).join(" ")
    : "Local user";
  const initials = words.slice(0, 2).map((word) => word.charAt(0).toUpperCase()).join("") || "L";
  return { label, initials };
}

function updateContextChips() {
  const work = hasWorkspace(state.session);
  const mode = state.session ? sessionMode(state.session) : newConversationMode;
  $("workspace-chip").hidden = !work;
  $("workspace-chip").textContent = work ? workspaceName(state.session!.workspace_uri) : "No workspace";
  $("workspace-chip").title = state.session?.workspace_uri || "";
  $("permission-chip").hidden = mode === "chat";
  $("session-mode-badge").textContent = mode === "work" ? "Work" : "Chat";
  $("session-mode-badge").dataset.mode = mode;
  $("new-session-mode").hidden = Boolean(state.session);
  $("work-directory-field").hidden = Boolean(state.session) || mode !== "work";
  $("choose-chat").setAttribute("aria-pressed", String(mode === "chat"));
  $("choose-work").setAttribute("aria-pressed", String(mode === "work"));
  $("mode-description").textContent = mode === "chat" ? "Ask, learn, and explore." : "Create, edit, and run files.";
  $("start-work").hidden = !state.session || mode !== "chat" || state.session.status !== "active";
  $("start-work").disabled = !state.connected || state.turnRunning || workTransitionPending;
  $("start-work").textContent = workTransitionPending ? "Starting…" : "Start work";
  $("start-work").title = state.turnRunning ? "Wait for this reply to finish, or ask S-Code to start work." : "Create a working folder and keep this conversation";
  $("empty-title").textContent = mode === "chat" ? "What’s on your mind?" : "What are we building?";
  $("empty-guidance").textContent = mode === "chat"
    ? "Ask a question or explore an idea. When you need files, S-Code can start Work in a new folder."
    : "Describe an outcome. S-Code can create files, edit code, and run tests in your working directory.";
  document.querySelector<HTMLElement>(".starter-actions")!.hidden = mode === "chat";
  for (const id of ["show-diff", "quick-diff", "review-session", "show-checkpoints", "undo-turn"]) {
    $(id).hidden = !work;
    if (!work) $(id).disabled = true;
  }
  $("show-diff").disabled = !work;
  $("quick-diff").disabled = !work;
  $("review-session").disabled = !work || !state.capabilities.has("review.read_only");
  $("show-checkpoints").disabled = !work;
  renderSessionGoal();
  if (!work) closeMentionMenu();
  if (state.session) $("session-meta").textContent = sessionDescription(state.session);
  else {
    $("session-title").textContent = mode === "chat" ? "New chat" : "New work";
    $("session-meta").textContent = mode === "chat" ? "Conversation without a working directory" : "Choose a project or start in a new folder";
  }
  $("model-chip").textContent = state.session?.model || $("model").value.trim() || "No model";
  $("permission-chip").textContent = permissionLabels[state.permissionMode];
  const identity = actorIdentity($("actor").value);
  $("user-name").textContent = identity.label;
  $("user-avatar").textContent = identity.initials;
}

function sessionDescription(session: Session) {
  return `${sessionMode(session) === "work" ? "Work" : "Chat"} · ${session.status} · ${session.model}${hasWorkspace(session) ? ` · ${session.workspace_uri}` : ""}`;
}

function chooseNewConversationMode(mode: ConversationMode) {
  if (state.session) return;
  newConversationMode = mode;
  updateContextChips();
  $("prompt").focus();
}

async function startWork() {
  const session = state.session;
  if (!session || sessionMode(session) !== "chat" || session.status !== "active" || state.turnRunning || workTransitionPending) return;
  const generation = state.generation;
  workTransitionPending = true;
  updateContextChips();
  try {
    const updated = await api<Session>(`/v1/sessions/${encodeURIComponent(session.id)}/work`, {
      method: "POST",
      body: JSON.stringify({ scope: scope(), reason: "User requested Work" }),
    });
    if (!isCurrent(generation)) return;
    state.sessions = state.sessions.map((item) => item.id === updated.id ? updated : item);
    if (state.session?.id === updated.id) {
      state.session = updated;
      updateContextChips();
      announce(`Work started in ${updated.workspace_uri}. Your conversation is preserved.`);
      toast("Work started. Your conversation is preserved.");
    }
    renderSessions();
  } catch (error) {
    if (isCurrent(generation)) toast(`Could not start Work: ${error.message}`);
  } finally {
    workTransitionPending = false;
    updateContextChips();
  }
}

function activeMention() {
  const prompt = $("prompt").value;
  const match = prompt.match(/(^|\s)@([^\s@]*)$/);
  if (!match) return null;
  return {
    start: prompt.length - match[0].length,
    prefix: match[1],
    query: match[2],
  };
}

function closeMentionMenu() {
  if (mentionAbort) mentionAbort.abort();
  mentionAbort = null;
  $("mention-menu").hidden = true;
  $("mention-menu").replaceChildren();
  mentionSelection = 0;
}

function selectMention(path: string) {
  const mention = activeMention();
  if (!mention) return;
  const prompt = $("prompt");
  prompt.value = `${prompt.value.slice(0, mention.start)}${mention.prefix}@${path} `;
  sessionStorage.setItem(composerTextDraftKey(), prompt.value);
  closeMentionMenu();
  resizePrompt();
  prompt.focus();
}

function updateMentionSelection(index: number) {
  const options = [...$("mention-menu").querySelectorAll("button")];
  if (!options.length) return;
  mentionSelection = (index + options.length) % options.length;
  options.forEach((option, optionIndex) => {
    option.setAttribute("aria-selected", String(optionIndex === mentionSelection));
  });
  options[mentionSelection].scrollIntoView({ block: "nearest" });
}

async function loadFileMentions() {
  const mention = activeMention();
  if (!mention || !state.session || !hasWorkspace(state.session) || !state.capabilities.has("workspace.fuzzy_search")) {
    closeMentionMenu();
    return;
  }
  if (mentionAbort) mentionAbort.abort();
  mentionAbort = new AbortController();
  const controller = mentionAbort;
  const selectedSession = state.session.id;
  const s = scope();
  const query = new URLSearchParams({
    organization_id: s.organization_id,
    team_id: s.team_id,
    actor_id: s.actor_id,
    query: mention.query,
    limit: "12",
  });
  try {
    const paths = await api<WorkspacePathMatch[]>(`/v1/sessions/${encodeURIComponent(selectedSession)}/files?${query}`, {
      signal: controller.signal,
    });
    if (controller.signal.aborted || state.session?.id !== selectedSession) return;
    const menu = $("mention-menu");
    menu.replaceChildren();
    paths.forEach((item, index) => {
      const option = document.createElement("button");
      option.type = "button";
      option.setAttribute("role", "option");
      option.setAttribute("aria-selected", String(index === 0));
      option.textContent = item.path;
      option.addEventListener("click", () => selectMention(item.path));
      menu.append(option);
    });
    mentionSelection = 0;
    menu.hidden = paths.length === 0;
  } catch (error) {
    if (error.name !== "AbortError") closeMentionMenu();
  }
}

function scheduleFileMentions() {
  if (mentionTimer) clearTimeout(mentionTimer);
  mentionTimer = setTimeout(() => {
    mentionTimer = null;
    loadFileMentions();
  }, 100);
}

function updateConversationState(hasMessages = Boolean($("messages").children.length)) {
  $("conversation-view").classList.toggle("is-empty", !hasMessages);
  if (hasMessages) advanceTranscript();
}

function transcriptIsNearBottom() {
  const messages = $("messages");
  return messages.scrollHeight - messages.scrollTop - messages.clientHeight <= 96;
}

function advanceTranscript(force = false) {
  const messages = $("messages");
  if (force) transcriptFollowing = true;
  if (transcriptFollowing) {
    messages.scrollTop = messages.scrollHeight;
    $("jump-latest").hidden = true;
  } else {
    $("jump-latest").hidden = false;
  }
}

function isCurrent(generation: number) {
  return state.connected && generation === state.generation;
}

function resizePrompt() {
  const prompt = $("prompt");
  prompt.style.height = "auto";
  prompt.style.height = `${Math.min(prompt.scrollHeight, 180)}px`;
}

function composerDraftContext(session: Session | null = state.session) {
  return accountDraftContext(composerAccount, session?.id);
}

function composerTextDraftKey(session: Session | null = state.session) {
  return `oc.prompt-draft:${composerDraftContext(session)}`;
}

function saveComposerDraft() {
  if (!composerAccountEstablished) return;
  const key = composerDraftContext();
  const text = $("prompt").value;
  if (text) sessionStorage.setItem(composerTextDraftKey(), text);
  else sessionStorage.removeItem(composerTextDraftKey());
  state.draftFilesByContext.set(key, [...state.draftFiles]);
}

function restoreComposerDraft() {
  if (!composerAccountEstablished) return;
  state.draftFiles = [
    ...(state.draftFilesByContext.get(composerDraftContext()) || []),
  ];
  $("prompt").value = sessionStorage.getItem(composerTextDraftKey()) || "";
  resizePrompt();
  renderDraftAttachments();
  updateSendAction();
}

function transferNewComposerDraft(session: Session) {
  const files = [...state.draftFiles];
  state.draftFilesByContext.set(composerDraftContext(null), []);
  state.draftFilesByContext.set(composerDraftContext(session), files);
  const text = sessionStorage.getItem(composerTextDraftKey(null));
  if (text) sessionStorage.setItem(composerTextDraftKey(session), text);
  sessionStorage.removeItem(composerTextDraftKey(null));
  state.draftFiles = [];
  $("prompt").value = "";
}

function restoreAccountPermission() {
  if (!composerAccountEstablished) { state.permissionMode = "manual"; return; }
  const saved = sessionStorage.getItem(accountPermissionKey(composerAccount));
  state.permissionMode = saved === "accept_edits" || saved === "workspace" || saved === "plan" ? saved : "manual";
}

function switchComposerAccount(nextScope: Scope) {
  const nextAccount = accountKey(nextScope);
  if (!composerAccountEstablished) {
    composerAccount = nextAccount;
    composerAccountEstablished = true;
    if ($("prompt").value) saveComposerDraft();
    restoreAccountPermission();
    restoreComposerDraft();
    updateContextChips();
    return;
  }
  if (nextAccount === composerAccount) return;
  saveComposerDraft();
  composerAccount = nextAccount;
  // Connection invalidation has already cleared the selected session.
  state.draftFiles = [];
  $("prompt").value = "";
  $("attachment-input").value = "";
  restoreAccountPermission();
  restoreComposerDraft();
  updateContextChips();
  routePath("/", true);
}

function formatBytes(value: number): string {
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${Math.ceil(value / 1024)} KB`;
  return `${(value / (1024 * 1024)).toFixed(1)} MB`;
}

function renderDraftAttachments() {
  const target = $("attachment-list");
  target.replaceChildren();
  target.hidden = state.draftFiles.length === 0;
  state.draftFiles.forEach((file, index) => {
    const chip = document.createElement("div");
    chip.className = "attachment-chip";
    const name = document.createElement("span");
    name.textContent = file.name;
    name.title = file.name;
    const size = document.createElement("small");
    size.textContent = formatBytes(file.size);
    const remove = document.createElement("button");
    remove.type = "button";
    remove.textContent = "×";
    remove.setAttribute("aria-label", `Remove ${file.name}`);
    remove.addEventListener("click", () => {
      state.draftFiles.splice(index, 1);
      state.draftFilesByContext.set(composerDraftContext(), [...state.draftFiles]);
      renderDraftAttachments();
      updateSendAction();
      $("prompt").focus();
    });
    chip.append(name, size, remove);
    target.append(chip);
  });
}

function addDraftFiles(files: Iterable<File>) {
  const incoming = [...files];
  if (!state.capabilities.has("composer.attachments.v1")) {
    toast("This server does not support attachments.");
    return;
  }
  for (const file of incoming) {
    if (!file.size) {
      toast(`${file.name} is empty.`);
      continue;
    }
    if (file.size > 5 * 1024 * 1024) {
      toast(`${file.name} exceeds the 5 MB limit.`);
      continue;
    }
    if (state.draftFiles.length >= 8) {
      toast("A message can contain at most eight files.");
      break;
    }
    state.draftFiles.push(file);
  }
  state.draftFilesByContext.set(composerDraftContext(), [...state.draftFiles]);
  renderDraftAttachments();
  updateSendAction();
}

async function deleteDraftAttachment(id: string, s: Scope) {
  const query = new URLSearchParams({
    organization_id: s.organization_id,
    team_id: s.team_id,
    actor_id: s.actor_id,
  });
  await api(`/v1/attachments/${encodeURIComponent(id)}?${query}`, { method: "DELETE" });
}

async function uploadDraftAttachments(
  session: Session,
  files: File[],
): Promise<AttachmentMetadata[]> {
  const generation = state.generation;
  const attachmentScope = { ...session.scope };
  const stillCurrent = () => isCurrent(generation) && state.session?.id === session.id
    && ownsSession(session, state.authenticatedScope);
  const uploaded: AttachmentMetadata[] = [];
  try {
    for (const file of files) {
      const prepared = await prepareAttachment(file, stillCurrent);
      // Recheck after the asynchronous helper returns, before initiating a request.
      if (!stillCurrent()) throw new DOMException("Account or session changed", "AbortError");
      uploaded.push(await api(`/v1/sessions/${encodeURIComponent(session.id)}/attachments`, {
        method: "POST",
        body: JSON.stringify({ scope: attachmentScope, ...prepared }),
      }));
      if (!stillCurrent()) throw new DOMException("Account or session changed", "AbortError");
    }
    return uploaded;
  } catch (error) {
    if (isCurrent(generation) && ownsSession(session, state.authenticatedScope)) {
      await Promise.allSettled(uploaded.map((attachment) => deleteDraftAttachment(attachment.id, attachmentScope)));
    }
    throw error;
  }
}

function announce(message: string) {
  $("announcer").textContent = "";
  window.setTimeout(() => { $("announcer").textContent = message; }, 20);
}

function toast(message: string) {
  const item = document.createElement("div");
  item.className = "toast";
  item.textContent = message;
  $("toast-region").append(item);
  window.setTimeout(() => item.remove(), 3200);
}

function notificationMode(): NotificationMode {
  const value = localStorage.getItem("oc.notification-mode");
  return value === "background" || value === "always" ? value : "off";
}

function updateNotificationControls() {
  const supported = "Notification" in globalThis;
  const mode = notificationMode();
  $("notification-mode").value = mode;
  $("notification-mode").disabled = !supported;
  $("enable-notifications").disabled = !supported || Notification.permission === "granted";
  $("enable-notifications").textContent = !supported
    ? "Desktop notifications unavailable"
    : Notification.permission === "granted"
      ? "Desktop notifications enabled"
      : Notification.permission === "denied"
        ? "Notifications blocked in browser settings"
        : "Enable desktop notifications";
  $("notification-status").textContent = !supported
    ? "This browser does not expose desktop notifications."
    : Notification.permission === "granted"
      ? mode === "off"
        ? "Permission granted; notifications are currently off."
        : mode === "background"
          ? "You will be notified when this tab is in the background."
          : "You will be notified for decisions and task completion."
      : Notification.permission === "denied"
        ? "Permission is blocked. Change it in browser site settings."
        : "Desktop notifications are off until you explicitly enable them.";
}

async function enableNotifications() {
  if (!("Notification" in globalThis)) {
    updateNotificationControls();
    return;
  }
  const permission = await Notification.requestPermission();
  if (permission === "granted" && notificationMode() === "off") {
    localStorage.setItem("oc.notification-mode", "background");
  }
  updateNotificationControls();
}

function notifyUser(tag: string, message: string) {
  const mode = notificationMode();
  if (
    mode === "off"
    || !("Notification" in globalThis)
    || Notification.permission !== "granted"
    || (mode === "background" && document.visibilityState === "visible")
  ) {
    return false;
  }
  new Notification("S-Code", {
    body: message,
    tag,
  });
  return true;
}

async function copyText(text: string, success = "Copied") {
  try {
    await navigator.clipboard.writeText(text);
  } catch (_) {
    const temporary = document.createElement("textarea");
    temporary.value = text;
    temporary.setAttribute("readonly", "");
    temporary.className = "sr-only";
    document.body.append(temporary);
    temporary.select();
    document.execCommand("copy");
    temporary.remove();
  }
  toast(success);
}

function closeActionDialog(value: Record<string, string> | null = null) {
  if ($("action-dialog").open) $("action-dialog").close();
  const resolve = actionResolver;
  actionResolver = null;
  if (resolve) resolve(value);
}

function requestAction({
  eyebrow = "Team action",
  title,
  description,
  details = [],
  confirm = "Continue",
  danger = false,
  fields: requestedFields = [],
}: ActionOptions): Promise<Record<string, string> | null> {
  if (actionResolver) closeActionDialog(null);
  $("action-eyebrow").textContent = eyebrow;
  $("action-title").textContent = title;
  $("action-description").textContent = description || "";
  $("confirm-action").textContent = confirm;
  $("confirm-action").classList.toggle("danger", danger);
  $("confirm-action").classList.toggle("primary", !danger);
  const container = $("action-fields");
  container.replaceChildren();
  if (details.length) {
    const list = document.createElement("ul");
    list.className = "impact-list";
    details.forEach((detail) => {
      const item = document.createElement("li");
      item.textContent = detail;
      list.append(item);
    });
    container.append(list);
  }
  requestedFields.forEach((field) => {
    const label = document.createElement("label");
    label.textContent = field.label;
    const input = document.createElement(field.multiline ? "textarea" : field.options ? "select" : "input") as unknown as AppElement;
    input.name = field.name;
    input.id = `action-${field.name}`;
    input.required = Boolean(field.required);
    if (!field.multiline && !field.options) input.type = field.type || "text";
    if (field.placeholder) input.placeholder = field.placeholder;
    if (field.min !== undefined) input.min = String(field.min);
    if (field.max !== undefined) input.max = String(field.max);
    if (field.maxlength !== undefined) input.maxLength = Number(field.maxlength);
    if (field.options) field.options.forEach(([value, text]) => input.append(new Option(text, value)));
    if (field.value !== undefined) input.value = String(field.value);
    label.append(input);
    container.append(label);
  });
  $("action-dialog").showModal();
  window.setTimeout(() => container.querySelector<HTMLElement>("input, textarea, select")?.focus(), 0);
  return new Promise<Record<string, string> | null>((resolve) => { actionResolver = resolve; });
}

function submitActionDialog(event: SubmitEvent) {
  event.preventDefault();
  if (!$("action-form").reportValidity()) return;
  const values = Object.fromEntries(
    [...new FormData($("action-form") as unknown as HTMLFormElement).entries()]
      .map(([key, value]) => [key, String(value)]),
  );
  closeActionDialog(values);
}

function showShortcuts() {
  if (!$("shortcuts-dialog").open) {
    $("shortcuts-dialog").showModal();
    document.querySelector<HTMLElement>(".dialog-close")?.focus();
  }
}

function renderSessionGoal() {
  const goal = hasWorkspace(state.session) ? state.goal : null;
  const container = $("session-goal");
  container.hidden = !goal;
  for (const id of ["edit-session-goal", "toggle-session-goal", "clear-session-goal"]) $(id).disabled = !goal;
  if (!goal) {
    container.removeAttribute("data-status");
    $("session-goal-objective").textContent = "";
    $("session-goal-progress").textContent = "";
    return;
  }
  container.dataset.status = goal.status;
  $("session-goal-status").textContent = `Goal · ${goal.status}`;
  $("session-goal-objective").textContent = goal.objective;
  const tokens = goal.input_tokens + goal.output_tokens;
  $("session-goal-progress").textContent = goal.token_budget
    ? `${tokens.toLocaleString()} / ${goal.token_budget.toLocaleString()} tokens · ${goal.continuation_count} continuations`
    : `${tokens.toLocaleString()} tokens · ${goal.continuation_count} continuations${goal.blocked_reason ? ` · ${goal.blocked_reason}` : ""}`;
  $("toggle-session-goal").textContent = goal.status === "active" ? "Pause" : "Resume";
  $("toggle-session-goal").disabled = goal.status === "completed";
  $("edit-session-goal").disabled = goal.status === "completed";
}

async function loadSessionGoal(sessionId = state.session?.id) {
  if (!sessionId || !hasWorkspace(state.session) || !state.capabilities.has("session.goal.v1")) {
    state.goal = null;
    renderSessionGoal();
    return;
  }
  const goal = await api<SessionGoal | null>(
    `/v1/sessions/${encodeURIComponent(sessionId)}/goal?${catalogQuery()}`,
  );
  if (state.session?.id !== sessionId) return;
  state.goal = goal;
  renderSessionGoal();
}

async function editSessionGoal() {
  const session = state.session;
  if (!session || !hasWorkspace(session)) return;
  const current = state.goal;
  const values = await requestAction({
    eyebrow: "Persistent Goal",
    title: current ? "Edit Goal" : "Start a Goal",
    description: current
      ? "Update the outcome S-Code should keep working toward."
      : "S-Code will keep working across turns until this outcome is complete, paused, or genuinely blocked.",
    confirm: current ? "Save Goal" : "Start Goal",
    fields: [{
      name: "objective",
      label: "Objective",
      multiline: true,
      maxlength: 4000,
      required: true,
      value: current?.objective || "",
      placeholder: "Describe the verified outcome you want",
    }],
  });
  if (!values || state.session?.id !== session.id || !hasWorkspace(state.session)) return;
  const sessionId = session.id;
  const objective = values.objective.trim();
  const goal = current
    ? await api<SessionGoal>(`/v1/sessions/${encodeURIComponent(sessionId)}/goal`, {
      method: "PATCH",
      body: JSON.stringify({
        scope: scope(),
        objective,
        status: null,
        auto_continue: null,
        expected_revision: current.revision,
        blocked_reason: null,
      }),
    })
    : await api<SessionGoal>(`/v1/sessions/${encodeURIComponent(sessionId)}/goal`, {
      method: "PUT",
      body: JSON.stringify({
        scope: scope(),
        objective,
        auto_continue: true,
        token_budget: null,
      }),
    });
  if (state.session?.id !== sessionId) return;
  state.goal = goal;
  renderSessionGoal();
  toast(current ? "Goal updated" : "Goal started");
  if (!current && !state.turnRunning) await executeContent(objective);
}

async function toggleSessionGoal() {
  const session = state.session;
  const current = state.goal;
  if (!session || !hasWorkspace(session) || !current || current.status === "completed") return;
  const status: SessionGoalStatus = current.status === "active" ? "paused" : "active";
  const goal = await api<SessionGoal>(`/v1/sessions/${encodeURIComponent(session.id)}/goal`, {
    method: "PATCH",
    body: JSON.stringify({
      scope: scope(),
      objective: null,
      status,
      auto_continue: null,
      expected_revision: current.revision,
      blocked_reason: null,
    }),
  });
  if (state.session?.id !== session.id) return;
  state.goal = goal;
  renderSessionGoal();
  toast(status === "active" ? "Goal resumed" : "Goal paused");
}

async function clearSessionGoal() {
  const session = state.session;
  const current = state.goal;
  if (!session || !hasWorkspace(session) || !current) return;
  const values = await requestAction({
    eyebrow: "Persistent Goal",
    title: "Clear this Goal?",
    description: "Automatic continuation will stop. Existing conversation history is preserved.",
    confirm: "Clear Goal",
    danger: true,
  });
  if (!values) return;
  await api<SessionGoal>(`/v1/sessions/${encodeURIComponent(session.id)}/goal`, {
    method: "DELETE",
    body: JSON.stringify({ scope: scope(), expected_revision: current.revision }),
  });
  if (state.session?.id !== session.id) return;
  state.goal = null;
  renderSessionGoal();
  toast("Goal cleared");
}

function commandDefinitions(): CommandDefinition[] {
  return [
    { label: "Focus task prompt", detail: "Return to the primary action", shortcut: "⌘L", run: () => $("prompt").focus() },
    { label: "Start a new chat", detail: "Start a conversation without a working directory", shortcut: "⌘N", run: () => { clearSessionSelection(); showWorkspace(); } },
    { label: state.goal ? "Manage persistent Goal" : "Start persistent Goal", detail: "Keep working across turns until a verified outcome is reached", shortcut: "", enabled: () => hasWorkspace(state.session) && state.capabilities.has("session.goal.v1"), run: editSessionGoal },
    { label: "Show changes", detail: "Review the current Git diff when you need it", shortcut: "⌘D", enabled: () => hasWorkspace(state.session), run: async () => { showWorkspace(); openDrawer("inspector"); await showDiff(); } },
    { label: "Toggle history", detail: "Show or hide recent sessions", shortcut: "⌘B", run: toggleHistory },
    { label: "Search task history", detail: "Find a session by title, workspace, or model", shortcut: "⌘F", run: focusSessionSearch },
    { label: "Settings", detail: "Connection, identity, model, and workspace", shortcut: "", run: () => openDrawer("settings-drawer") },
    { label: "Open projects", detail: "Inspect workspace instructions, context, memory, and recent sessions", shortcut: "", run: () => showProjects() },
    { label: "Open artifacts", detail: "Browse reports, documents, diffs, and larger results across sessions", shortcut: "", run: () => showArtifacts() },
    { label: "Open extensions", detail: "Inspect MCP servers, tools, permissions, trust, and sources", shortcut: "", run: () => showExtensions() },
    { label: "Start background terminal", detail: "Run a confirmed PTY process that continues after this page closes", shortcut: "", enabled: () => state.capabilities.has("terminal.background_pty.v1"), run: createBackgroundTerminal },
    { label: "Show context", detail: "Inspect token usage, sources, AGENTS.md, and memory", shortcut: "", enabled: () => Boolean(state.session), run: async () => { openDrawer("inspector"); await showContext(); } },
    { label: "Start side conversation", detail: "Ask in a temporary Fork without changing the current Session", shortcut: "", enabled: () => Boolean(state.session) && !state.turnRunning && state.capabilities.has("session.side_conversation.v1"), run: createSideConversation },
    { label: "Open Team", detail: "Goals, ownership, capacity, budget, and work queue", shortcut: "", run: showTeam },
    { label: "Cycle theme", detail: "Switch among system, light, and dark", shortcut: "", run: cycleTheme },
    { label: "Show keyboard shortcuts", detail: "See every keyboard-first action", shortcut: "⌘/", run: showShortcuts },
  ];
}

function cycleTheme() {
  const theme = currentTheme();
  applyTheme(themeOrder[(themeOrder.indexOf(theme) + 1) % themeOrder.length]);
}

function focusSessionSearch() {
  if (window.matchMedia("(max-width: 760px)").matches) document.body.classList.add("mobile-sidebar-open");
  else if (document.body.classList.contains("sidebar-collapsed")) toggleHistory();
  $("session-search").focus();
}

function renderCommands() {
  const query = $("command-query").value.trim().toLowerCase();
  const commands = commandDefinitions().filter((command) => !query || `${command.label} ${command.detail}`.toLowerCase().includes(query));
  commandSelection = Math.max(0, Math.min(commandSelection, commands.length - 1));
  const results = $("command-results");
  results.replaceChildren();
  commands.forEach((command, index) => {
    const button = document.createElement("button");
    button.type = "button";
    button.id = `command-option-${index}`;
    button.className = `command-item${index === commandSelection ? " selected" : ""}`;
    button.setAttribute("role", "option");
    button.setAttribute("aria-selected", String(index === commandSelection));
    button.disabled = command.enabled ? !command.enabled() : false;
    const text = document.createElement("span");
    const label = document.createElement("span");
    const detail = document.createElement("small");
    label.textContent = command.label;
    detail.textContent = command.detail;
    text.append(label, detail);
    button.append(text);
    if (command.shortcut) { const key = document.createElement("kbd"); key.textContent = command.shortcut; button.append(key); }
    button.addEventListener("click", () => runCommand(command));
    results.append(button);
  });
  $("command-query").setAttribute("aria-activedescendant", commands.length ? `command-option-${commandSelection}` : "");
  return commands;
}

function runCommand(command: CommandDefinition) {
  $("command-dialog").close();
  Promise.resolve(command.run()).catch((error) => toast(error.message));
}

function openCommands() {
  commandSelection = 0;
  $("command-query").value = "";
  renderCommands();
  $("command-dialog").showModal();
  $("command-query").focus();
}

function filteredContextPickerOptions() {
  const query = $("context-picker-query").value.trim().toLowerCase();
  return contextPickerOptions.filter((option) =>
    !query || `${option.label} ${option.detail} ${option.meta.join(" ")}`.toLowerCase().includes(query)
  );
}

function renderContextPicker() {
  const options = filteredContextPickerOptions();
  contextPickerSelection = Math.max(0, Math.min(contextPickerSelection, options.length - 1));
  const target = $("context-picker-results");
  target.replaceChildren();
  if (!options.length) {
    const empty = document.createElement("p");
    empty.className = "command-hint";
    empty.textContent = "No matching options";
    target.append(empty);
  }
  options.forEach((option, index) => {
    const button = document.createElement("button");
    button.type = "button";
    button.id = `context-picker-option-${index}`;
    button.className = `command-item${index === contextPickerSelection ? " selected" : ""}${option.current ? " current" : ""}`;
    button.setAttribute("role", "option");
    button.setAttribute("aria-selected", String(index === contextPickerSelection));
    button.disabled = Boolean(option.disabled);
    if (option.disabledReason) button.title = option.disabledReason;
    const copy = document.createElement("span");
    const label = document.createElement("span");
    label.textContent = `${option.current ? "✓ " : ""}${option.label}`;
    const detail = document.createElement("small");
    detail.textContent = option.disabledReason || option.detail;
    copy.append(label, detail);
    const meta = document.createElement("span");
    meta.className = "picker-meta";
    option.meta.forEach((value) => {
      const chip = document.createElement("span");
      chip.textContent = value;
      meta.append(chip);
    });
    button.append(copy, meta);
    button.addEventListener("click", () => {
      Promise.resolve(option.select())
        .then(() => $("context-picker-dialog").close())
        .catch((error) => toast(error.message));
    });
    target.append(button);
  });
  $("context-picker-query").setAttribute(
    "aria-activedescendant",
    options.length ? `context-picker-option-${contextPickerSelection}` : "",
  );
  return options;
}

function openContextPicker({
  eyebrow,
  title,
  placeholder,
  hint,
  options,
}: {
  eyebrow: string;
  title: string;
  placeholder: string;
  hint: string;
  options: ContextPickerOption[];
}) {
  contextPickerOptions = options;
  contextPickerSelection = Math.max(0, options.findIndex((option) => option.current));
  $("context-picker-eyebrow").textContent = eyebrow;
  $("context-picker-title").textContent = title;
  $("context-picker-query").placeholder = placeholder;
  $("context-picker-query").value = "";
  $("context-picker-hint").textContent = hint;
  renderContextPicker();
  $("context-picker-dialog").showModal();
  $("context-picker-query").focus();
}

function catalogQuery() {
  const s = scope();
  return new URLSearchParams({
    organization_id: s.organization_id,
    team_id: s.team_id,
    actor_id: s.actor_id,
  });
}

async function chooseModel(modelId: string) {
  const session = state.session;
  const generation = state.generation;
  if (session) {
    const updated = await api<Session>(`/v1/sessions/${encodeURIComponent(session.id)}`, {
      method: "PATCH",
      body: JSON.stringify({ scope: scope(), model: modelId }),
    });
    if (!isCurrent(generation) || state.session?.id !== session.id || !ownsSession(updated, state.authenticatedScope)) return;
    state.session = updated;
    state.sessions = state.sessions.map((session) => session.id === updated.id ? updated : session);
    $("session-meta").textContent = sessionDescription(updated);
    renderSessions();
  } else {
    $("model").value = modelId;
    await api("/v1/settings", {
      method: "PUT",
      body: JSON.stringify(daemonSettings()),
    });
    settingsDirty = false;
  }
  updateContextChips();
  toast(`Model set to ${modelId}`);
}

async function openModelPicker() {
  if (!state.connected) {
    toast("Connect the local service before choosing a model.");
    return;
  }
  if (!state.capabilities.has("composer.model_picker.v1")) {
    openDrawer("settings-drawer");
    return;
  }
  const models = await api<ModelCatalogEntry[]>(`/v1/models?${catalogQuery()}`);
  openContextPicker({
    eyebrow: "Model",
    title: "Choose a model",
    placeholder: "Search model or provider",
    hint: "Availability and policy are decided by the connected server.",
    options: models.map((model) => ({
      id: model.id,
      label: model.display_name,
      detail: `${model.provider} · ${model.source.replaceAll("_", " ")}`,
      meta: [
        model.recommended ? "Recommended" : "",
        model.context_window ? `${Math.round(model.context_window / 1000)}k context` : "",
        model.supports_reasoning === true ? "Reasoning" : "",
        model.cost_tier || "",
      ].filter(Boolean),
      current: model.id === (state.session?.model || $("model").value.trim()),
      disabled: !model.available,
      disabledReason: model.locked_reason,
      select: () => chooseModel(model.id),
    })),
  });
}

async function choosePermissionMode(mode: PermissionMode) {
  const session = state.session;
  const generation = state.generation;
  if (session) {
    const preferences = await api<SessionPreferences>(
      `/v1/sessions/${encodeURIComponent(session.id)}/preferences`,
      {
        method: "PATCH",
        body: JSON.stringify({ scope: scope(), permission_mode: mode }),
      },
    );
    if (!isCurrent(generation) || state.session?.id !== session.id) return;
    state.permissionMode = preferences.permission_mode;
  } else {
    state.permissionMode = mode;
    sessionStorage.setItem(accountPermissionKey(composerAccount), mode);
  }
  updateContextChips();
  toast("The active Team policy still decides what is allowed.");
}

function applyAssistantAlias(alias: string) {
  state.assistantAlias = alias.trim() || "S-Code";
  document.querySelectorAll<HTMLElement>(".message.assistant").forEach((message) => {
    message.setAttribute("aria-label", `${state.assistantAlias} response`);
    const label = message.querySelector<HTMLElement>(".message-label");
    if (label) label.textContent = state.assistantAlias;
  });
}

async function loadSessionPreferences(sessionId: string) {
  const preferences = await api<SessionPreferences>(
    `/v1/sessions/${encodeURIComponent(sessionId)}/preferences?${catalogQuery()}`,
  );
  if (state.session?.id !== sessionId) return;
  state.permissionMode = preferences.permission_mode;
  applyAssistantAlias(preferences.assistant_alias);
  updateContextChips();
}

async function openPermissionPicker() {
  if (!state.connected) {
    toast("Connect the local service before choosing permissions.");
    return;
  }
  if (!state.capabilities.has("composer.permission_picker.v1")) {
    toast("This server does not support persisted permission profiles.");
    return;
  }
  const profiles = await api<PermissionProfile[]>(`/v1/permission-profiles?${catalogQuery()}`);
  openContextPicker({
    eyebrow: "Permissions",
    title: "Choose a permission profile",
    placeholder: "Search permission behavior",
    hint: "Profiles can reduce prompts but never bypass Team policy or sandbox limits.",
    options: profiles.map((profile) => ({
      id: profile.mode,
      label: profile.label,
      detail: profile.description,
      meta: [profile.file_changes, profile.commands, profile.network, profile.source.replaceAll("_", " ")],
      current: profile.mode === state.permissionMode,
      disabled: Boolean(profile.locked_reason),
      disabledReason: profile.locked_reason,
      select: () => choosePermissionMode(profile.mode),
    })),
  });
}

async function loadConfigurationSources() {
  const target = $("configuration-sources");
  if (!state.connected) {
    target.className = "configuration-sources empty";
    target.textContent = "Connect to inspect effective configuration.";
    return;
  }
  target.className = "configuration-sources";
  target.textContent = "Loading effective values…";
  const generation = state.generation;
  try {
    const [models, profiles] = await Promise.all([
      api<ModelCatalogEntry[]>(`/v1/models?${catalogQuery()}`),
      api<PermissionProfile[]>(`/v1/permission-profiles?${catalogQuery()}`),
    ]);
    if (!isCurrent(generation)) return;
    const modelId = state.session?.model || $("model").value.trim();
    const model = models.find((entry) => entry.id === modelId);
    const profile = profiles.find((entry) => entry.mode === state.permissionMode);
    target.replaceChildren();
    [
      {
        label: "Model",
        value: model?.display_name || modelId || "Not configured",
        source: model?.source || "local default",
        locked: model?.locked_reason || null,
      },
      {
        label: "Permission profile",
        value: profile?.label || permissionLabels[state.permissionMode],
        source: profile?.source || "session",
        locked: profile?.locked_reason || null,
      },
      {
        label: "Workspace",
        value: state.session ? (state.session.workspace_uri || "Chat — no working directory") : "Chosen when starting Work",
        source: state.session ? "session" : "daemon default",
        locked: null,
      },
    ].forEach((entry) => {
      const row = document.createElement("article");
      row.className = `configuration-source${entry.locked ? " locked" : ""}`;
      const label = document.createElement("strong");
      label.textContent = `${entry.label} · ${entry.value}`;
      const source = document.createElement("span");
      source.textContent = `Source: ${entry.source.replaceAll("_", " ")}`;
      row.append(label, source);
      if (entry.locked) {
        const reason = document.createElement("small");
        reason.textContent = `Managed and locked: ${entry.locked}`;
        row.append(reason);
      }
      target.append(row);
    });
  } catch (error) {
    if (isCurrent(generation)) {
      target.className = "configuration-sources empty";
      target.textContent = `Configuration unavailable: ${error.message}`;
    }
  }
}

function closeDrawers(restoreFocus = true) {
  ["inspector", "settings-drawer"].forEach((id) => { $(id).classList.remove("open"); $(id).setAttribute("aria-hidden", "true"); $(id).setAttribute("inert", ""); });
  $("toggle-inspector").setAttribute("aria-expanded", "false");
  document.body.classList.remove("drawer-open");
  if (restoreFocus && drawerReturnFocus?.isConnected) drawerReturnFocus.focus();
  if (restoreFocus) drawerReturnFocus = null;
}

function openDrawer(id: string) {
	const trigger = document.activeElement;
	closeUserMenu();
	document.body.classList.remove("mobile-sidebar-open");
	closeDrawers(false);
  drawerReturnFocus = trigger instanceof HTMLElement ? trigger : null;
  $(id).classList.add("open");
  $(id).setAttribute("aria-hidden", "false");
  $(id).removeAttribute("inert");
  document.body.classList.add("drawer-open");
  if (id === "inspector") $("toggle-inspector").setAttribute("aria-expanded", "true");
  if (id === "settings-drawer") loadConfigurationSources().catch(() => {});
  window.setTimeout(() => (id === "settings-drawer" ? $("organization") : $(`close-${id}`))?.focus(), 0);
}

function toggleHistory() {
	closeUserMenu();
	if (window.matchMedia("(max-width: 760px)").matches) document.body.classList.toggle("mobile-sidebar-open");
  else {
    document.body.classList.toggle("sidebar-collapsed");
    const expanded = !document.body.classList.contains("sidebar-collapsed");
    $("toggle-sidebar").setAttribute("aria-expanded", String(expanded));
    sessionStorage.setItem("oc.sidebar-collapsed", String(!expanded));
  }
}

const historyVisibilityMedia = window.matchMedia("(max-width: 760px)");

function syncHistoryAccessibility() {
  const visible = historyVisibilityMedia.matches
    ? document.body.classList.contains("mobile-sidebar-open")
    : !document.body.classList.contains("sidebar-collapsed");
  const sidebar = $("history-sidebar");
  sidebar.setAttribute("aria-hidden", String(!visible));
  if (visible) sidebar.removeAttribute("inert");
  else sidebar.setAttribute("inert", "");
  $("toggle-sidebar").setAttribute("aria-expanded", String(visible));
}

function formScope(): Scope {
  return { organization_id: $("organization").value.trim(), team_id: $("team").value.trim(), actor_id: $("actor").value.trim(), goal_id: null, task_id: null };
}

function scope(): Scope {
  return state.connected && state.authenticatedScope ? { ...state.authenticatedScope } : formScope();
}

function saveSettings() {
  updateContextChips();
}

function loadSettings() {
  fields.forEach((id) => sessionStorage.removeItem(`oc.${id}`));
}

function daemonSettings() {
  return {
    organization_id: $("organization").value.trim(), team_id: $("team").value.trim(), actor_id: $("actor").value.trim(),
    workspace_uri: $("workspace").value.trim(), default_model: $("model").value.trim(), default_title: $("title").value.trim(),
    telemetry_enabled: false, max_context_tokens: 32000
  };
}

function applyDaemonSettings(settings: DaemonSettings) {
  $("organization").value = settings.organization_id; $("team").value = settings.team_id; $("actor").value = settings.actor_id;
  $("workspace").value = settings.workspace_uri; $("model").value = settings.default_model; $("title").value = settings.default_title;
  switchComposerAccount(formScope());
  updateContextChips();
}

function protocolMajor(version: unknown) {
  const value = Number.parseInt(String(version).split(".")[0], 10); return Number.isSafeInteger(value) && value >= 0 ? value : null;
}

function negotiateCapabilities(manifest: JsonObject): Set<string> {
  if (!manifest || protocolMajor(manifest.protocol_version) !== 1) throw new Error(`Incompatible daemon protocol ${manifest?.protocol_version || "unknown"}`);
  const enabled = new Set<string>(
    (Array.isArray(manifest.capabilities) ? manifest.capabilities : [])
      .filter((item: JsonObject) => item?.enabled && protocolMajor(item.version) === 1)
      .map((item: JsonObject) => String(item.id)),
  );
  for (const required of ["scope.team", "session.persistence", "event.sse_replay"]) if (!enabled.has(required)) throw new Error(`Missing required capability ${required} v1`);
  return enabled;
}

async function api<T = unknown>(
  path: `/v1/${string}`,
  options: ApiRequestOptions = {},
): Promise<T> {
  const { allowDisconnected = false } = options;
  if (!allowDisconnected && !state.connected) throw new Error("daemon is not connected");
  return guardAccountResponse(requestJson<T>(path, options), state.generation, () => state.generation);
}

const openProviderSetup = providerSetup(api, (model) => {
  $("model").value = model;
  settingsDirty = false;
  saveSettings();
  updateContextChips();
  toast("Provider connected. You're ready to code.");
  $("prompt").focus();
});
document.getElementById("open-provider-setup")?.addEventListener("click", () => { void openProviderSetup(); });
let providerSetupPrompted = false;
async function suggestProviderSetup() {
  if (providerSetupPrompted) return;
  try {
    const catalog = await api<ProviderSetupCatalog>("/v1/provider-setup");
    providerSetupPrompted = true;
    if (catalog.needs_setup || !catalog.configured || !catalog.credentials_available) await openProviderSetup(catalog);
  } catch { /* Managed or older daemons do not expose local provider setup. */ }
}

async function bootstrapBrowserSession() {
  const meta = document.querySelector<HTMLMetaElement>('meta[name="s-code-bootstrap"]');
  meta?.remove();
  const fragment = new URLSearchParams(window.location.hash.replace(/^#/, ""));
  const token = fragment.get("s-code-bootstrap") || "";
  if (token) window.history.replaceState(null, "", `${window.location.pathname}${window.location.search}`);
  if (!token) return false;
  if (token.length > 128 || !/^[A-Za-z0-9]+$/.test(token)) {
    throw new Error("invalid local browser bootstrap");
  }
  const response = await fetch("/v1/auth/bootstrap", {
    method: "POST",
    cache: "no-store",
    credentials: "same-origin",
    referrerPolicy: "no-referrer",
    headers: { "content-type": "application/json", "x-s-code-csrf": "1" },
    body: JSON.stringify({ token }),
  });
  if (!response.ok) throw new Error(`automatic daemon authentication failed (${response.status})`);
  return true;
}

function connectionRecovery(label: string) {
  if (label.startsWith("Incompatible daemon protocol")) {
    return {
      title: "S-Code update required.",
      description: `This Web client supports protocol v1, but the local service reported ${label.replace("Incompatible daemon protocol ", "v")}. Update or reinstall S-Code so both components use the same version.`,
      action: "Check again",
    };
  }
  if (label.startsWith("Missing required capability")) {
    return {
      title: "Web and local service versions do not match.",
      description: `${label}. Update or reinstall S-Code, restart the local service, then check again.`,
      action: "Check again",
    };
  }
  if (label === "reconnecting") {
    return {
      title: "Connection interrupted.",
      description: "Your draft is safe. S-Code is retrying automatically; reconnect now if the local service has restarted.",
      action: "Reconnect now",
    };
  }
  return {
    title: "Local service is unavailable.",
    description: "Your draft is safe. Restart the S-Code local service, then retry or open Diagnostics.",
    action: "Retry",
  };
}

function renderClientPresence() {
  const menu = $("presence-menu");
  menu.hidden = !state.connected
    || !state.capabilities.has("client.presence.v1")
    || clientPresence.length === 0;
  if (menu.hidden) return;
  const currentSession = clientPresence.filter(
    (client) => client.session_id === state.session?.id,
  );
  const count = currentSession.length || clientPresence.length;
  $("presence-summary").textContent = `${count} client${count === 1 ? "" : "s"}`;
  const list = $("presence-list");
  list.replaceChildren();
  clientPresence.forEach((client) => {
    const row = document.createElement("article");
    row.className = "presence-client";
    row.setAttribute("role", "listitem");
    const title = document.createElement("strong");
    const own = client.client_id === currentPresenceClientId();
    title.textContent = `${own ? "You" : client.actor_id} · ${client.client_kind.toUpperCase()}${
      client.remote ? " · remote" : ""
    }`;
    const details = document.createElement("span");
    const session = state.sessions.find((candidate) => candidate.id === client.session_id);
    details.textContent = [
      client.focused ? "active" : "background",
      session?.title || (client.session_id ? `session ${client.session_id}` : "no session"),
      client.device_id ? `device ${client.device_id}` : null,
    ].filter(Boolean).join(" · ");
    row.append(title, details);
    if (client.remote && client.revocable) {
      const revoke = document.createElement("button");
      revoke.type = "button";
      revoke.textContent = own ? "Disconnect" : "Revoke";
      revoke.addEventListener("click", () => {
        revokeRemoteClient(client).catch((error) => toast(error.message));
      });
      row.append(revoke);
    }
    list.append(row);
  });
}

async function updateClientPresence() {
  if (!state.connected || !state.capabilities.has("client.presence.v1")) return;
  const clientId = currentPresenceClientId();
  if (!clientId) return;
  clientPresence = await api<ClientPresence[]>("/v1/client-presence", {
    method: "PUT",
    body: JSON.stringify({
      scope: scope(),
      client_id: clientId,
      client_kind: "web",
      session_id: state.session?.id || null,
      focused: document.visibilityState === "visible" && document.hasFocus(),
    }),
  });
  renderClientPresence();
}

async function revokeRemoteClient(client: ClientPresence) {
  const own = client.client_id === currentPresenceClientId();
  const confirmed = await requestAction({
    eyebrow: "Remote client",
    title: `${own ? "Disconnect" : "Revoke"} ${client.client_kind.toUpperCase()} client?`,
    description: "The short-lived Team Grant will be rejected immediately and after daemon restart. The registered device must obtain a new grant under Enterprise policy.",
    details: [
      `Actor: ${client.actor_id}`,
      `Device: ${client.device_id || "unknown"}`,
      `Client: ${client.client_id}`,
      `Presence expires: ${new Date(client.expires_at).toLocaleString()}`,
    ],
    confirm: own ? "Disconnect client" : "Revoke grant",
    danger: true,
  });
  if (!confirmed) return;
  clientPresence = await api<ClientPresence[]>(
    `/v1/client-presence/${encodeURIComponent(client.client_id)}`,
    {
      method: "DELETE",
      body: JSON.stringify({
        scope: scope(),
        revoke_remote_grant: true,
      }),
    },
  );
  renderClientPresence();
  if (own) setConnection(false, "Remote client grant revoked");
  else toast("Remote client grant revoked");
}

function setConnection(ok: boolean, label = ok ? "connected" : "offline") {
  if (!ok) { state.generation += 1; composerSubmissionPending = false; }
  state.connecting = false;
  state.connected = ok;
  $("connection").replaceChildren();
  const dot = document.createElement("span");
  $("connection").append(dot, document.createTextNode(label));
  $("connection").className = `status-dot ${ok ? "online" : "offline"}`;
  const arrow = document.createElement("span");
  arrow.setAttribute("aria-hidden", "true");
  arrow.textContent = "→";
  $("empty-connect").replaceChildren(document.createTextNode(ok ? "Change workspace " : "Connect workspace "), arrow);
  $("empty-guidance").textContent = ok
    ? "Describe the outcome. S-Code plans, edits, tests, and shows every change before you merge."
    : "Connect a workspace, then describe the outcome. S-Code plans, edits, tests, and shows every change.";
  const recovery = connectionRecovery(label);
  $("recovery-title").textContent = recovery.title;
  $("recovery-description").textContent = recovery.description;
  $("retry-connection").textContent = recovery.action;
  $("offline-recovery").hidden = ok || label === "connecting";
  announce(ok ? "Daemon connected" : `Daemon ${label}`);
  if (!ok) {
    closeActionDialog();
    if ($("context-picker-dialog").open) $("context-picker-dialog").close();
    contextPickerOptions = [];
    $("context-picker-results").replaceChildren();
    $("configuration-sources").replaceChildren();
    resetAccountLibrary();
    state.authenticatedScope = null;
    state.capabilities = new Set();
    state.sessions = [];
    clientPresence = [];
    $("presence-menu").hidden = true;
    clearSessionSelection(false, false);
    $("sessions").replaceChildren(document.createTextNode("Connect to load history"));
    $("sessions").className = "sessions empty";
    $("team-metrics").replaceChildren();
    [
      ["team-goals", "No active goals"],
      ["team-resources", "No owned resources"],
      ["team-queue", "No queued work"],
      ["durable-task-list", "No background tasks"],
      ["background-terminal-list", "No background terminals"],
      ["agent-run-list", "No agent runs"],
      ["team-budget-list", "No budgets configured"],
      ["team-approval-list", "No approval decisions"],
      ["team-outcome-list", "No verified outcomes"],
      ["outcome-summary", "No verified outcomes"],
      ["team-audit-list", "No audit activity"],
    ].forEach(([id, text]) => {
      $(id).replaceChildren(document.createTextNode(text));
      $(id).className = "team-queue empty";
    });
    $("capacity-summary").replaceChildren();
    $("activity").replaceChildren(document.createTextNode("No activity yet"));
    $("activity").className = "activity empty";
    setToolMessage("No result selected.");
  }
  updateContextChips();
}

async function connect() {
  saveSettings();
  if (state.reconnectTimer) { clearTimeout(state.reconnectTimer); state.reconnectTimer = null; }
  if (state.abort) state.abort.abort();
  setConnection(false, "connecting");
  state.connecting = true;
  state.abort = new AbortController();
  const signal = state.abort.signal;
  state.after = 0;
  const generation = state.generation;
  try {
    const capabilities = negotiateCapabilities(await api("/v1/capabilities", { allowDisconnected: true, signal }));
    if (generation !== state.generation) return;
    state.capabilities = capabilities;
    if (settingsDirty) await api("/v1/settings", { method: "PUT", body: JSON.stringify(daemonSettings()), allowDisconnected: true, signal });
    else {
      const settings = await api<DaemonSettings>("/v1/settings", { allowDisconnected: true, signal });
      if (generation !== state.generation) return;
      applyDaemonSettings(settings); saveSettings();
    }
    if (generation !== state.generation) return;
    settingsDirty = false; state.authenticatedScope = formScope(); switchComposerAccount(state.authenticatedScope); setConnection(true);
    await Promise.all([refreshSessions(), refreshTeam()]);
    if (!isCurrent(generation)) return;
    await restoreRoute();
    if (!isCurrent(generation)) return;
    await updateClientPresence();
    if (!isCurrent(generation)) return;
    subscribe(generation); closeDrawers(); $("prompt").focus(); toast("Workspace connected"); void suggestProviderSetup();
  }
  catch (error) { if (generation === state.generation) setConnection(false, error.message); }
}

async function refreshSessions() {
  const generation = state.generation;
  const s = scope(); const query = new URLSearchParams({ organization_id: s.organization_id, team_id: s.team_id, actor_id: s.actor_id });
  const sessions = await api<Session[]>(`/v1/sessions?${query}`);
  if (!state.connected || generation !== state.generation) return;
  state.sessions = sessions.filter((session) => ownsSession(session, state.authenticatedScope));
  renderSessions();
  if (!$("projects-view").hidden) {
    const route = parseRoute(window.location.pathname);
    renderProjects(route.type === "project" ? route.projectId : null).catch(() => {});
  }
  if (!$("artifacts-view").hidden) renderArtifactList();
}

function renderSessions() {
  const container = $("sessions"); container.replaceChildren(); container.className = "sessions";
  const query = $("session-search").value.trim().toLowerCase();
  const status = $("session-filter").value;
  const sessions = state.sessions.filter((session) => {
    if (status !== "all" && session.status !== status) return false;
    return !query || [
      session.title,
      workspaceName(session.workspace_uri),
      sessionMode(session),
      session.model,
    ].some((value) => String(value || "").toLowerCase().includes(query));
  });
  if (!sessions.length) {
    container.textContent = state.sessions.length ? "No matching tasks" : "No sessions yet";
    container.classList.add("empty");
    return;
  }
  sessionWindowStart = Math.min(
    sessionWindowStart,
    Math.max(0, sessions.length - SESSION_WINDOW_SIZE),
  );
  const visibleSessions = sessions.slice(
    sessionWindowStart,
    sessionWindowStart + SESSION_WINDOW_SIZE,
  );
  const sessionWindowControl = (label: string, nextStart: number) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "session-window-control";
    button.textContent = label;
    button.addEventListener("click", () => {
      sessionWindowStart = nextStart;
      renderSessions();
      container.querySelector<HTMLElement>(".session, .session-window-control")?.focus();
    });
    return button;
  };
  if (sessionWindowStart > 0) {
    container.append(sessionWindowControl(
      `Show previous ${Math.min(SESSION_WINDOW_SIZE, sessionWindowStart)} sessions`,
      Math.max(0, sessionWindowStart - SESSION_WINDOW_SIZE),
    ));
  }
  visibleSessions.forEach((session) => {
    const button = document.createElement("button"); button.className = `session${state.session?.id === session.id ? " active" : ""}`;
    const title = document.createElement("span"); title.textContent = session.title;
    const detail = document.createElement("small"); detail.textContent = `${sessionMode(session) === "work" ? `Work · ${workspaceName(session.workspace_uri)}` : "Chat"} · ${session.model}`;
    button.append(title, detail); button.addEventListener("click", () => selectSession(session)); container.append(button);
  });
  const remainingSessions = sessions.length - sessionWindowStart - visibleSessions.length;
  if (remainingSessions > 0) {
    container.append(sessionWindowControl(
      `Show next ${Math.min(SESSION_WINDOW_SIZE, remainingSessions)} sessions`,
      sessionWindowStart + SESSION_WINDOW_SIZE,
    ));
  }
}

async function createSession() {
  const generation = state.generation;
  try {
    saveSettings();
    if (!$("model").value.trim()) throw new Error("Choose a model to start a conversation");
    const session = await api<Session>("/v1/sessions", { method: "POST", body: JSON.stringify({ scope: scope(), mode: newConversationMode, workspace_uri: newSessionWorkspace(newConversationMode, $("work-directory").value), title: $("title").value.trim() || (newConversationMode === "chat" ? "New chat" : "New work"), model: $("model").value.trim() }) });
    if (!isCurrent(generation)) return null;
    transferNewComposerDraft(session);
    if (state.permissionMode !== "manual") {
      await api(`/v1/sessions/${encodeURIComponent(session.id)}/preferences`, {
        method: "PATCH",
        body: JSON.stringify({ scope: scope(), permission_mode: state.permissionMode }),
      });
    }
    await refreshSessions();
    if (!isCurrent(generation)) return null;
    await selectSession(session); closeDrawers(); return session;
  } catch (error) { if (isCurrent(generation)) { addActivity("session.error", { error: error.message }); openDrawer("settings-drawer"); } return null; }
}

async function selectSession(session: Session, { updateRoute = true }: ViewOptions = {}) {
  if (!state.connected || !ownsSession(session, state.authenticatedScope)) return;
  saveComposerDraft();
  closeMentionMenu();
  transcriptFollowing = true;
  $("jump-latest").hidden = true;
  transcriptProjection = selectTranscriptSession(transcriptProjection, session.id);
  loadedTranscriptSnapshot = null;
  $("load-earlier").hidden = true;
  state.pendingInputs = []; renderPendingInputs(); state.session = session; state.turn = null; setTurnRunning(false); $("undo-turn").disabled = true; $("show-context").disabled = !state.capabilities.has("context.explain"); $("review-session").disabled = !state.capabilities.has("review.read_only"); $("show-checkpoints").disabled = false; $("fork-session").disabled = false; $("show-branches").disabled = false; $("export-session").disabled = false; $("rename-session").disabled = false; $("rename-assistant").disabled = false; $("cancel-session").disabled = !["active", "archived"].includes(session.status); $("cancel-session").textContent = session.status === "archived" ? "Restore" : "Archive"; $("cancel-session").classList.toggle("danger", session.status !== "archived"); $("delete-session").disabled = false; $("quick-diff").disabled = false; $("session-title").textContent = session.title; $("session-meta").textContent = sessionDescription(session);
  updateContextChips();
  restoreComposerDraft();
  document.body.classList.remove("mobile-sidebar-open");
  showWorkspace({ updateRoute });
  const generation = state.generation;
  const sessionId = session.id;
  const query = catalogQuery();
  const preferencesRequest = state.capabilities.has("composer.permission_picker.v1")
    ? api<SessionPreferences>(`/v1/sessions/${encodeURIComponent(session.id)}/preferences?${query}`)
    : Promise.resolve<SessionPreferences>({
      session_id: session.id,
      permission_mode: "manual",
      assistant_alias: "S-Code",
      source: "compatibility_default",
      locked_reason: null,
      updated_at: session.updated_at || new Date(0).toISOString(),
    });
  const sideConversationsRequest = state.capabilities.has("session.side_conversation.v1")
    ? api<SideConversation[]>(`/v1/side-conversations?${query}`).catch(() => [])
    : Promise.resolve<SideConversation[]>([]);
  const goalRequest = hasWorkspace(session) && state.capabilities.has("session.goal.v1")
    ? api<SessionGoal | null>(`/v1/sessions/${encodeURIComponent(session.id)}/goal?${query}`)
    : Promise.resolve<SessionGoal | null>(null);
  const [, , preferences, sideConversations, goal] = await Promise.all([
    refreshSessions(),
    loadMessages(),
    preferencesRequest,
    sideConversationsRequest,
    goalRequest,
  ]);
  if (isCurrent(generation) && state.session?.id === sessionId) {
    state.permissionMode = preferences.permission_mode;
    applyAssistantAlias(preferences.assistant_alias);
    state.goal = goal;
    state.sideConversation = sideConversations.find(
      (conversation) => conversation.session_id === sessionId
        && conversation.status === "active"
    ) || null;
    updateContextChips();
    renderSessionGoal();
    renderSideConversationState();
  }
  updateClientPresence().catch((error) =>
    addActivity("client.presence.error", { error: error.message })
  );
  $("prompt").focus();
}

function clearSessionSelection(refresh = true, updateRoute = true) {
  newConversationMode = "chat";
  $("work-directory").value = "";
  saveComposerDraft();
  closeMentionMenu();
  transcriptFollowing = true;
  $("jump-latest").hidden = true;
  transcriptProjection = selectTranscriptSession(transcriptProjection, null);
  loadedTranscriptSnapshot = null;
  $("load-earlier").hidden = true;
  state.session = null; state.goal = null; renderSessionGoal(); state.sideConversation = null; renderSideConversationState(); state.turn = null; state.pendingInputs = []; renderPendingInputs(); setTurnRunning(false); state.approvals.clear(); state.questions.clear(); applyAssistantAlias("S-Code"); $("session-title").textContent = "New task"; $("session-meta").textContent = "Ready when you are"; $("messages").replaceChildren(); $("approvals").replaceChildren(); $("rename-session").disabled = true; $("rename-assistant").disabled = true; $("show-context").disabled = true; $("review-session").disabled = true; $("show-checkpoints").disabled = true; $("fork-session").disabled = true; $("show-branches").disabled = true; $("export-session").disabled = true; $("cancel-session").disabled = true; $("cancel-session").textContent = "Archive"; $("cancel-session").classList.add("danger"); $("delete-session").disabled = true; $("undo-turn").disabled = true; $("quick-diff").disabled = true; $("turn-state").textContent = "idle"; updateConversationState(false); if (refresh && state.connected) refreshSessions().catch(() => {}); $("prompt").focus();
  state.toolSteps.clear();
  state.itemsById.clear();
  restoreAccountPermission();
  updateContextChips();
  restoreComposerDraft();
  showWorkspace({ updateRoute });
  updateClientPresence().catch(() => {});
}

function setTurnRunning(running: boolean) {
  state.turnRunning = running;
  updateContextChips();
  updateSendAction();
  if (running) {
    announce("Task running. Type another message to queue it, or use the stop button with an empty prompt.");
  }
}

function updateSendAction() {
  const hasAttachments = state.draftFiles.length > 0;
  const hasInput = Boolean($("prompt").value.trim()) || hasAttachments;
  const queues = state.turnRunning && hasInput;
  $("send-turn").classList.toggle("is-stop", state.turnRunning && !queues);
  $("send-turn").textContent = queues ? "＋" : state.turnRunning ? "■" : "↑";
  $("send-turn").disabled = composerSubmissionPending || (state.turnRunning && hasAttachments);
  $("send-turn").setAttribute(
    "aria-label",
    state.turnRunning && hasAttachments
      ? "Remove attachments before queuing"
      : queues
        ? "Queue message"
        : state.turnRunning
          ? "Stop turn"
          : "Run turn",
  );
  $("steer-turn").hidden = !queues
    || hasAttachments
    || !state.capabilities.has("turn.input_queue.v1");
}

async function withComposerSubmission(action: () => Promise<void>) {
  if (composerSubmissionPending) return;
  const generation = state.generation;
  composerSubmissionPending = true;
  updateSendAction();
  try {
    await action();
  } finally {
    if (generation === state.generation) {
      composerSubmissionPending = false;
      updateSendAction();
    }
  }
}

function renderPendingInputs() {
  const target = $("pending-inputs");
  target.replaceChildren();
  target.hidden = state.pendingInputs.length === 0;
  state.pendingInputs.forEach((item, index) => {
    const row = document.createElement("div");
    row.className = "pending-input";
    const label = document.createElement("span");
    const mode = item.mode === "steer" ? "Steering" : index === 0 ? "Next" : `Queued ${index + 1}`;
    label.textContent = `${mode} · ${typeof item.content === "string" ? item.content : "Structured input"}`;
    const remove = document.createElement("button");
    remove.type = "button";
    remove.setAttribute("aria-label", `Remove queued message ${index + 1}`);
    remove.textContent = "×";
    remove.addEventListener("click", async () => {
      remove.disabled = true;
      try {
        await api(`/v1/turn-inputs/${encodeURIComponent(item.id)}`, {
          method: "DELETE",
          body: JSON.stringify({ scope: scope() }),
        });
        state.pendingInputs = state.pendingInputs.filter((candidate) => candidate.id !== item.id);
        renderPendingInputs();
      } catch (error) {
        remove.disabled = false;
        addActivity("turn.input.cancel.error", { error: error.message });
      }
    });
    row.append(label, remove);
    target.append(row);
  });
}

async function refreshPendingInputs() {
  if (!state.session) return;
  const selected = state.session.id;
  const s = scope();
  const query = new URLSearchParams({
    organization_id: s.organization_id,
    team_id: s.team_id,
    actor_id: s.actor_id,
  });
  const inputs = await api<TurnInput[]>(`/v1/sessions/${encodeURIComponent(selected)}/inputs?${query}`);
  if (state.session?.id !== selected) return;
  state.pendingInputs = inputs;
  renderPendingInputs();
}

async function submitTurnInput(content: string, mode: TurnInput["mode"]) {
  if (!state.session || !state.turn) throw new Error("No running turn is available");
  const input = await api<TurnInput>(`/v1/sessions/${encodeURIComponent(state.session.id)}/inputs`, {
    method: "POST",
    body: JSON.stringify({
      scope: scope(),
      target_turn_id: state.turn,
      mode,
      content,
      idempotency_key: `web-${globalThis.crypto?.randomUUID?.() || `${Date.now()}-${Math.random().toString(16).slice(2)}`}`,
    }),
  });
  if (!state.pendingInputs.some((candidate) => candidate.id === input.id)) {
    state.pendingInputs.push(input);
  }
  renderPendingInputs();
  $("prompt").value = "";
  sessionStorage.removeItem(composerTextDraftKey());
  resizePrompt();
  updateSendAction();
  toast(mode === "steer" ? "Steering the current turn" : "Message durably queued");
  return input;
}

async function cancelActiveTurn() {
  if (!state.turn) return;
  const generation = state.generation;
  $("turn-state").textContent = "cancelling";
  try {
    await api(`/v1/turns/${encodeURIComponent(state.turn)}/cancel`, { method: "POST", body: JSON.stringify({ scope: scope() }) });
  } catch (error) {
    if (isCurrent(generation)) { setTurnRunning(false); addActivity("turn.cancel.error", { error: error.message }); }
  }
}

async function closeSession(deleting: boolean) {
  if (!state.session) return;
  if (!deleting && state.session.status === "archived") {
    const generation = state.generation;
    try {
      const restored = await api<Session>(`/v1/sessions/${encodeURIComponent(state.session.id)}`, {
        method: "PATCH",
        body: JSON.stringify({ scope: scope(), status: "active" }),
      });
      if (!isCurrent(generation)) return;
      await selectSession(restored);
      toast("Session restored");
    } catch (error) {
      addActivity("session.lifecycle.error", { error: error.message });
    }
    return;
  }
  const generation = state.generation;
  let impact: SessionImpactPreview | null = null;
  if (deleting) {
    const s = scope();
    const query = new URLSearchParams({
      organization_id: s.organization_id,
      team_id: s.team_id,
      actor_id: s.actor_id,
    });
    try {
      impact = await api<SessionImpactPreview>(
        `/v1/sessions/${encodeURIComponent(state.session.id)}/impact?${query}`,
      );
    } catch (error) {
      if (isCurrent(generation)) setToolMessage(`Unable to preview delete impact: ${error.message}`, true);
      return;
    }
  }
  const values = await requestAction({
    eyebrow: "Session", title: deleting ? "Delete this session?" : "Archive this session?",
    description: deleting ? "Review the exact retained and affected records before deleting." : "Active turns stop and the session remains available in history.",
    details: impact ? [
      `${impact.turn_count} turns · ${impact.message_count} messages`,
      `${impact.attachment_count} attachments · ${impact.artifact_count} artifacts`,
      `${impact.branch_count} linked branches remain independent`,
      impact.active_turn_count
        ? `${impact.active_turn_count} active turns will stop`
        : "No active turns will be stopped",
      impact.audit_evidence_preserved
        ? "Audit evidence is preserved"
        : "Audit evidence is removed",
    ] : [],
    confirm: deleting ? "Delete session" : "Archive session", danger: true,
  });
  if (!values) return;
  const id = encodeURIComponent(state.session.id); const path: `/v1/${string}` = deleting ? `/v1/sessions/${id}` : `/v1/sessions/${id}/cancel`;
  try {
    const closed = await api<Session>(path, { method: deleting ? "DELETE" : "POST", body: JSON.stringify({ scope: scope() }) });
    if (!isCurrent(generation)) return;
    addActivity(deleting ? "session.deleted" : "session.cancelled", { session_id: closed.id });
    clearSessionSelection();
    await refreshSessions();
    toast(deleting ? "Session deleted" : "Session archived");
  } catch (error) { addActivity("session.lifecycle.error", { error: error.message }); }
}

async function renameSession() {
  if (!state.session) return;
  const generation = state.generation;
  const values = await requestAction({
    eyebrow: "Session",
    title: "Rename session",
    description: "Use a short title that makes this task easy to find.",
    confirm: "Rename",
    fields: [{
      name: "title",
      label: "Session title",
      value: state.session.title,
      required: true,
      maxlength: 64,
    }],
  });
  if (!values?.title) return;
  try {
    const updated = await api<Session>(`/v1/sessions/${encodeURIComponent(state.session.id)}`, {
      method: "PATCH",
      body: JSON.stringify({ scope: scope(), title: values.title }),
    });
    if (!isCurrent(generation)) return;
    state.session = updated;
    $("session-title").textContent = updated.title;
    $("session-meta").textContent = sessionDescription(updated);
    await refreshSessions();
    toast("Session renamed");
  } catch (error) {
    addActivity("session.lifecycle.error", { error: error.message });
  }
}

async function renameAssistant() {
  if (!state.session) return;
  const sessionId = state.session.id;
  const values = await requestAction({
    eyebrow: "Session appearance",
    title: "Rename the assistant",
    description: "This changes the assistant heading in this Session. It does not rename the Session or rewrite history.",
    confirm: "Save name",
    fields: [{
      name: "alias",
      label: "Assistant name",
      value: state.assistantAlias,
      required: true,
      maxlength: 40,
    }],
  });
  if (!values?.alias) return;
  const preferences = await api<SessionPreferences>(
    `/v1/sessions/${encodeURIComponent(sessionId)}/preferences`,
    {
      method: "PATCH",
      body: JSON.stringify({
        scope: scope(),
        assistant_alias: values.alias.trim(),
      }),
    },
  );
  if (state.session?.id !== sessionId) return;
  applyAssistantAlias(preferences.assistant_alias);
  toast(`Assistant name set to ${preferences.assistant_alias}`);
}

async function forkSession() {
  if (!state.session) return;
  const generation = state.generation;
  const source = state.session;
  const s = scope();
  const query = new URLSearchParams({
    organization_id: s.organization_id,
    team_id: s.team_id,
    actor_id: s.actor_id,
  });
  let turns: Turn[];
  try {
    turns = (await api<Turn[]>(`/v1/sessions/${encodeURIComponent(source.id)}/turns?${query}`))
      .filter((turn) => ["completed", "failed", "cancelled"].includes(turn.status));
  } catch (error) {
    toast(error.message);
    return;
  }
  if (!isCurrent(generation) || state.session?.id !== source.id) return;
  const fields: ActionField[] = [{
    name: "title",
    label: "Fork title",
    value: `Fork of ${source.title}`.slice(0, 64),
    required: true,
    maxlength: 64,
  }];
  if (turns.length) {
    const latestTurn = turns.at(-1);
    if (!latestTurn) return;
    fields.push({
      name: "source_turn_id",
      label: "Branch after",
      value: latestTurn.id,
      options: turns.map((turn, index) => [
        turn.id,
        `${index + 1}. ${turn.status} · ${new Date(turn.started_at).toLocaleString()}`,
      ]),
    });
  }
  const values = await requestAction({
    eyebrow: "Session",
    title: "Fork conversation",
    description: "Create an independent branch from a completed checkpoint. Future turns will not affect the source.",
    confirm: "Create fork",
    fields,
  });
  if (!values?.title) return;
  try {
    const fork = await api<Session>(`/v1/sessions/${encodeURIComponent(source.id)}/fork`, {
      method: "POST",
      body: JSON.stringify({
        scope: scope(),
        title: values.title,
        source_turn_id: values.source_turn_id || null,
      }),
    });
    if (!isCurrent(generation)) return;
    await refreshSessions();
    await selectSession(fork);
    toast("Conversation forked");
  } catch (error) {
    addActivity("session.fork.error", { error: error.message });
  }
}

function renderSideConversationState() {
  const active = state.sideConversation?.status === "active"
    ? state.sideConversation
    : null;
  $("side-conversation-banner").hidden = !active;
  if (active) {
    $("side-conversation-banner").querySelector("span")!.textContent =
      `Temporary Fork of ${active.source_session_id}. Promote it to keep it as a regular Session.`;
  }
}

async function createSideConversation() {
  if (!state.session || state.turnRunning) return;
  const source = state.session;
  const generation = state.generation;
  const values = await requestAction({
    eyebrow: "Side conversation",
    title: "Ask without changing this Session",
    description: "Creates a temporary Fork at the latest completed Turn. The answer stays separate until you promote it.",
    confirm: "Start side conversation",
    fields: [{
      name: "prompt",
      label: "Question",
      multiline: true,
      maxlength: 8_000,
      placeholder: "Explain whether this alternative is safer without changing any files.",
      required: true,
    }],
  });
  if (!values?.prompt || !isCurrent(generation) || !ownsSession(source, state.authenticatedScope)) return;
  const result = await api<SideConversationStart>(
    `/v1/sessions/${encodeURIComponent(source.id)}/side-conversations`,
    {
      method: "POST",
      body: JSON.stringify({
        scope: source.scope,
        source_turn_id: null,
        prompt: values.prompt.trim(),
      }),
    },
  );
  await refreshSessions();
  if (!isCurrent(generation)) return;
  await selectSession(result.session);
  if (!isCurrent(generation)) return;
  state.sideConversation = result.conversation;
  renderSideConversationState();
  toast("Side conversation started");
}

async function promoteSideConversation() {
  const side = state.sideConversation;
  if (!side || side.status !== "active") return;
  const values = await requestAction({
    eyebrow: "Side conversation",
    title: "Promote to a regular Session?",
    description: "The Fork becomes normal project history and is no longer treated as temporary.",
    confirm: "Promote Session",
  });
  if (!values) return;
  await api(`/v1/side-conversations/${encodeURIComponent(side.id)}/promote`, {
    method: "POST",
    body: JSON.stringify({ scope: side.scope }),
  });
  state.sideConversation = null;
  renderSideConversationState();
  await refreshSessions();
  toast("Side conversation promoted");
}

async function closeSideConversation() {
  const side = state.sideConversation;
  if (!side || side.status !== "active") return;
  const source = state.sessions.find((session) => session.id === side.source_session_id);
  const values = await requestAction({
    eyebrow: "Side conversation",
    title: "Close this temporary Fork?",
    description: "The temporary Session is archived. The source Session and its Transcript are unchanged.",
    confirm: "Close side conversation",
    danger: true,
  });
  if (!values) return;
  await api(`/v1/side-conversations/${encodeURIComponent(side.id)}`, {
    method: "DELETE",
    body: JSON.stringify({ scope: side.scope }),
  });
  state.sideConversation = null;
  renderSideConversationState();
  await refreshSessions();
  if (source) await selectSession(source);
  else clearSessionSelection();
  toast("Side conversation closed");
}

function renderMessageAttachments(
  item: HTMLElement,
  attachments: Array<Pick<AttachmentMetadata, "file_name" | "media_type" | "byte_length">>,
) {
  item.querySelector(".message-attachments")?.remove();
  if (!attachments.length) return;
  const list = document.createElement("div");
  list.className = "message-attachments";
  list.setAttribute("aria-label", "Message attachments");
  attachments.forEach((attachment) => {
    const chip = document.createElement("span");
    chip.className = "message-attachment";
    const name = document.createElement("strong");
    name.textContent = attachment.file_name;
    const detail = document.createElement("span");
    detail.textContent = `${attachment.media_type} · ${formatBytes(Number(attachment.byte_length))}`;
    chip.append(name, detail);
    list.append(chip);
  });
  item.append(list);
}

function renderMessage(
  role: string,
  content: unknown,
  identity: { itemId?: string | null; turnId?: string | null } = {},
  attachments: Array<Pick<AttachmentMetadata, "file_name" | "media_type" | "byte_length">> = [],
) {
  const item = document.createElement("article");
  item.className = `message ${role}`;
  if (identity.itemId) item.dataset.itemId = identity.itemId;
  if (identity.turnId) item.dataset.turnId = identity.turnId;
  item.setAttribute("aria-label", role === "assistant" ? `${state.assistantAlias} response` : "Your message");
  if (role === "assistant") {
    const label = document.createElement("div");
    label.className = "message-label";
    label.textContent = state.assistantAlias;
    item.append(label);
  }
  renderMessageContent(item, content);
  renderMessageAttachments(item, attachments);
  if (role === "user" && identity.turnId && typeof content === "string") {
    const turnId = identity.turnId;
    const actions = document.createElement("div");
    actions.className = "message-actions";
    const retry = document.createElement("button");
    retry.type = "button";
    retry.textContent = "Edit and retry";
    retry.setAttribute("aria-label", "Edit this message and retry from here");
    retry.addEventListener("click", () => editAndRetry(turnId, content));
    actions.append(retry);
    item.append(actions);
  }
  $("messages").append(item);
  if (identity.itemId) state.itemsById.set(identity.itemId, item);
  updateConversationState(true);
  return item;
}

async function editAndRetry(turnId: string, originalContent: string) {
  if (state.turnRunning) {
    toast("Stop the running Turn before retrying an earlier message.");
    return;
  }
  const values = await requestAction({
    eyebrow: "Branch and retry",
    title: "Edit message and retry?",
    description: "S-Code creates a new branch before this Turn. The original Session and its evidence remain unchanged.",
    confirm: "Retry in new branch",
    fields: [{
      name: "content",
      label: "Message",
      value: originalContent,
      multiline: true,
      required: true,
      maxlength: 200_000,
    }],
  });
  if (!values?.content.trim()) return;
  const generation = state.generation;
  try {
    const result = await api<RetryTurnResult>(
      `/v1/turns/${encodeURIComponent(turnId)}/retry`,
      {
        method: "POST",
        body: JSON.stringify({
          scope: scope(),
          content: values.content.trim(),
        }),
      },
    );
    if (!isCurrent(generation)) return;
    await selectSession(result.session);
    if (!isCurrent(generation)) return;
    state.turn = result.turn.id;
    setTurnRunning(true);
    toast("Retry started in a new branch");
  } catch (error) {
    if (isCurrent(generation)) addActivity("turn.retry.error", { error: error.message });
  }
}

function renderPlan(
  itemId: string,
  turnId: string | null | undefined,
  title: string | null | undefined,
  steps: Array<{ text: string; status: string }> = [],
  status = "streaming",
) {
  let item = state.itemsById.get(itemId);
  if (!item?.classList.contains("plan-card")) {
    item = document.createElement("article");
    item.className = "plan-card";
    item.dataset.itemId = itemId;
    if (turnId) item.dataset.turnId = turnId;
    item.setAttribute("aria-label", "Execution plan");
    $("messages").append(item);
    state.itemsById.set(itemId, item);
  }
  item.classList.toggle("complete", status === "completed" || (steps.length > 0 && steps.every((step) => step.status === "completed")));
  item.replaceChildren();
  const heading = document.createElement("div");
  heading.className = "plan-heading";
  const label = document.createElement("strong");
  label.textContent = title || "Plan";
  const count = document.createElement("small");
  count.textContent = `${steps.filter((step) => step.status === "completed").length}/${steps.length} complete`;
  heading.append(label, count);
  const list = document.createElement("ol");
  steps.forEach((step) => {
    const row = document.createElement("li");
    row.className = `plan-step ${step.status}`;
    const marker = document.createElement("span");
    marker.setAttribute("aria-hidden", "true");
    marker.textContent = step.status === "completed" ? "✓" : step.status === "in_progress" ? "●" : "○";
    const text = document.createElement("span");
    text.textContent = step.text;
    row.append(marker, text);
    list.append(row);
  });
  item.append(heading, list);
  updateConversationState(true);
  return item;
}

function renderTranscriptNotice(
  itemId: string,
  turnId: string | null | undefined,
  kind: string,
  title: string,
  detail: string,
) {
  let item = state.itemsById.get(itemId);
  if (!item?.classList.contains("transcript-notice")) {
    item = document.createElement("article");
    item.className = "transcript-notice";
    item.dataset.itemId = itemId;
    if (turnId) item.dataset.turnId = turnId;
    $("messages").append(item);
    state.itemsById.set(itemId, item);
  }
  item.className = `transcript-notice ${kind.replaceAll("_", "-")}`;
  item.replaceChildren();
  const label = document.createElement("strong");
  label.textContent = title;
  const copy = document.createElement("span");
  copy.textContent = detail;
  item.append(label, copy);
  if (kind === "warning") item.setAttribute("role", "alert");
  else item.setAttribute("role", "status");
  updateConversationState(true);
  return item;
}

function renderArtifact(
  itemId: string,
  turnId: string | null | undefined,
  artifactId: string,
  title: string,
  mediaType: string,
) {
  let item = state.itemsById.get(itemId);
  if (!item?.classList.contains("artifact-card")) {
    item = document.createElement("article");
    item.className = "artifact-card";
    item.dataset.itemId = itemId;
    if (turnId) item.dataset.turnId = turnId;
    $("messages").append(item);
    state.itemsById.set(itemId, item);
  }
  item.replaceChildren();
  const copy = document.createElement("div");
  const label = document.createElement("strong");
  label.textContent = title || "Artifact";
  const type = document.createElement("small");
  type.textContent = mediaType || "content";
  copy.append(label, type);
  const open = document.createElement("button");
  open.type = "button";
  open.textContent = "Open";
  open.setAttribute("aria-label", `Open artifact ${title || artifactId}`);
  open.addEventListener("click", () => openArtifact(artifactId));
  item.append(copy, open);
  updateConversationState(true);
  return item;
}

function renderReviewReport(target: HTMLElement, value: unknown) {
  if (!isJsonObject(value) || !Array.isArray(value.findings) || typeof value.target !== "string") {
    throw new Error("Review report has an invalid shape");
  }
  const report = value as unknown as ReviewReport;
  const summary = document.createElement("div");
  summary.className = "review-summary";
  const title = document.createElement("strong");
  title.textContent = `${report.findings.length} actionable finding${report.findings.length === 1 ? "" : "s"}`;
  const scope = document.createElement("small");
  scope.textContent = report.target;
  summary.append(title, scope);
  target.append(summary);
  if (!report.findings.length) {
    const empty = document.createElement("p");
    empty.className = "review-empty";
    empty.textContent = "No actionable findings were reported.";
    target.append(empty);
    return;
  }
  report.findings.forEach((finding) => {
    const card = document.createElement("article");
    card.className = `review-finding severity-${finding.severity}`;
    card.dataset.findingId = finding.id;
    const heading = document.createElement("div");
    heading.className = "review-finding-heading";
    const identity = document.createElement("div");
    const severity = document.createElement("span");
    severity.className = "review-severity";
    severity.textContent = finding.severity;
    const findingTitle = document.createElement("strong");
    findingTitle.textContent = finding.title;
    identity.append(severity, findingTitle);
    const location = `${finding.location.path}:${finding.location.line_start}${
      finding.location.line_end === finding.location.line_start
        ? ""
        : `-${finding.location.line_end}`
    }`;
    const locate = document.createElement("button");
    locate.type = "button";
    locate.className = "review-location";
    locate.textContent = location;
    locate.setAttribute("aria-label", `Copy review location ${location}`);
    locate.addEventListener("click", () => copyText(location, "Review location copied"));
    heading.append(identity, locate);
    const description = document.createElement("p");
    description.textContent = finding.description;
    const evidence = document.createElement("p");
    evidence.className = "review-evidence";
    const evidenceLabel = document.createElement("strong");
    evidenceLabel.textContent = "Evidence";
    evidence.append(evidenceLabel, document.createTextNode(` · ${finding.evidence}`));
    card.append(heading, description, evidence);
    if (finding.suggested_fix) {
      const fix = document.createElement("p");
      fix.className = "review-fix";
      const fixLabel = document.createElement("strong");
      fixLabel.textContent = "Suggested fix";
      fix.append(fixLabel, document.createTextNode(` · ${finding.suggested_fix}`));
      card.append(fix);
    }
    target.append(card);
  });
}

async function openArtifact(artifactId: string) {
  if (!artifactId) return;
  const generation = state.generation;
  const s = scope();
  const query = new URLSearchParams({
    organization_id: s.organization_id,
    team_id: s.team_id,
    actor_id: s.actor_id,
  });
  try {
    const artifact = await api<Artifact>(`/v1/artifacts/${encodeURIComponent(artifactId)}?${query}`);
    if (!isCurrent(generation)) return;
    openDrawer("inspector");
    const target = $("tool-result");
    target.className = "tool-result artifact-detail";
    target.replaceChildren();
    const heading = document.createElement("div");
    heading.className = "artifact-detail-heading";
    const title = document.createElement("strong");
    title.textContent = artifact.metadata.title;
    const type = document.createElement("span");
    type.textContent = artifact.metadata.media_type;
    heading.append(title, type);
    target.append(heading);
    if (artifact.metadata.media_type === "application/vnd.s-code.review+json") {
      renderReviewReport(target, artifact.content);
    } else if (artifact.metadata.media_type === "text/markdown" && typeof artifact.content === "string") {
      const body = document.createElement("div");
      body.className = "message-body";
      appendMarkdownBlocks(body, artifact.content);
      target.append(body);
    } else {
      const content = typeof artifact.content === "string"
        ? artifact.content
        : JSON.stringify(artifact.content, null, 2);
      target.append(createCodeBlock(
        content,
        artifact.metadata.media_type === "application/json" ? "json" : "text",
      ));
    }
    announce(`Opened artifact ${artifact.metadata.title}`);
  } catch (error) {
    if (isCurrent(generation)) setToolMessage(error.message, true);
  }
}

async function loadMessages() {
  if (!state.session) return;
  const generation = state.generation; const sessionId = state.session.id;
  const s = scope(); const query = new URLSearchParams({ organization_id: s.organization_id, team_id: s.team_id, actor_id: s.actor_id });
  if (state.capabilities.has("transcript.snapshot")) {
    const snapshot = parseTranscriptSnapshot(await api(`/v1/sessions/${encodeURIComponent(state.session.id)}/snapshot?${query}`));
    if (!isCurrent(generation) || state.session?.id !== sessionId) return;
    renderTranscriptSnapshot(snapshot);
    return;
  }
  const messages = await api<Message[]>(`/v1/sessions/${encodeURIComponent(state.session.id)}/messages?${query}`);
  if (!isCurrent(generation) || state.session?.id !== sessionId) return;
  $("messages").replaceChildren();
  state.itemsById.clear();
  messages.forEach((message) => renderMessage(message.role, message.content, { itemId: message.id, turnId: message.turn_id }));
  updateConversationState(messages.length > 0);
}

function renderTranscriptSnapshot(
  snapshot: TranscriptSnapshot,
  mergeOlder = false,
  preserveWindow = false,
) {
  if (state.session && snapshot.session.id !== state.session.id) {
    throw new Error("transcript snapshot does not match the selected session");
  }
  if (!mergeOlder && isTranscriptSnapshotStale(transcriptProjection, snapshot)) {
    return;
  }
  if (!mergeOlder && state.session) {
    state.session = { ...state.session, ...snapshot.session };
    updateContextChips();
  }
  const preserveNewerLiveUsage =
    mergeOlder && snapshot.snapshot_revision < transcriptProjection.snapshotRevision;
  if (
    mergeOlder
    && loadedTranscriptSnapshot
    && loadedTranscriptSnapshot.session.id === snapshot.session.id
  ) {
    const items = new Map(
      [...snapshot.items, ...loadedTranscriptSnapshot.items]
        .map((item) => [item.id, item] as const),
    );
    snapshot = {
      ...loadedTranscriptSnapshot,
      items: [...items.values()].sort((left, right) =>
        left.created_at.localeCompare(right.created_at) || left.id.localeCompare(right.id)
      ),
      item_count: Math.max(snapshot.item_count, loadedTranscriptSnapshot.item_count),
      next_cursor: snapshot.next_cursor,
      cursor: Math.max(snapshot.cursor, loadedTranscriptSnapshot.cursor),
      snapshot_revision: Math.max(
        snapshot.snapshot_revision,
        loadedTranscriptSnapshot.snapshot_revision,
      ),
    };
  }
  if (!preserveWindow) {
    transcriptWindowStart = mergeOlder
      ? 0
      : Math.max(0, snapshot.items.length - TRANSCRIPT_WINDOW_SIZE);
  }
  transcriptWindowStart = Math.min(
    transcriptWindowStart,
    Math.max(0, snapshot.items.length - TRANSCRIPT_WINDOW_SIZE),
  );
  loadedTranscriptSnapshot = snapshot;
  if (!preserveNewerLiveUsage) {
    state.usage = snapshot.usage || {
      input_tokens: 0,
      output_tokens: 0,
      total_tokens: 0,
      model_calls: 0,
      tool_calls: 0,
      turns: 0,
    };
  }
  state.usageTurns = new Set(
    snapshot.items
      .filter((item) => item.kind === "usage" && item.content.type === "usage")
      .map((item) => item.turn_id),
  );
  $("load-earlier").hidden = !snapshot.next_cursor;
  $("load-earlier").textContent = snapshot.next_cursor
    ? `Load earlier · ${snapshot.items.length}/${snapshot.item_count}`
    : "All history loaded";
  transcriptProjection = applyTranscriptSnapshot(transcriptProjection, snapshot);
  $("messages").replaceChildren();
  $("approvals").replaceChildren();
  state.itemsById.clear();
  state.toolSteps.clear();
  state.approvals.clear();
  state.questions.clear();
  const workNotice = workTransitionNotice(state.session);
  if (workNotice) renderTranscriptNotice(workNotice.id, undefined, "work_started", "Work started", workNotice.detail);
  state.after = Math.max(state.after, Number(snapshot.cursor || 0));
  const visibleItems = snapshot.items.slice(
    transcriptWindowStart,
    transcriptWindowStart + TRANSCRIPT_WINDOW_SIZE,
  );
  const transcriptWindowControl = (
    label: string,
    nextStart: number,
    position: "before" | "after",
  ) => {
    const control = document.createElement("button");
    control.type = "button";
    control.className = "transcript-window-control";
    control.dataset.windowPosition = position;
    control.textContent = label;
    control.addEventListener("click", () => {
      if (!loadedTranscriptSnapshot) return;
      transcriptWindowStart = nextStart;
      renderTranscriptSnapshot(loadedTranscriptSnapshot, false, true);
      document
        .querySelector<HTMLElement>(`.transcript-window-control[data-window-position="${position}"]`)
        ?.focus();
    });
    $("messages").append(control);
  };
  if (transcriptWindowStart > 0) {
    transcriptWindowControl(
      `Show previous ${Math.min(TRANSCRIPT_WINDOW_SIZE, transcriptWindowStart)} loaded items`,
      Math.max(0, transcriptWindowStart - TRANSCRIPT_WINDOW_SIZE),
      "before",
    );
  }
  visibleItems.forEach((item) => {
    const content = item.content;
    if (isMessageItem(item)) {
      renderMessage(
        item.content.role || (item.kind === "user_message" ? "user" : "assistant"),
        item.content.content,
        transcriptItemIdentity(item),
        item.content.attachments || [],
      );
      return;
    }
    if (item.kind === "approval" && content.type === "approval") {
      const request = content.request;
      const requestId = request.id || item.approval_id;
      if (request.status === "pending" && requestId) {
        renderApproval(requestId, request, item.turn_id);
      }
      return;
    }
    if (item.kind === "plan" && content.type === "plan") {
      renderPlan(item.id, item.turn_id, content.title, content.steps, item.status);
      return;
    }
    if (item.kind === "question" && content.type === "question") {
      renderQuestion(content.request, item.id, item.turn_id);
      return;
    }
    if (item.kind === "artifact" && content.type === "artifact") {
      renderArtifact(item.id, item.turn_id, content.artifact_id, content.title, content.media_type);
      return;
    }
    if (item.kind === "reasoning_summary" && content.type === "reasoning_summary") {
      renderTranscriptNotice(item.id, item.turn_id, item.kind, "Reasoning summary", content.text);
      return;
    }
    if (item.kind === "warning" && content.type === "warning") {
      renderTranscriptNotice(item.id, item.turn_id, item.kind, item.summary || "Warning", content.message);
      return;
    }
    if (item.kind === "context_compaction" && content.type === "context_compaction") {
      const detail = [
        `${content.omitted_messages} earlier message${content.omitted_messages === 1 ? "" : "s"} summarized`,
        content.truncated_messages
          ? `${content.truncated_messages} long message${content.truncated_messages === 1 ? "" : "s"} shortened`
          : "",
        content.estimated_tokens ? `about ${content.estimated_tokens.toLocaleString()} summary tokens` : "",
      ].filter(Boolean).join(" · ");
      renderTranscriptNotice(item.id, item.turn_id, item.kind, "Context optimized", detail);
      return;
    }
    if (item.kind === "model_reroute" && content.type === "model_reroute") {
      const route = content.from_model
        ? `${content.from_model} → ${content.to_model}`
        : content.to_model;
      renderTranscriptNotice(item.id, item.turn_id, item.kind, "Model switched", `${route} · ${content.reason}`);
      return;
    }
    if (item.kind === "usage" && content.type === "usage") {
      renderTranscriptNotice(
        item.id,
        item.turn_id,
        item.kind,
        "Usage",
        `${content.total_tokens.toLocaleString()} tokens · ${content.input_tokens.toLocaleString()} input + ${content.output_tokens.toLocaleString()} output · ${content.model_calls} model / ${content.tool_calls} tool calls · ${content.model}`,
      );
      return;
    }
    if (item.kind === "agent_status" && content.type === "agent_status") {
      renderTranscriptNotice(item.id, item.turn_id, item.kind, "Agent", content.label);
      return;
    }
    if (item.kind === "hook" && content.type === "hook") {
      const detail = [
        `${content.event} · ${content.handler}`,
        content.input_modified ? "input modified" : "",
        content.result_summary || "",
      ].filter(Boolean).join(" · ");
      renderTranscriptNotice(item.id, item.turn_id, item.kind, "Hook", detail);
      return;
    }
    if (isToolItem(item)) {
      const kind = item.status === "cancelled" ? "tool.cancelled" : item.status === "completed" ? "tool.completed" : item.status === "failed" ? "tool.failed" : item.status === "denied" ? "tool.denied" : item.status === "awaiting_approval" ? "approval.required" : "tool.running";
      const tool = item.content.type === "mcp_call"
        ? item.content.namespaced_tool
        : item.content.tool;
      const progress = item.content.type === "mcp_call" ? item.content.progress : null;
      renderToolStep(kind, {
        tool,
        display: item.summary,
        tool_call_id: item.content.tool_call_id || item.id,
        parent_tool_call_id: item.content.type === "tool_call" ? item.content.parent_tool_call_id : null,
        progress: progress?.progress,
        total: progress?.total,
        message: progress?.message,
      }, { item_id: item.id, turn_id: item.turn_id });
    }
  });
  const remainingItems = snapshot.items.length - transcriptWindowStart - visibleItems.length;
  if (remainingItems > 0) {
    transcriptWindowControl(
      `Show next ${Math.min(TRANSCRIPT_WINDOW_SIZE, remainingItems)} loaded items`,
      transcriptWindowStart + TRANSCRIPT_WINDOW_SIZE,
      "after",
    );
  }
  state.pendingInputs = snapshot.pending_inputs || [];
  renderPendingInputs();
  const activeTurn = snapshot.turns.findLast((turn) => !["completed", "failed", "cancelled"].includes(turn.status));
  if (activeTurn) {
    state.turn = activeTurn.id;
    setTurnRunning(true);
    $("turn-state").textContent = activeTurn.status;
  } else {
    state.turn = snapshot.turns.at(-1)?.id || null;
    setTurnRunning(false);
  }
  updateConversationState(snapshot.items.length > 0 || Boolean(workNotice));
}

async function loadEarlierTranscript() {
  const session = state.session;
  const cursor = loadedTranscriptSnapshot?.next_cursor;
  if (!session || !cursor) return;
  if (state.turnRunning) {
    toast("Wait for the running task to finish before loading older history.");
    return;
  }
  const query = catalogQuery();
  query.set("before", cursor);
  query.set("limit", "500");
  $("load-earlier").disabled = true;
  try {
    const snapshot = parseTranscriptSnapshot(await api(
      `/v1/sessions/${encodeURIComponent(session.id)}/snapshot?${query}`,
    ));
    if (state.session?.id !== session.id) return;
    renderTranscriptSnapshot(snapshot, true);
  } finally {
    $("load-earlier").disabled = false;
  }
}

async function executeContent(content: string, files: File[] = []) {
  const session = state.session;
  if (!session) throw new Error("No active session");
  const shellCommand = content.startsWith("!") ? content.slice(1).trim() : null;
  const generation = state.generation;
  const submissionScope = { ...session.scope };
  const stillCurrent = () => isCurrent(generation) && state.session?.id === session.id
    && ownsSession(session, state.authenticatedScope);
  let uploaded: AttachmentMetadata[] = [];
  let optimisticMessage: HTMLElement | null = null;
  try {
    if (files.length) {
      $("turn-state").textContent = "uploading";
      uploaded = await uploadDraftAttachments(session, files);
    }
    if (!stillCurrent()) return;
    optimisticMessage = renderMessage("user", content, {}, uploaded);
    $("turn-state").textContent = "starting";
    $("send-turn").disabled = true;
    if (shellCommand !== null) {
      const outcome = await api<ToolSubmissionResponse>(`/v1/sessions/${encodeURIComponent(session.id)}/tools`, {
        method: "POST",
        body: JSON.stringify({
          scope: submissionScope,
          tool: "run_command",
          arguments: { program: "sh", args: ["-lc", shellCommand], timeout_seconds: 60, network_enabled: false, max_bytes: 1048576 },
        }),
      });
      if (!stillCurrent()) return;
      state.turn = outcome.tool_call?.request?.turn_id || null;
      setTurnRunning(outcome.outcome === "awaiting_approval");
      $("turn-state").textContent = outcome.outcome || "submitted";
      if (outcome.outcome === "completed") renderToolOutcome(outcome);
    } else {
      const turn = await api<Turn>(`/v1/sessions/${encodeURIComponent(session.id)}/turns`, {
        method: "POST",
        body: JSON.stringify({
          scope: submissionScope,
          content,
          attachment_ids: uploaded.map((attachment) => attachment.id),
        }),
      });
      if (!stillCurrent()) return;
      state.draftFiles = [];
      state.draftFilesByContext.set(composerDraftContext(), []);
      $("attachment-input").value = "";
      renderDraftAttachments();
      state.turn = turn.id; setTurnRunning(true); $("undo-turn").disabled = true; $("turn-state").textContent = turn.status;
    }
  }
  catch (error) {
    optimisticMessage?.remove();
    if (isCurrent(generation) && ownsSession(session, state.authenticatedScope)) {
      await Promise.allSettled(uploaded.map((attachment) => deleteDraftAttachment(attachment.id, submissionScope)));
    }
    if (stillCurrent()) {
      updateConversationState();
      if (
        shellCommand === null
        && files.length === 0
        && state.capabilities.has("turn.input_queue.v1")
        && error instanceof Error
        && error.message === "session already has active or queued input"
      ) {
        try {
          await loadMessages();
          if (isCurrent(generation) && state.session?.id === session.id && state.turnRunning && state.turn) {
            await submitTurnInput(content, "queue");
            return;
          }
        } catch (_) {
          // Preserve the original conflict below if state recovery fails.
        }
      }
      if (!$("prompt").value) {
        $("prompt").value = content;
        sessionStorage.setItem(composerTextDraftKey(), content);
        resizePrompt();
      }
      addActivity("turn.error", { error: error.message });
    }
  }
  finally {
    if (stillCurrent()) updateSendAction();
  }
}

async function runTurn(event: SubmitEvent) {
  event.preventDefault();
  if (composerSubmissionPending) return;
  closeMentionMenu();
  const text = $("prompt").value.trim();
  const hasAttachments = state.draftFiles.length > 0;
  const content = text || (hasAttachments ? "Please inspect the attached files." : "");
  if (state.turnRunning) {
    if (hasAttachments) {
      toast("Attachments cannot be added to a running Turn yet. Remove them or wait for completion.");
      return;
    }
    if (content) {
      await withComposerSubmission(async () => {
        try {
          await submitTurnInput(content, "queue");
        } catch (error) {
          addActivity("turn.input.queue.error", { error: error.message });
        }
      });
    }
    else await withComposerSubmission(cancelActiveTurn);
    return;
  }
  if (!content) return;
  const shellCommand = content.startsWith("!") ? content.slice(1).trim() : null;
  if (shellCommand === "") { toast("Enter a command after !"); return; }
  if (shellCommand !== null && hasAttachments) {
    toast("Attachments are available to agent messages, not shell commands.");
    return;
  }
  if (shellCommand === null && !state.capabilities.has("agent.tool_loop")) { addActivity("agent.unavailable", { reason: "Connect the daemon before starting a task" }); openDrawer("settings-drawer"); return; }
  if (!state.session && !await createSession()) return;
  $("prompt").value = ""; sessionStorage.removeItem(composerTextDraftKey()); resizePrompt(); updateSendAction();
  await withComposerSubmission(() => executeContent(content, [...state.draftFiles]));
}

function activityLabel(kind: string) {
  const labels: Record<string, string> = {
    "approval.required": "Decision needed",
    "session.updated": "Session updated",
    "session.cancelled": "Session cancelled",
    "session.deleted": "Session deleted",
    "turn.completed": "Task completed",
    "turn.failed": "Task failed",
    "turn.cancelled": "Task stopped",
    "turn.status": "Task progress",
    "tool.completed": "Tool completed",
    "tool.failed": "Tool failed",
    "tool.denied": "Tool denied",
  };
  return labels[kind] || String(kind).split(".").map((part) => part.charAt(0).toUpperCase() + part.slice(1)).join(" · ");
}

function activityDetail(payload: JsonObject) {
  if (!payload || typeof payload !== "object") return "Update received";
  for (const key of ["error", "error_code", "reason", "status", "tool", "title", "message"]) {
    if (typeof payload[key] === "string" && payload[key].trim()) {
      return key === "tool" ? friendlyTool(payload[key]) : payload[key];
    }
  }
  const identifier = payload.session_id || payload.turn_id || payload.task_id || payload.approval_id;
  return identifier ? `Reference ${String(identifier).slice(0, 12)}` : "Update received";
}

function friendlyTool(tool: string) {
  const labels: Record<string, string> = {
    execute: "Code Mode",
    read_file: "Read file",
    search_text: "Search code",
    apply_patch: "Edit files",
    git_diff: "Review changes",
    shell: "Run shell",
  };
  return labels[tool] || String(tool).replaceAll("_", " ");
}

function shouldRenderToolStep(kind: string) {
  return kind === "approval.required"
    || kind.startsWith("tool.")
    || kind === "mcp.progress"
    || [
      "turn.created",
      "turn.status",
      "turn.awaiting_input",
      "turn.awaiting_approval",
      "turn.completed",
      "turn.failed",
      "turn.cancelled",
    ].includes(kind)
    || kind.endsWith(".error");
}

function renderToolStep(kind: string, payload: JsonObject, envelope: JsonObject = {}) {
  if (!shouldRenderToolStep(kind)) return;
  if (
    payload?.tool === "update_plan"
    || payload?.tool === "request_user_input"
    || payload?.tool === "publish_artifact"
  ) return;
  const itemId = payload?.tool_call_id || payload?.model_call_id || envelope.item_id || null;
  const turnId = envelope.turn_id || null;
  const toolEvent = kind.startsWith("tool.") || kind === "approval.required" || kind === "mcp.progress";
  const family = toolEvent
    ? `tool:${itemId || `${turnId || "unknown"}:${payload?.tool || "tool"}`}`
    : kind.startsWith("turn.")
      ? `turn:${turnId || "unknown"}`
      : `${kind}:${itemId || turnId || "unknown"}`;
  let item = state.toolSteps.get(family);
  if (!item && toolEvent && kind !== "tool.proposed" && !payload?.parent_tool_call_id) {
    const pending = [...state.toolSteps.entries()].find(([, candidate]) =>
      canBindToolProposal(candidate.dataset, turnId || "", payload?.tool || "tool") &&
      !candidate.classList.contains("complete") &&
      !candidate.classList.contains("error")
    );
    if (pending) {
      const [previousFamily, candidate] = pending;
      state.toolSteps.delete(previousFamily);
      state.toolSteps.set(family, candidate);
      item = candidate;
    }
  }
  if (!item?.isConnected) {
    item = document.createElement("div");
    item.className = "tool-step";
    if (itemId) item.dataset.itemId = itemId;
    if (turnId) item.dataset.turnId = turnId;
    item.dataset.tool = payload?.tool || "tool";
    item.setAttribute("role", "status");
    const marker = document.createElement("span");
    marker.className = "tool-step-marker";
    marker.setAttribute("aria-hidden", "true");
    const copy = document.createElement("span");
    copy.className = "tool-step-copy";
    const title = document.createElement("strong");
    const detail = document.createElement("span");
    copy.append(title, detail);
    item.append(marker, copy);
    const streamingAnswer = turnId
      ? [...$("messages").querySelectorAll<HTMLElement>(".message.assistant.streaming")]
        .find((candidate) => candidate.dataset.turnId === turnId)
      : null;
    if (toolEvent && streamingAnswer) {
      $("messages").insertBefore(item, streamingAnswer);
    } else {
      $("messages").append(item);
    }
    state.toolSteps.set(family, item);
    if (itemId) state.itemsById.set(itemId, item);
  }
  // Cancellation is final even if an older completion arrives late.
  if (item.dataset.cancelled === "true" && kind !== "tool.cancelled") return;
  if (kind === "tool.cancelled") item.dataset.cancelled = "true";
  item.dataset.proposed = String(kind === "tool.proposed");
  if (itemId) {
    item.dataset.itemId = itemId;
    state.itemsById.set(itemId, item);
  }
  if (typeof payload?.display === "string" && payload.display.trim()) {
    item.dataset.display = payload.display;
  }
  if (typeof payload?.parent_tool_call_id === "string") {
    item.dataset.parentToolCallId = payload.parent_tool_call_id;
    item.classList.add("code-mode-child");
    item.setAttribute("aria-label", "Code Mode child tool");
  }
  if (payload?.tool === "execute") item.classList.add("code-mode-parent");
  item.classList.toggle("error", /(error|failed)/.test(kind));
  item.classList.toggle("decision", /(approval|denied|policy)/.test(kind));
  item.classList.toggle("complete", /(completed|cancelled)/.test(kind));
  item.classList.toggle("cancelled", kind.endsWith(".cancelled"));
  const title = item.querySelector<HTMLElement>(":scope > .tool-step-copy > strong");
  const detail = item.querySelector<HTMLElement>(":scope > .tool-step-copy > span");
  if (title) title.textContent = activityLabel(kind);
  if (detail) {
    detail.textContent = toolEvent
      ? `${item.dataset.parentToolCallId ? "Code Mode › " : ""}${toolStepDetail(payload, item.dataset.display)}`
      : activityDetail(payload);
  }
  groupCodeModeTools();
  updateConversationState(true);
}

function groupCodeModeTools() {
  const rows = [...state.toolSteps.values()].filter((row) => row.isConnected);
  for (const child of rows) {
    const parentId = child.dataset.parentToolCallId;
    if (!parentId) continue;
    const parent = rows.find((row) => row.dataset.itemId === parentId && row.dataset.tool === "execute");
    if (!parent) continue; // A paginated child retains its standalone Code Mode label.
    let children = parent.querySelector<HTMLElement>(":scope > .code-mode-children");
    if (!children) {
      children = document.createElement("div");
      children.className = "code-mode-children";
      children.setAttribute("role", "group");
      children.setAttribute("aria-label", "Code Mode tool calls");
      parent.append(children);
    }
    if (child.parentElement !== children) children.append(child);
  }
}

function toolStepDetail(payload: JsonObject, preservedDisplay?: string) {
  const base = typeof payload?.display === "string" && payload.display.trim()
    ? payload.display
    : preservedDisplay || friendlyTool(payload?.tool || "tool");
  if (typeof payload?.progress !== "number") return base;
  const amount = typeof payload.total === "number" && payload.total > 0
    ? `${Math.max(0, Math.min(100, Math.round(payload.progress / payload.total * 100)))}%`
    : String(payload.progress);
  return [base, amount, typeof payload.message === "string" ? payload.message : ""]
    .filter(Boolean)
    .join(" · ");
}

function addActivity(kind: string, payload: JsonObject, envelope: JsonObject = {}) {
  if (!state.connected || kind === "model.delta") return;
  renderToolStep(kind, payload, envelope);
  $("activity").classList.remove("empty");
  if ($("activity").textContent === "No activity yet") $("activity").replaceChildren();
  const item = document.createElement("div");
  item.className = `event${/(error|failed)/.test(kind) ? " error" : /(approval|denied|policy)/.test(kind) ? " decision" : ""}`;
  const title = document.createElement("strong"); title.textContent = activityLabel(kind);
  const detail = document.createElement("span"); detail.textContent = activityDetail(payload);
  const time = document.createElement("time"); time.dateTime = new Date().toISOString(); time.textContent = "just now";
  item.append(title, detail, time); $("activity").prepend(item);
  while ($("activity").children.length > 100) $("activity").lastChild?.remove();
}

function renderApproval(
  id: string,
  requestOrTool: ApprovalRequest | string | null | undefined,
  turnId: string | null = null,
) {
  if (!id || state.approvals.has(id)) return; state.approvals.add(id);
  const row = document.createElement("div"); row.className = "approval"; row.dataset.id = id;
  row.dataset.itemId = id;
  if (turnId) row.dataset.turnId = turnId;
  const request = typeof requestOrTool === "object" && requestOrTool !== null
    ? requestOrTool
    : null;
  const summary = request?.summary || (typeof requestOrTool === "string" ? requestOrTool : "Tool");
  const copy = document.createElement("section"); copy.className = "approval-copy";
  const label = document.createElement("strong"); label.textContent = `${summary} needs permission to continue`;
  copy.append(label);
  if (request) {
    const meta = document.createElement("span");
    meta.textContent = `${request.risk} risk · ${request.impact_scope}`;
    copy.append(meta);
    appendApprovalTarget(copy, request);
  }
  const actions = document.createElement("div");
  const choices: Array<{
    label: string;
    approved: boolean;
    scope: "once" | "session";
  }> = [
    { label: "Allow once", approved: true, scope: "once" },
    { label: "Reject", approved: false, scope: "once" },
  ];
  choices.forEach((choice) => {
    const button = document.createElement("button");
    button.textContent = choice.label;
    button.classList.toggle("primary", choice.approved && choice.scope === "once");
    button.addEventListener("click", () => resolveApproval(id, choice.approved, row, choice.scope));
    actions.append(button);
  });
  row.append(copy, actions); $("approvals").append(row); state.itemsById.set(id, row);
  announce(`Approval required for ${summary}`);
}

async function resolveApproval(
  id: string,
  approved: boolean,
  row: HTMLElement,
  approvalScope: "once" | "session" = "once",
) {
  const generation = state.generation;
  try { await api(`/v1/approvals/${encodeURIComponent(id)}`, { method: "POST", body: JSON.stringify({ scope: scope(), approved, approval_scope: approvalScope }) }); if (!isCurrent(generation)) return; row.remove(); state.approvals.delete(id); }
  catch (error) { if (isCurrent(generation)) addActivity("approval.error", { error: error.message }); }
}

function renderQuestion(
  request: QuestionRequestView,
  itemId: string | null = null,
  turnId: string | null = null,
) {
  if (!request?.id || !Array.isArray(request.questions)) return null;
  const stableItemId = itemId || request.item_id || `question_${request.id}`;
  const existing = state.itemsById.get(stableItemId);
  const previousTimer = existing?._countdownTimer;
  if (typeof previousTimer === "number") window.clearInterval(previousTimer);
  const card = document.createElement("article");
  card.className = "question-card";
  card.dataset.itemId = stableItemId;
  card.dataset.requestId = request.id;
  if (turnId || request.turn_id) card.dataset.turnId = turnId || request.turn_id;
  const heading = document.createElement("div");
  heading.className = "question-heading";
  const title = document.createElement("strong");
  title.textContent = request.status === "answered" ? "Answered" : "Your input is needed";
  const count = document.createElement("span");
  count.textContent = `${request.questions.length} question${request.questions.length === 1 ? "" : "s"}`;
  heading.append(title, count);
  if (request.status !== "answered" && request.expires_at) {
    const deadline = Date.parse(request.expires_at);
    if (Number.isFinite(deadline)) {
      const countdown = document.createElement("span");
      countdown.className = "question-countdown";
      const updateCountdown = () => {
        const seconds = Math.max(0, Math.ceil((deadline - Date.now()) / 1000));
        countdown.textContent = seconds > 0
          ? `Recommended defaults in ${seconds}s`
          : "Applying recommended defaults…";
        if (seconds <= 0 && typeof card._countdownTimer === "number") {
          window.clearInterval(card._countdownTimer);
          delete card._countdownTimer;
        }
      };
      updateCountdown();
      card._countdownTimer = window.setInterval(updateCountdown, 1000);
      heading.append(countdown);
    }
  }
  card.append(heading);

  if (request.status === "answered") {
    request.questions.forEach((prompt) => {
      const row = document.createElement("div");
      row.className = "question-resolved";
      const label = document.createElement("strong");
      label.textContent = prompt.header;
      const answer = request.answers?.find((value) => value.question_id === prompt.id);
      const value = document.createElement("span");
      value.textContent = answer?.answer || "Answered in another client";
      row.append(label, value);
      card.append(row);
    });
  } else {
    const form = document.createElement("form");
    form.className = "question-form";
    request.questions.forEach((prompt, questionIndex) => {
      const fieldset = document.createElement("fieldset");
      fieldset.dataset.questionId = prompt.id;
      const legend = document.createElement("legend");
      const eyebrow = document.createElement("span");
      eyebrow.textContent = prompt.header;
      const promptText = document.createElement("strong");
      promptText.textContent = prompt.question;
      legend.append(eyebrow, promptText);
      fieldset.append(legend);
      (prompt.options || []).forEach((option, optionIndex) => {
        const label = document.createElement("label");
        label.className = "question-option";
        const radio = document.createElement("input");
        radio.type = "radio";
        radio.name = `question-${request.id}-${questionIndex}`;
        radio.value = option.label;
        if (optionIndex === 0) radio.dataset.first = "true";
        const copy = document.createElement("span");
        const name = document.createElement("strong");
        name.textContent = option.label;
        const description = document.createElement("small");
        description.textContent = option.description;
        copy.append(name, description);
        label.append(radio, copy);
        fieldset.append(label);
      });
      if (request.allow_other !== false) {
        const other = document.createElement("input");
        other.className = "question-other";
        other.type = "text";
        other.maxLength = 500;
        other.placeholder = "Or type another answer";
        other.setAttribute("aria-label", `${prompt.header} other answer`);
        fieldset.append(other);
      }
      form.append(fieldset);
    });
    const error = document.createElement("p");
    error.className = "question-error";
    error.setAttribute("role", "alert");
    const submit = document.createElement("button");
    submit.type = "submit";
    submit.className = "primary";
    submit.textContent = "Submit answer";
    form.append(error, submit);
    form.addEventListener("submit", async (event) => {
      event.preventDefault();
      const answers: Array<{ question_id: string; answer: string }> = [];
      for (const fieldset of form.querySelectorAll("fieldset")) {
        const selected = fieldset.querySelector<HTMLInputElement>('input[type="radio"]:checked');
        const other = fieldset.querySelector<HTMLInputElement>(".question-other")?.value.trim();
        const answer = other || selected?.value || "";
        if (!answer) {
          error.textContent = "Answer each question before continuing.";
          return;
        }
        const questionId = (fieldset as HTMLElement).dataset.questionId;
        if (!questionId) {
          error.textContent = "This question is missing its identifier.";
          return;
        }
        answers.push({ question_id: questionId, answer });
      }
      error.textContent = "";
      submit.disabled = true;
      const generation = state.generation;
      try {
        const answered = await api<QuestionRequest>(`/v1/questions/${encodeURIComponent(request.id)}`, {
          method: "POST",
          body: JSON.stringify({ scope: scope(), answers }),
        });
        if (!isCurrent(generation)) return;
        renderQuestion(answered, stableItemId, turnId || request.turn_id);
        $("turn-state").textContent = "continuing";
      } catch (submitError) {
        if (isCurrent(generation)) {
          error.textContent = submitError.message;
          submit.disabled = false;
        }
      }
    });
    card.append(form);
  }
  if (existing) existing.replaceWith(card);
  else $("messages").append(card);
  state.itemsById.set(stableItemId, card);
  state.questions.add(request.id);
  updateConversationState(true);
  if (request.status !== "answered") announce("Your input is needed");
  return card;
}

function markQuestionAnswered(requestId: string, itemId: string | null = null) {
  const card = (itemId
    ? state.itemsById.get(itemId)
    : $("messages").querySelector(`[data-request-id="${CSS.escape(requestId || "")}"]`)) as HTMLElement | null;
  if (!card) return;
  if (typeof card._countdownTimer === "number") {
    window.clearInterval(card._countdownTimer);
    delete card._countdownTimer;
  }
  card.classList.add("resolved");
  const heading = card.querySelector(".question-heading strong");
  if (heading) heading.textContent = "Answered in another client";
  card.querySelectorAll<HTMLInputElement | HTMLButtonElement>("input, button").forEach((control) => {
    control.disabled = true;
  });
}

async function showDiff() {
  if (!hasWorkspace(state.session) || !state.session) { toast("Start Work to view file changes"); return; }
  const generation = state.generation;
  setToolMessage("Loading changes…");
  try { const outcome = await api<JsonObject>(`/v1/sessions/${encodeURIComponent(state.session.id)}/tools`, { method: "POST", body: JSON.stringify({ scope: scope(), tool: "git_diff", arguments: { paths: [], max_bytes: 524288 } }) }); if (isCurrent(generation)) { renderToolOutcome(outcome); announce("Changes loaded"); } }
  catch (error) { if (isCurrent(generation)) setToolMessage(error.message, true); }
}

function setToolMessage(message: string, error = false) {
  const target = $("tool-result");
  target.className = `tool-result ${error ? "error" : "empty"}`;
  target.replaceChildren(document.createTextNode(message));
}

function toolResult(outcome: JsonObject): unknown {
  return outcome?.tool_call?.result ?? outcome?.result ?? outcome;
}

function renderDiffSnapshot(snapshot: JsonObject) {
  const target = $("tool-result");
  const diff = String(snapshot.unified_diff || "");
  if (!diff) {
    setToolMessage("Working tree is clean — no changes to review.");
    return;
  }
  const lines = diff.split("\n");
  const additions = lines.filter((line) => line.startsWith("+") && !line.startsWith("+++")).length;
  const deletions = lines.filter((line) => line.startsWith("-") && !line.startsWith("---")).length;
  const files = lines.filter((line) => line.startsWith("diff --git ")).length;
  target.className = "tool-result diff-result";
  target.replaceChildren();
  const summary = document.createElement("div");
  summary.className = "diff-summary";
  const identity = document.createElement("div");
  const title = document.createElement("strong");
  title.textContent = `${files || 1} changed ${files === 1 ? "file" : "files"}`;
  const hash = document.createElement("small");
  hash.textContent = `${snapshot.truncated ? "Partial diff" : "Complete diff"} · ${String(snapshot.sha256 || "no hash").slice(0, 12)}`;
  identity.append(title, hash);
  const added = document.createElement("span"); added.className = "diff-stat add"; added.textContent = `+${additions}`;
  const removed = document.createElement("span"); removed.className = "diff-stat delete"; removed.textContent = `−${deletions}`;
  const copy = document.createElement("button"); copy.type = "button"; copy.className = "code-copy"; copy.textContent = "Copy"; copy.setAttribute("aria-label", "Copy unified diff"); copy.addEventListener("click", () => copyText(diff, "Diff copied"));
  summary.append(identity, added, removed, copy);
  const pre = document.createElement("pre"); pre.className = "diff-code"; pre.setAttribute("aria-label", "Unified diff");
  lines.forEach((line) => {
    const row = document.createElement("span");
    row.className = `diff-line${line.startsWith("diff --git") || line.startsWith("---") || line.startsWith("+++") ? " header" : line.startsWith("@@") ? " hunk" : line.startsWith("+") ? " add" : line.startsWith("-") ? " delete" : ""}`;
    row.textContent = line || " ";
    pre.append(row);
  });
  target.append(summary, pre);
}

function renderToolOutcome(outcome: JsonObject) {
  const result = toolResult(outcome);
  if (isJsonObject(result) && typeof result.unified_diff === "string") {
    renderDiffSnapshot(result);
    return;
  }
  const target = $("tool-result");
  target.className = "tool-result structured";
  target.replaceChildren();
  const pre = document.createElement("pre"); pre.className = "structured-result";
  pre.textContent = typeof result === "string" ? result : JSON.stringify(result, null, 2);
  target.append(pre);
}

async function undoTurn(turnId = state.turn) {
  if (!turnId || !state.capabilities.has("turn.undo")) return;
  const generation = state.generation;
  const s = scope();
  const query = new URLSearchParams({
    organization_id: s.organization_id,
    team_id: s.team_id,
    actor_id: s.actor_id,
  });
  let impact: TurnUndoImpactPreview;
  try {
    impact = await api<TurnUndoImpactPreview>(
      `/v1/turns/${encodeURIComponent(turnId)}/undo-impact?${query}`,
    );
  } catch (error) {
    if (isCurrent(generation)) setToolMessage(`Unable to preview rollback impact: ${error.message}`, true);
    return;
  }
  const details = impact.paths.length
    ? impact.paths.map((path) => `${path.action === "delete" ? "Delete created file" : "Restore previous content"}: ${path.path}`)
    : ["No file changes were recorded for this turn"];
  if (impact.conversation_messages) details.push(`Remove ${impact.conversation_messages} message(s) from future model context`);
  if (impact.plan_items) details.push(`Rewind ${impact.plan_items} plan item(s)`);
  if (impact.queued_inputs) details.push(`Cancel ${impact.queued_inputs} queued follow-up input(s)`);
  if (impact.session_goal_changes) details.push("Restore the Session Goal state before this turn");
  const values = await requestAction({
    eyebrow: "Safe rollback",
    title: "Undo this turn?",
    description: "Conversation-time state is rewound together. Audit, usage and external side effects remain recorded.",
    details,
    confirm: "Undo turn",
    danger: true,
  });
  if (!values) return;
  try {
    const result = await api<JsonObject>(`/v1/turns/${encodeURIComponent(turnId)}/undo`, { method: "POST", body: JSON.stringify({ scope: scope() }) });
    if (isCurrent(generation)) {
      renderToolOutcome(result);
      await Promise.all([loadMessages(), loadSessionGoal()]);
      if (turnId === state.turn) $("undo-turn").disabled = true;
      toast("Turn and conversation state restored");
    }
  } catch (error) { if (isCurrent(generation)) setToolMessage(error.message, true); }
}

async function showCheckpoints() {
  if (!hasWorkspace(state.session) || !state.session) return;
  const generation = state.generation;
  const s = scope();
  const query = new URLSearchParams({
    organization_id: s.organization_id,
    team_id: s.team_id,
    actor_id: s.actor_id,
  });
  try {
    const turns = await api<Turn[]>(`/v1/sessions/${encodeURIComponent(state.session.id)}/turns?${query}`);
    if (!isCurrent(generation)) return;
    const target = $("tool-result");
    target.replaceChildren();
    target.className = `tool-result checkpoint-list${turns.length ? "" : " empty"}`;
    if (!turns.length) {
      target.textContent = "No checkpoints in this session.";
      return;
    }
    [...turns].reverse().forEach((turn, index) => {
      const row = document.createElement("article");
      row.className = "checkpoint-row";
      const detail = document.createElement("div");
      const title = document.createElement("strong");
      title.textContent = index === 0 ? "Latest turn" : `Checkpoint ${turns.length - index}`;
      const meta = document.createElement("small");
      meta.textContent = `${turn.status} · ${turn.id}`;
      detail.append(title, meta);
      const undo = document.createElement("button");
      undo.type = "button";
      undo.textContent = "Undo";
      undo.disabled = !state.capabilities.has("turn.undo") || turn.status !== "completed";
      undo.addEventListener("click", () => undoTurn(turn.id));
      row.append(detail, undo);
      target.append(row);
    });
  } catch (error) {
    if (isCurrent(generation)) setToolMessage(error.message, true);
  }
}

async function showBranches() {
  if (!state.session) return;
  const selectedId = state.session.id;
  const s = scope();
  const query = new URLSearchParams({
    organization_id: s.organization_id,
    team_id: s.team_id,
    actor_id: s.actor_id,
  });
  try {
    const tree = await api<SessionBranchTree>(
      `/v1/sessions/${encodeURIComponent(selectedId)}/branches?${query}`,
    );
    if (state.session?.id !== selectedId) return;
    const target = $("tool-result");
    target.replaceChildren();
    target.className = "tool-result branch-list";
    const byId = new Map(tree.nodes.map((node) => [node.session.id, node]));
    const depth = (
      node: SessionBranchTree["nodes"][number],
      seen: Set<string> = new Set(),
    ): number => {
      if (!node.parent_session_id || seen.has(node.session.id)) return 0;
      seen.add(node.session.id);
      const parent = byId.get(node.parent_session_id);
      return parent ? 1 + depth(parent, seen) : 0;
    };
    tree.nodes.forEach((node) => {
      const row = document.createElement("article");
      row.className = "branch-row";
      row.style.setProperty("--branch-indent", `${Math.min(depth(node), 6) * 18}px`);
      const marker = document.createElement("span");
      marker.className = "branch-marker";
      marker.setAttribute("aria-hidden", "true");
      const detail = document.createElement("div");
      const title = document.createElement("strong");
      title.textContent = node.session.title;
      const meta = document.createElement("small");
      const current = node.session.id === selectedId ? "Current · " : "";
      const source = node.source_turn_id ? ` · after ${node.source_turn_id}` : "";
      meta.textContent = `${current}${node.session.status} · ${node.session.model}${source}`;
      detail.append(title, meta);
      const open = document.createElement("button");
      open.type = "button";
      open.textContent = node.session.id === selectedId ? "Open" : "Switch";
      open.disabled = node.session.id === selectedId;
      open.addEventListener("click", () => selectSession(node.session));
      row.append(marker, detail, open);
      target.append(row);
    });
    if (!tree.nodes.length) {
      target.classList.add("empty");
      target.textContent = "No branches are available.";
    }
  } catch (error) {
    toast(error.message);
  }
}

async function exportSession() {
  if (!state.session) return;
  const sessionId = state.session.id;
  const values = await requestAction({
    eyebrow: "Session",
    title: "Export transcript",
    description: "Download a point-in-time copy of the visible transcript. Attachment contents are not embedded.",
    confirm: "Download",
    fields: [{
      name: "format",
      label: "Format",
      options: [["markdown", "Markdown"], ["json", "JSON snapshot"]],
    }],
  });
  if (!values || state.session?.id !== sessionId) return;
  const s = scope();
  const query = new URLSearchParams({
    organization_id: s.organization_id,
    team_id: s.team_id,
    actor_id: s.actor_id,
    format: values.format || "markdown",
  });
  try {
    const exported = await api<SessionExport>(
      `/v1/sessions/${encodeURIComponent(sessionId)}/export?${query}`,
    );
    if (state.session?.id !== sessionId) return;
    const blob = new Blob([exported.content], { type: `${exported.media_type};charset=utf-8` });
    const href = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = href;
    link.download = exported.file_name;
    document.body.append(link);
    link.click();
    link.remove();
    window.setTimeout(() => URL.revokeObjectURL(href), 0);
    toast(`Transcript exported · ${exported.sha256.slice(0, 12)}`);
  } catch (error) {
    toast(error.message);
  }
}

async function createMemory() {
  if (!state.session) return;
  const values = await requestAction({
    eyebrow: "Context memory",
    title: "Remember this context",
    description: "Memory is encrypted at rest, requires a citation, and rejects likely secrets.",
    confirm: "Save memory",
    fields: [
      {
        name: "memoryScope",
        label: "Scope",
        options: [["project", "This project"], ["user", "My sessions"], ["team", "Team"]],
      },
      { name: "citation", label: "Citation / source", placeholder: "Architecture decision ADR-012", required: true, maxlength: 500 },
      { name: "content", label: "What should S-Code remember?", multiline: true, required: true, maxlength: 16_000 },
      {
        name: "expiry",
        label: "Expires",
        options: [["30", "In 30 days"], ["7", "In 7 days"], ["90", "In 90 days"], ["never", "Never"]],
      },
    ],
  });
  if (!values) return;
  const expiresAt = values.expiry === "never"
    ? null
    : new Date(Date.now() + Number(values.expiry) * 86_400_000).toISOString();
  try {
    await api<MemoryItem>(
      `/v1/sessions/${encodeURIComponent(state.session.id)}/memories`,
      {
        method: "POST",
        body: JSON.stringify({
          scope: scope(),
          memory_scope: values.memoryScope as MemoryScope,
          citation: values.citation,
          content: values.content,
          expires_at: expiresAt,
        }),
      },
    );
    toast("Memory saved with its citation");
    await showContext();
  } catch (error) {
    toast(error.message);
  }
}

async function removeMemory(memory: MemoryItem) {
  if (!state.session) return;
  const values = await requestAction({
    eyebrow: "Context memory",
    title: "Remove this memory?",
    description: memory.citation,
    details: [`Scope: ${memory.memory_scope}`, `Source: ${memory.source_uri}`],
    confirm: "Remove memory",
    danger: true,
  });
  if (!values) return;
  try {
    await api(`/v1/memories/${encodeURIComponent(memory.id)}?${catalogQuery()}`, {
      method: "DELETE",
    });
    toast("Memory removed");
    await showContext();
  } catch (error) {
    toast(error.message);
  }
}

async function compactContext() {
  if (!state.session) return;
  const values = await requestAction({
    eyebrow: "Context",
    title: "Compact earlier conversation?",
    description: "Recent messages stay verbatim. Earlier messages become a bounded summary; the full transcript remains available.",
    confirm: "Compact context",
    fields: [{
      name: "focus",
      label: "Preserve this focus (optional, kept for 24 hours)",
      multiline: true,
      maxlength: 500,
      placeholder: "Keep test evidence and final architecture decisions.",
    }],
  });
  if (!values) return;
  try {
    const result = await api<CompactSessionResult>(
      `/v1/sessions/${encodeURIComponent(state.session.id)}/compact`,
      {
        method: "POST",
        body: JSON.stringify({
          scope: scope(),
          focus: values.focus || null,
        }),
      },
    );
    toast(`Context compacted · ${result.before_tokens.toLocaleString()} → ${result.after_tokens.toLocaleString()} tokens`);
    await showContext();
  } catch (error) {
    toast(error.message);
  }
}

async function showContext() {
  if (!state.session || !state.capabilities.has("context.explain")) return;
  const generation = state.generation;
  const s = scope();
  const query = new URLSearchParams({
    organization_id: s.organization_id,
    team_id: s.team_id,
    actor_id: s.actor_id,
  });
  try {
    const [summary, memories] = await Promise.all([
      api<ContextSummary>(`/v1/sessions/${encodeURIComponent(state.session.id)}/context?${query}`),
      api<MemoryItem[]>(`/v1/sessions/${encodeURIComponent(state.session.id)}/memories?${query}`),
    ]);
    if (!isCurrent(generation)) return;
    const target = $("tool-result");
    target.replaceChildren();
    target.className = "tool-result context-summary";
    const heading = document.createElement("div");
    heading.className = "context-total";
    const title = document.createElement("strong");
    title.textContent = `${summary.total_estimated_tokens.toLocaleString()} estimated tokens`;
    const meta = document.createElement("small");
    meta.textContent = `${summary.conversation_tokens.toLocaleString()} conversation · ${summary.item_tokens.toLocaleString()} sources · ${summary.reserved_output_tokens.toLocaleString()} reserved output`;
    const usage = document.createElement("small");
    usage.textContent = `Session used ${state.usage.total_tokens.toLocaleString()} tokens · ${state.usage.input_tokens.toLocaleString()} input + ${state.usage.output_tokens.toLocaleString()} output · ${state.usage.model_calls} model / ${state.usage.tool_calls} tool calls`;
    heading.append(title, meta, usage);
    target.append(heading);
    const actions = document.createElement("div");
    actions.className = "context-actions";
    const compact = document.createElement("button");
    compact.type = "button";
    compact.textContent = "Compact";
    compact.addEventListener("click", compactContext);
    const remember = document.createElement("button");
    remember.type = "button";
    remember.textContent = "Add memory";
    remember.addEventListener("click", createMemory);
    actions.append(compact, remember);
    target.append(actions);
    if (!summary.items.length) {
      const empty = document.createElement("p");
      empty.textContent = "No project, Team Knowledge, or editor context is loaded.";
      target.append(empty);
    }
    summary.items.forEach((item) => {
      const row = document.createElement("article");
      row.className = "context-row";
      const identity = document.createElement("div");
      const source = document.createElement("strong");
      source.textContent = item.source_uri;
      const detail = document.createElement("small");
      detail.textContent = `${item.kind.replaceAll("_", " ")} · ${item.estimated_tokens.toLocaleString()} tokens · ${item.trust_level}${item.pinned ? " · pinned" : ""}`;
      identity.append(source, detail);
      row.append(identity);
      target.append(row);
    });
    const memoryHeading = document.createElement("div");
    memoryHeading.className = "context-section-heading";
    memoryHeading.textContent = `Memory · ${memories.length}`;
    target.append(memoryHeading);
    if (!memories.length) {
      const empty = document.createElement("p");
      empty.className = "context-empty";
      empty.textContent = "No active User, Project, or Team memory.";
      target.append(empty);
    }
    memories.forEach((memory) => {
      const row = document.createElement("article");
      row.className = "context-row memory-row";
      const identity = document.createElement("div");
      const citation = document.createElement("strong");
      citation.textContent = memory.citation;
      const detail = document.createElement("small");
      detail.textContent = `${memory.memory_scope} · ${memory.expires_at ? `expires ${new Date(memory.expires_at).toLocaleDateString()}` : "no expiry"} · ${memory.source_uri}`;
      identity.append(citation, detail);
      const remove = document.createElement("button");
      remove.type = "button";
      remove.textContent = "Remove";
      remove.setAttribute("aria-label", `Remove memory ${memory.citation}`);
      remove.addEventListener("click", () => removeMemory(memory));
      row.append(identity, remove);
      target.append(row);
    });
  } catch (error) {
    if (isCurrent(generation)) setToolMessage(error.message, true);
  }
}

async function startReview() {
  if (!hasWorkspace(state.session) || !state.session || !state.capabilities.has("review.read_only")) return;
  if (state.turnRunning) {
    toast("Finish or stop the current turn before starting a review");
    return;
  }
  const generation = state.generation;
  const label = "Review uncommitted changes";
  renderMessage("user", label);
  $("turn-state").textContent = "starting review";
  try {
    const turn = await api<Turn>(`/v1/sessions/${encodeURIComponent(state.session.id)}/reviews`, {
      method: "POST",
      body: JSON.stringify({
        scope: scope(),
        target: "uncommitted",
        instructions: null,
      }),
    });
    if (!isCurrent(generation)) return;
    state.turn = turn.id;
    setTurnRunning(true);
    $("undo-turn").disabled = true;
    $("turn-state").textContent = turn.status;
    toast("Read-only review started");
  } catch (error) {
    if (isCurrent(generation)) addActivity("review.error", { error: error.message });
  }
}

async function renderServerStartedInput(
  inputId: string,
  itemId: string,
  turnId: string,
  sessionId: string,
) {
  if (!inputId || !itemId || !turnId || !sessionId || state.session?.id !== sessionId) return;
  try {
    const s = scope();
    const query = new URLSearchParams({
      organization_id: s.organization_id,
      team_id: s.team_id,
      actor_id: s.actor_id,
    });
    const input = await api<TurnInput>(`/v1/turn-inputs/${encodeURIComponent(inputId)}?${query}`);
    if (state.session?.id !== sessionId || state.itemsById.has(itemId)) return;
    renderMessage("user", input.content, { itemId, turnId });
  } catch (error) {
    addActivity("turn.input.load.error", { error: error.message });
  }
}

function handleEvent(kind: string, payload: JsonObject, envelope: JsonObject = {}) {
  if (kind === "mcp.progress") renderToolStep(kind, payload, envelope);
  else if (!["turn.usage", "reasoning.summary.delta"].includes(kind)) {
    addActivity(kind, payload, envelope);
  }
  if (kind === "session.mode_changed" && envelope.session_id) {
    state.sessions = state.sessions.map((session) => applyWorkTransition(session, envelope.session_id, payload));
    if (state.session && state.session.id === envelope.session_id) {
      const previous = state.session;
      state.session = applyWorkTransition(previous, envelope.session_id, payload);
      updateContextChips();
      const workNotice = workTransitionNotice(state.session);
      if (workNotice) {
        renderTranscriptNotice(workNotice.id, envelope.turn_id, "work_started", "Work started", workNotice.detail);
        announce(`Work started in ${state.session.workspace_uri}`);
      }
    }
    renderSessions();
  }
  if (kind === "session.updated" && envelope.session_id) {
    const session = state.session;
    if (session && session.id === envelope.session_id) {
      if (payload.title) { session.title = payload.title; $("session-title").textContent = payload.title; }
      if (payload.status) {
        session.status = payload.status;
        $("session-meta").textContent = sessionDescription(session);
        $("cancel-session").textContent = session.status === "archived" ? "Restore" : "Archive";
        $("cancel-session").classList.toggle("danger", session.status !== "archived");
        updateContextChips();
      }
    }
    refreshSessions().catch((error) => addActivity("session.refresh.error", { error: error.message }));
  }
  if (kind === "session.cancelled" || kind === "session.deleted") {
    if (state.session && envelope.session_id === state.session.id) {
      if (kind === "session.deleted") clearSessionSelection();
      else { state.session.status = "archived"; $("session-meta").textContent = sessionDescription(state.session); $("cancel-session").disabled = false; $("cancel-session").textContent = "Restore"; $("cancel-session").classList.remove("danger"); updateContextChips(); }
    }
    refreshSessions().catch((error) => addActivity("session.refresh.error", { error: error.message }));
  }
  const notificationMessage = {
    "approval.required": "A task needs an approval decision.",
    "question.required": "A task is waiting for your answer.",
    "turn.completed": "A task completed.",
    "turn.failed": "A task failed. Open S-Code for details.",
    "terminal.completed": "A background terminal finished. Its output Artifact is ready.",
  }[kind];
  if (notificationMessage) {
    notifyUser(
      `s-code:${envelope.session_id || "team"}:${envelope.turn_id || "none"}:${kind}`,
      notificationMessage,
    );
  }
  if (kind === "terminal.started" || kind === "terminal.completed") {
    refreshTeam().catch((error) => addActivity("terminal.refresh.error", { error: error.message }));
  }
  if (kind.startsWith("client.presence.") || kind === "client.remote_grant_revoked") {
    updateClientPresence().catch((error) =>
      addActivity("client.presence.refresh.error", { error: error.message })
    );
  }
  if (kind === "approval.required" || kind === "approval.resolved") {
    refreshTeam().catch((error) => addActivity("approval.refresh.error", { error: error.message }));
  }
  if (envelope.session_id && state.session && envelope.session_id !== state.session.id) return;
  if (kind === "turn.created" && envelope.turn_id) {
    state.turn = envelope.turn_id;
    setTurnRunning(true);
    if (payload.source_input_id) {
      renderServerStartedInput(
        payload.source_input_id,
        envelope.item_id || payload.item_id,
        envelope.turn_id,
        envelope.session_id,
      );
    }
  }
  if (kind === "plan.updated") {
    renderPlan(
      envelope.item_id || payload.item_id,
      envelope.turn_id,
      payload.title,
      Array.isArray(payload.steps) ? payload.steps : [],
      payload.status || envelope.status,
    );
  }
  if (kind === "question.required") {
    renderQuestion({
      id: envelope.request_id || payload.request_id,
      item_id: envelope.item_id || payload.item_id,
      turn_id: envelope.turn_id,
      questions: Array.isArray(payload.questions) ? payload.questions : [],
      allow_other: payload.allow_other !== false,
      expires_at: payload.expires_at || null,
      status: "pending",
      answers: [],
    }, envelope.item_id || payload.item_id, envelope.turn_id);
    $("turn-state").textContent = "awaiting input";
    setTurnRunning(true);
  }
  if (kind === "question.answered") {
    markQuestionAnswered(
      envelope.request_id || payload.request_id,
      envelope.item_id || payload.item_id,
    );
    $("turn-state").textContent = "continuing";
  }
  if (kind === "artifact.created") {
    renderArtifact(
      envelope.item_id || payload.item_id,
      envelope.turn_id,
      payload.artifact_id,
      payload.title,
      payload.media_type,
    );
  }
  if (kind === "context.compacted") {
    const omitted = Number(payload.omitted_messages || 0);
    const truncated = Number(payload.truncated_messages || 0);
    const tokens = Number(payload.estimated_tokens || 0);
    const detail = [
      `${omitted} earlier message${omitted === 1 ? "" : "s"} summarized`,
      truncated ? `${truncated} long message${truncated === 1 ? "" : "s"} shortened` : "",
      tokens ? `about ${tokens.toLocaleString()} summary tokens` : "",
    ].filter(Boolean).join(" · ");
    renderTranscriptNotice(
      envelope.item_id || payload.item_id || envelope.id,
      envelope.turn_id,
      "context_compaction",
      "Context optimized",
      detail,
    );
  }
  if (kind === "model.rerouted") {
    const route = payload.from_model
      ? `${payload.from_model} → ${payload.to_model}`
      : String(payload.to_model || "fallback model");
    renderTranscriptNotice(
      envelope.item_id || envelope.id,
      envelope.turn_id,
      "model_reroute",
      "Model switched",
      `${route} · ${payload.reason || "routing policy"}`,
    );
  }
  if (kind === "reasoning.summary.delta") {
    const itemId = envelope.item_id || payload.item_id || envelope.id;
    const existing = state.itemsById.get(itemId);
    const prior = existing?.querySelector<HTMLElement>("span")?.textContent || "";
    renderTranscriptNotice(
      itemId,
      envelope.turn_id,
      "reasoning_summary",
      "Reasoning summary",
      `${prior}${String(payload.text || "")}`,
    );
  }
  if (kind === "turn.usage") {
    const inputTokens = Number(payload.input_tokens ?? payload.input_units ?? 0);
    const outputTokens = Number(payload.output_tokens ?? payload.output_units ?? 0);
    const modelCalls = Number(payload.model_calls || 0);
    const toolCalls = Number(payload.tool_calls || 0);
    state.usage.input_tokens += inputTokens;
    state.usage.output_tokens += outputTokens;
    state.usage.total_tokens = state.usage.input_tokens + state.usage.output_tokens;
    state.usage.model_calls += modelCalls;
    state.usage.tool_calls += toolCalls;
    if (envelope.turn_id && !state.usageTurns.has(envelope.turn_id)) {
      state.usageTurns.add(envelope.turn_id);
      state.usage.turns += 1;
    }
    renderTranscriptNotice(
      envelope.item_id || payload.item_id || envelope.id,
      envelope.turn_id,
      "usage",
      "Usage",
      `${(inputTokens + outputTokens).toLocaleString()} tokens · ${inputTokens.toLocaleString()} input + ${outputTokens.toLocaleString()} output · ${modelCalls} model / ${toolCalls} tool calls · ${String(payload.model || "model")}`,
    );
  }
  if (kind === "agent.status") {
    renderTranscriptNotice(
      envelope.item_id || payload.item_id || envelope.id,
      envelope.turn_id,
      "agent_status",
      "Agent",
      String(payload.label || payload.status || "updated"),
    );
  }
  if (kind.startsWith("hook.")) {
    const detail = [
      `${String(payload.event || "hook")} · ${String(payload.handler || "handler")}`,
      payload.input_modified ? "input modified" : "",
      payload.result_summary || payload.error_code || "",
    ].filter(Boolean).join(" · ");
    renderTranscriptNotice(
      envelope.item_id || payload.item_id || envelope.id,
      envelope.turn_id,
      "hook",
      "Hook",
      detail,
    );
  }
  if (kind === "model.delta") {
    const itemId = envelope.item_id || payload.item_id || null;
    let current = itemId ? state.itemsById.get(itemId) : null;
    if (!current?.classList.contains("streaming")) current = null;
    if (!current) {
      current = document.createElement("article");
      current.className = "message assistant streaming";
      if (itemId) current.dataset.itemId = itemId;
      if (envelope.turn_id) current.dataset.turnId = envelope.turn_id;
      current.setAttribute("aria-label", `${state.assistantAlias} response`);
      const label = document.createElement("div"); label.className = "message-label"; label.textContent = state.assistantAlias;
      const body = document.createElement("div"); body.className = "message-body";
      current.append(label, body);
      current.dataset.raw = "";
      $("messages").append(current);
      if (itemId) state.itemsById.set(itemId, current);
      updateConversationState(true);
    }
    current.dataset.raw = `${current.dataset.raw ?? ""}${String(payload.text ?? "")}`;
    renderMessageContent(current, current.dataset.raw);
    advanceTranscript();
  }
  if ([
    "turn.created",
    "turn.status",
    "turn.awaiting_input",
    "turn.awaiting_approval",
    "turn.completed",
    "turn.failed",
    "turn.cancelled",
  ].includes(kind)) {
    $("turn-state").textContent = payload.status || kind.slice(5);
  }
  if (kind === "turn.completed" || kind === "turn.failed" || kind === "turn.cancelled") {
    const itemId = envelope.item_id || payload.item_id || null;
    const item = itemId ? state.itemsById.get(itemId) : null;
    const current = item?.classList.contains("streaming")
      ? item
      : $("messages").querySelector<HTMLElement>(`.streaming[data-turn-id="${CSS.escape(envelope.turn_id || "")}"]`);
    if (current) {
      current.classList.remove("streaming");
      renderMessageContent(current, current.dataset.raw || "");
      delete current.dataset.raw;
    }
    if (kind === "turn.failed") renderMessage("assistant", `Agent failed: ${payload.error_code || "unknown error"}. Check daemon logs for the provider-safe diagnostic.`);
    if (envelope.turn_id === state.turn) {
      setTurnRunning(false);
      announce(activityLabel(kind));
      $("undo-turn").disabled = !hasWorkspace(state.session) || !state.capabilities.has("turn.undo");
    }
  }
  if (kind.startsWith("turn.input.")) {
    refreshPendingInputs().catch((error) => addActivity("turn.input.refresh.error", { error: error.message }));
  }
  if (kind === "session.goal.changed" && envelope.session_id === state.session?.id) {
    loadSessionGoal(envelope.session_id).catch((error) =>
      addActivity("session.goal.refresh.error", { error: error.message })
    );
  }
  if (kind === "session.preferences.updated" && envelope.session_id === state.session?.id) {
    loadSessionPreferences(envelope.session_id).catch((error) =>
      addActivity("session.preferences.refresh.error", { error: error.message })
    );
  }
  if (kind === "approval.required") {
    renderApproval(
      payload.approval_id,
      payload.approval_request || payload.display || payload.tool,
      envelope.turn_id,
    );
  }
  if (kind === "approval.resolved" && payload.approval_id) {
    const approvalId = String(payload.approval_id);
    const row = $("approvals").querySelector<HTMLElement>(`[data-id="${CSS.escape(approvalId)}"]`);
    row?.remove();
    state.approvals.delete(approvalId);
  }
  if (
    kind.startsWith("team.")
    || [
      "client.presence.updated",
      "client.presence.left",
      "client.remote_grant_revoked",
      "task.queued",
      "task.leased",
      "task.lease_renewed",
      "task.started",
      "task.running",
      "task.checkpointed",
      "task.paused",
      "task.resumed",
      "task.retry_scheduled",
      "task.cancel_requested",
      "task.completed",
      "task.failed",
      "task.cancelled",
      "agent.follow_up.queued",
      "agent.close_requested",
      "task.kill_switch.changed",
    ].includes(kind)
  ) refreshTeam();
}

function handleClientEvent(kind: string, value: unknown) {
  const envelope = parseClientEvent(value);
  const reduction = reduceClientEvent(transcriptProjection, envelope);
  transcriptProjection = reduction.state;
  state.after = Math.max(state.after, transcriptProjection.cursor);
  if (reduction.gap) {
    throw new Error(
      `event sequence gap: expected ${reduction.gap.expected}, received ${reduction.gap.received}`
    );
  }
  if (reduction.appendGap) {
    throw new Error(
      `transcript append gap for ${reduction.appendGap.itemId}: expected byte offset ${reduction.appendGap.expected}, received ${reduction.appendGap.received}`
    );
  }
  if (!reduction.accepted) return;
  if (
    !reduction.visible
    && !kind.startsWith("session.")
    && !kind.startsWith("terminal.")
    && !kind.startsWith("client.")
  ) return;
  const notification = envelope.notification;
  if (!notification || typeof notification.type !== "string") {
    handleEvent(
      kind,
      isJsonObject(envelope.payload) ? envelope.payload : {},
      envelope as unknown as JsonObject,
    );
    return;
  }
  const notificationItemId = "item_id" in notification ? notification.item_id : null;
  const notificationRequestId = "request_id" in notification ? notification.request_id : null;
  const eventPayload: JsonObject = isJsonObject(envelope.payload)
    ? envelope.payload as JsonObject
    : {};
  const typed: JsonObject = {
    ...envelope,
    item_id: notificationItemId || envelope.item_id,
    request_id: notificationRequestId || envelope.request_id,
  };
  switch (notification.type) {
    case "agent_message_delta":
      handleEvent("model.delta", {
        text: notification.delta,
        item_id: notification.item_id,
        byte_offset: notification.byte_offset,
      }, typed);
      break;
    case "turn_status_changed":
      handleEvent(kind, {
        status: notification.status,
        error_code: notification.error_code,
        item_id: eventPayload.item_id,
        source_input_id: eventPayload.source_input_id,
      }, typed);
      break;
    case "tool_call_changed":
      handleEvent(kind, {
        tool_call_id: notification.item_id,
        parent_tool_call_id: notification.parent_tool_call_id,
        model_call_id: notification.model_call_id,
        tool: notification.tool,
        display: notification.display,
        status: notification.status,
      }, typed);
      break;
    case "approval_requested":
      handleEvent("approval.required", {
        approval_id: notification.request_id,
        tool_call_id: notification.tool_item_id,
        model_call_id: notification.model_call_id,
        tool: notification.tool,
        display: notification.summary,
        approval_request: eventPayload.approval_request,
      }, typed);
      break;
    case "question_requested":
      handleEvent("question.required", {
        request_id: notification.request_id,
        item_id: notification.item_id,
        questions: notification.questions,
        allow_other: eventPayload.allow_other !== false,
      }, typed);
      break;
    case "artifact_created":
      handleEvent("artifact.created", {
        item_id: notification.item_id,
        artifact_id: notification.artifact_id,
        title: notification.title,
        media_type: notification.media_type,
      }, typed);
      break;
    case "turn_input_changed":
      handleEvent(kind, {
        input_id: notification.input_id,
        target_turn_id: notification.target_turn_id,
        resulting_turn_id: notification.resulting_turn_id,
        mode: notification.mode,
        status: notification.status,
      }, typed);
      break;
    case "plan_updated":
      handleEvent("plan.updated", {
        item_id: notification.item_id,
        title: notification.title,
        steps: notification.steps,
        status: envelope.status,
      }, typed);
      break;
    case "context_compacted":
      handleEvent("context.compacted", {
        item_id: notification.item_id,
        omitted_messages: notification.omitted_messages,
        truncated_messages: notification.truncated_messages,
        estimated_tokens: notification.estimated_tokens,
      }, typed);
      break;
    case "model_rerouted":
      handleEvent("model.rerouted", {
        from_model: notification.from_model,
        to_model: notification.to_model,
        reason: notification.reason,
      }, typed);
      break;
    case "reasoning_summary_delta":
      handleEvent("reasoning.summary.delta", {
        item_id: notification.item_id,
        text: notification.delta,
      }, typed);
      break;
    case "mcp_progress_changed":
      handleEvent("mcp.progress", {
        item_id: notification.item_id,
        tool_call_id: notification.item_id,
        server: notification.server,
        tool: notification.tool,
        progress: notification.progress,
        total: notification.total,
        message: notification.message,
      }, typed);
      break;
    case "usage_recorded":
      handleEvent("turn.usage", {
        item_id: notification.item_id,
        model: notification.model,
        input_tokens: notification.input_tokens,
        output_tokens: notification.output_tokens,
        total_tokens: notification.total_tokens,
        model_calls: notification.model_calls,
        tool_calls: notification.tool_calls,
      }, typed);
      break;
    case "durable_task_changed":
      handleEvent(kind, {
        durable_task_id: notification.task_id,
        status: notification.status,
        attempt: notification.attempt,
        consumed_cost_micros: notification.consumed_cost_micros,
        consumed_runner_cost_micros: notification.consumed_runner_cost_micros,
      }, typed);
      break;
    case "agent_run_changed":
      handleEvent("agent.status", {
        item_id: notification.item_id,
        agent_id: notification.agent_id,
        parent_agent_id: notification.parent_agent_id,
        status: notification.status,
        label: notification.label,
        attempt: notification.attempt,
        consumed_cost_micros: notification.consumed_cost_micros,
        consumed_runner_cost_micros: notification.consumed_runner_cost_micros,
      }, typed);
      break;
    case "hook_changed":
      handleEvent(kind, {
        ...eventPayload,
        item_id: notification.item_id,
        event: notification.event,
        handler: notification.handler,
        status: notification.status,
        input_modified: notification.input_modified,
      }, typed);
      break;
    case "background_terminal_changed":
      handleEvent(kind, {
        terminal_id: notification.terminal_id,
        status: notification.status,
        output_byte_length: notification.output_byte_length,
        output_truncated: notification.output_truncated,
        exit_code: notification.exit_code,
        artifact_id: notification.artifact_id,
        revision: notification.revision,
      }, typed);
      break;
    default:
      handleEvent(kind, eventPayload, envelope as unknown as JsonObject);
  }
}

async function subscribe(generation = state.generation) {
  if (generation !== state.generation) return;
  if (state.abort) state.abort.abort(); state.abort = new AbortController();
  const controller = state.abort;
  const team = scope().team_id;
  try {
    const s = scope();
    const query = new URLSearchParams({ organization_id: s.organization_id, team_id: team, actor_id: s.actor_id, after: String(state.after) });
    const response = await fetch(`/v1/events?${query}`, { credentials: "same-origin", signal: controller.signal, cache: "no-store" });
    if (!response.ok || !response.body) throw new Error(`events ${response.status}`);
    if (generation !== state.generation) { controller.abort(); return; }
    if (!state.connected) {
      const capabilities = negotiateCapabilities(await api("/v1/capabilities", { allowDisconnected: true, signal: controller.signal }));
      if (generation !== state.generation) { controller.abort(); return; }
      state.capabilities = capabilities;
      state.authenticatedScope = formScope();
      switchComposerAccount(state.authenticatedScope);
      setConnection(true);
      await Promise.all([refreshSessions(), refreshTeam()]);
      await restoreRoute();
    }
    const reader = response.body.getReader(); const decoder = new TextDecoder(); let buffer = "";
    while (true) {
      const { value, done } = await reader.read(); if (done) throw new Error("event stream closed"); if (generation !== state.generation) { await reader.cancel(); return; } buffer += decoder.decode(value, { stream: true });
      let boundary; while ((boundary = buffer.indexOf("\n\n")) >= 0) { const block = buffer.slice(0, boundary); buffer = buffer.slice(boundary + 2); let kind = "message", data = ""; block.split("\n").forEach((line) => { if (line.startsWith("event:")) kind = line.slice(6).trim(); if (line.startsWith("data:")) data += line.slice(5).trim(); }); if (data && generation === state.generation) handleClientEvent(kind, JSON.parse(data)); }
    }
  } catch (error) {
    if (error.name !== "AbortError" && generation === state.generation) {
      setConnection(false, "reconnecting");
      const retryGeneration = state.generation;
      state.reconnectTimer = setTimeout(() => { state.reconnectTimer = null; if (retryGeneration === state.generation) subscribe(retryGeneration); }, 1200);
    }
  }
}

loadSettings();
applyTheme();
updateNotificationControls();
sessionStorage.removeItem("oc.permission-mode");
updateContextChips();
updateConversationState(false);
if (sessionStorage.getItem("oc.sidebar-collapsed") === "true") document.body.classList.add("sidebar-collapsed");
syncHistoryAccessibility();
historyVisibilityMedia.addEventListener("change", syncHistoryAccessibility);
new MutationObserver(syncHistoryAccessibility).observe(document.body, {
  attributes: true,
  attributeFilter: ["class"],
});
// Unscoped legacy drafts cannot be safely assigned to the current account.
sessionStorage.removeItem("oc.prompt-draft");
sessionStorage.removeItem("oc.prompt-draft:new");
restoreComposerDraft();
fields.forEach((id) => $(id).addEventListener("input", () => { settingsDirty = true; updateContextChips(); }));
identityFields.forEach((id) => $(id).addEventListener("input", () => {
  if (state.reconnectTimer) { clearTimeout(state.reconnectTimer); state.reconnectTimer = null; }
  if (state.abort) state.abort.abort();
  setConnection(false);
  switchComposerAccount(formScope());
}));
$("prompt").addEventListener("input", () => {
  const value = $("prompt").value;
  if (composerAccountEstablished) {
    if (value) sessionStorage.setItem(composerTextDraftKey(), value);
    else sessionStorage.removeItem(composerTextDraftKey());
  }
  resizePrompt();
  if ($("prompt").value === "/") {
    $("prompt").value = "";
    sessionStorage.removeItem(composerTextDraftKey());
    resizePrompt();
    openCommands();
  }
  scheduleFileMentions();
  updateSendAction();
});
$("prompt").addEventListener("paste", (event: ClipboardEvent) => {
  const files = event.clipboardData?.files;
  if (!files?.length) return;
  event.preventDefault();
  addDraftFiles(files);
  announce(`${files.length} pasted file${files.length === 1 ? "" : "s"} attached`);
});
$("prompt").addEventListener("keydown", (event) => {
  if (!$("mention-menu").hidden) {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      updateMentionSelection(mentionSelection + (event.key === "ArrowDown" ? 1 : -1));
      return;
    }
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      const selected = $("mention-menu").querySelectorAll("button")[mentionSelection];
      if (selected) selectMention(selected.textContent);
      return;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      closeMentionMenu();
      return;
    }
  }
  if (event.key === "Enter" && !event.shiftKey && !event.isComposing) { event.preventDefault(); $("prompt-form").requestSubmit(); }
});
$("connection-form").addEventListener("submit", (event) => { event.preventDefault(); connect(); });
$("notification-mode").addEventListener("change", () => {
  const mode = $("notification-mode").value;
  localStorage.setItem(
    "oc.notification-mode",
    mode === "background" || mode === "always" ? mode : "off",
  );
  updateNotificationControls();
});
$("enable-notifications").addEventListener("click", () => {
  enableNotifications().catch((error) => toast(error.message));
});
$("refresh").addEventListener("click", () => refreshSessions().catch((error) => addActivity("session.refresh.error", { error: error.message })));
$("session-search").addEventListener("input", () => {
  sessionWindowStart = 0;
  renderSessions();
});
$("session-filter").addEventListener("change", () => {
  sessionWindowStart = 0;
  renderSessions();
});
$("messages").addEventListener("scroll", () => {
  transcriptFollowing = transcriptIsNearBottom();
  if (transcriptFollowing) $("jump-latest").hidden = true;
});
$("jump-latest").addEventListener("click", () => advanceTranscript(true));
$("load-earlier").addEventListener("click", () => {
  loadEarlierTranscript().catch((error) => toast(error.message));
});
$("refresh-team").addEventListener("click", refreshTeam);
$("new-goal").addEventListener("click", () => createGoal().catch((error) => addActivity("goal.error", { error: error.message })));
$("new-task").addEventListener("click", () => createTask().catch((error) => addActivity("task.error", { error: error.message })));
$("set-capacity").addEventListener("click", () => setCapacity().catch((error) => addActivity("capacity.error", { error: error.message })));
$("new-ownership").addEventListener("click", () => createOwnership().catch((error) => addActivity("ownership.error", { error: error.message })));
$("new-budget").addEventListener("click", () => createBudget().catch((error) => addActivity("budget.error", { error: error.message })));
$("new-background-task").addEventListener("click", () => createBackgroundTask().catch((error) => addActivity("task.background.error", { error: error.message })));
$("new-background-terminal").addEventListener("click", () => createBackgroundTerminal().catch((error) => addActivity("terminal.background.error", { error: error.message })));
$("create-session").addEventListener("click", createSession);
$("choose-chat").addEventListener("click", () => chooseNewConversationMode("chat"));
$("choose-work").addEventListener("click", () => chooseNewConversationMode("work"));
$("start-work").addEventListener("click", startWork);
$("prompt-form").addEventListener("submit", runTurn);
$("steer-turn").addEventListener("click", async () => {
  const content = $("prompt").value.trim();
  if (!content || !state.turnRunning || composerSubmissionPending) return;
  await withComposerSubmission(async () => {
    try {
      await submitTurnInput(content, "steer");
    } catch (error) {
      addActivity("turn.input.steer.error", { error: error.message });
    }
  });
});
$("show-diff").addEventListener("click", () => showDiff().catch((error) => toast(error.message)));
$("show-context").addEventListener("click", showContext);
$("review-session").addEventListener("click", startReview);
$("quick-diff").addEventListener("click", () => { openDrawer("inspector"); showDiff().catch((error) => toast(error.message)); });
$("undo-turn").addEventListener("click", () => undoTurn());
$("show-checkpoints").addEventListener("click", showCheckpoints);
$("fork-session").addEventListener("click", forkSession);
$("promote-side-conversation").addEventListener("click", () => {
  promoteSideConversation().catch((error) => toast(error.message));
});
$("close-side-conversation").addEventListener("click", () => {
  closeSideConversation().catch((error) => toast(error.message));
});
$("show-branches").addEventListener("click", showBranches);
$("export-session").addEventListener("click", exportSession);
$("rename-session").addEventListener("click", renameSession);
$("rename-assistant").addEventListener("click", () => {
  renameAssistant().catch((error) => toast(error.message));
});
$("edit-session-goal").addEventListener("click", () => {
  editSessionGoal().catch((error) => toast(error.message));
});
$("toggle-session-goal").addEventListener("click", () => {
  toggleSessionGoal().catch((error) => toast(error.message));
});
$("clear-session-goal").addEventListener("click", () => {
  clearSessionGoal().catch((error) => toast(error.message));
});
$("cancel-session").addEventListener("click", () => closeSession(false));
$("delete-session").addEventListener("click", () => closeSession(true));
$("new-chat").addEventListener("click", () => { clearSessionSelection(); document.body.classList.remove("mobile-sidebar-open"); });
$("toggle-sidebar").addEventListener("click", toggleHistory);
$("open-sidebar").addEventListener("click", toggleHistory);
$("sidebar-scrim").addEventListener("click", () => {
	document.body.classList.remove("mobile-sidebar-open");
	closeUserMenu();
});
$("toggle-inspector").addEventListener("click", () => $("inspector").classList.contains("open") ? closeDrawers() : openDrawer("inspector"));
$("close-inspector").addEventListener("click", () => closeDrawers());
$("open-settings").addEventListener("click", () => openDrawer("settings-drawer"));
$("open-projects").addEventListener("click", () => showProjects());
$("open-artifacts").addEventListener("click", () => showArtifacts());
$("open-extensions").addEventListener("click", () => showExtensions());
$("add-mcp-server").addEventListener("click", () => {
  addMcpServer().catch((error) => toast(error.message));
});
$("add-skill").addEventListener("click", () => {
  addSkill().catch((error) => toast(error.message));
});
$("add-hook").addEventListener("click", () => {
  addHook().catch((error) => toast(error.message));
});
$("manage-marketplaces").addEventListener("click", () => {
  managePluginMarketplaces().catch((error) => toast(error.message));
});
$("refresh-extensions").addEventListener("click", () => {
  loadExtensionCatalog().catch((error) => toast(error.message));
});
$("extension-filter").addEventListener("change", renderExtensionList);
$("extension-search").addEventListener("input", renderExtensionList);
$("refresh-artifacts").addEventListener("click", () => {
  const route = parseRoute(window.location.pathname);
  loadArtifactPage(true, route.type === "artifact" ? route.artifactId : null)
    .catch((error) => toast(error.message));
});
$("artifact-filter").addEventListener("change", renderArtifactList);
$("load-more-artifacts").addEventListener("click", () => {
  loadMoreArtifacts().catch((error) => toast(error.message));
});
$("composer-settings").addEventListener("click", () => {
  if (state.turnRunning) {
    toast("Wait for the current Turn to finish before attaching files.");
    return;
  }
  $("attachment-input").click();
});
$("attachment-input").addEventListener("change", () => {
  const files = $("attachment-input").files;
  if (files) addDraftFiles(files);
  $("attachment-input").value = "";
});
$("workspace-chip").addEventListener("click", () => { if (state.session?.workspace_uri) copyText(state.session.workspace_uri, "Working directory copied"); });
$("model-chip").addEventListener("click", () => openModelPicker().catch((error) => toast(error.message)));
$("empty-connect").addEventListener("click", () => openDrawer("settings-drawer"));
$("retry-connection").addEventListener("click", connect);
$("offline-diagnostics").addEventListener("click", () => openDrawer("settings-drawer"));
$("close-settings").addEventListener("click", () => closeDrawers());
$("drawer-scrim").addEventListener("click", () => closeDrawers());
$("open-team").addEventListener("click", () => showTeam());
document.querySelectorAll<HTMLButtonElement>(".mobile-open-sidebar").forEach((button) => {
  button.addEventListener("click", toggleHistory);
});
$("projects-new-task").addEventListener("click", () => {
  clearSessionSelection();
  chooseNewConversationMode("work");
  showWorkspace();
});
$("team-new-task-primary").addEventListener("click", () => createTask().catch((error) => addActivity("task.error", { error: error.message })));
$("audit-filter-form").addEventListener("submit", (event) => {
  event.preventDefault();
  refreshTeam().catch((error) => toast(error.message));
});
$("clear-audit-filters").addEventListener("click", () => {
  [
    "audit-filter-actor",
    "audit-filter-action",
    "audit-filter-session",
    "audit-filter-since",
    "audit-filter-until",
  ].forEach((id) => { $(id).value = ""; });
  refreshTeam().catch((error) => toast(error.message));
});
$("export-team-audit").addEventListener("click", () => {
  exportTeamAudit().catch((error) => toast(error.message));
});
$("theme-toggle").addEventListener("click", cycleTheme);
$("open-diagnostics").addEventListener("click", () => { closeUserMenu(); openDrawer("settings-drawer"); $("connection").scrollIntoView({ block: "center" }); });
$("permission-chip").addEventListener("click", () => openPermissionPicker().catch((error) => toast(error.message)));
$("close-context-picker").addEventListener("click", () => $("context-picker-dialog").close());
$("context-picker-query").addEventListener("input", () => {
  contextPickerSelection = 0;
  renderContextPicker();
});
$("context-picker-query").addEventListener("keydown", (event) => {
  const options = renderContextPicker();
  if ((event.key === "ArrowDown" || event.key === "ArrowUp") && options.length) {
    event.preventDefault();
    const direction = event.key === "ArrowDown" ? 1 : -1;
    contextPickerSelection = (contextPickerSelection + direction + options.length) % options.length;
    renderContextPicker();
    document.getElementById(`context-picker-option-${contextPickerSelection}`)?.scrollIntoView({ block: "nearest" });
  } else if (event.key === "Enter" && options[contextPickerSelection]) {
    event.preventDefault();
    const option = options[contextPickerSelection];
    if (!option.disabled) {
      Promise.resolve(option.select())
        .then(() => $("context-picker-dialog").close())
        .catch((error) => toast(error.message));
    }
  }
});
$("open-command").addEventListener("click", openCommands);
$("open-shortcuts-inline").addEventListener("click", showShortcuts);
$("command-query").addEventListener("input", () => { commandSelection = 0; renderCommands(); });
$("command-query").addEventListener("keydown", (event) => {
  const commands = renderCommands();
  if ((event.key === "ArrowDown" || event.key === "ArrowUp") && commands.length) {
    event.preventDefault();
    const direction = event.key === "ArrowDown" ? 1 : -1;
    commandSelection = (commandSelection + direction + commands.length) % commands.length;
    renderCommands();
    $(`command-option-${commandSelection}`)?.scrollIntoView({ block: "nearest" });
  } else if (event.key === "Enter" && commands[commandSelection]) {
    event.preventDefault();
    const command = commands[commandSelection];
    if (!command.enabled || command.enabled()) runCommand(command);
  }
});
$("action-form").addEventListener("submit", submitActionDialog);
$("close-action").addEventListener("click", () => closeActionDialog(null));
$("cancel-action").addEventListener("click", () => closeActionDialog(null));
$("action-dialog").addEventListener("cancel", (event) => { event.preventDefault(); closeActionDialog(null); });
document.querySelector<HTMLElement>(".dialog-close")?.addEventListener("click", () => $("shortcuts-dialog").close());
$("shortcuts-dialog").addEventListener("keydown", (event) => {
  if (event.key === "Tab") {
    event.preventDefault();
    document.querySelector<HTMLElement>(".dialog-close")?.focus();
  }
});
document.querySelectorAll<HTMLElement>(".starter-actions button").forEach((button) => button.addEventListener("click", () => {
  $("prompt").value = button.dataset.prompt || "";
  sessionStorage.setItem(composerTextDraftKey(), $("prompt").value);
  resizePrompt();
  $("prompt").focus();
  announce("Suggested task added. Edit it or press Enter to run.");
}));
document.querySelector<HTMLElement>(".brand")?.addEventListener("click", (event) => { event.preventDefault(); clearSessionSelection(); });
document.querySelectorAll<HTMLElement>(".team-tabs button").forEach((button) => button.addEventListener("click", () => {
  const section = button.dataset.teamSection as TeamSection | undefined;
  if (!section) return;
  selectTeamSection(section, "smooth");
  routePath(teamRoute(section));
}));
document.addEventListener("keydown", (event) => {
  const commandKey = event.metaKey || event.ctrlKey;
  if (commandKey && event.key.toLowerCase() === "k") { event.preventDefault(); openCommands(); return; }
  if (commandKey && event.key.toLowerCase() === "n") { event.preventDefault(); clearSessionSelection(); return; }
  if (commandKey && event.key.toLowerCase() === "l") { event.preventDefault(); $("prompt").focus(); return; }
  if (commandKey && event.key.toLowerCase() === "d") { event.preventDefault(); if (state.session) { openDrawer("inspector"); showDiff().catch((error) => toast(error.message)); } return; }
  if (commandKey && event.key.toLowerCase() === "b") { event.preventDefault(); toggleHistory(); return; }
  if (commandKey && event.key.toLowerCase() === "f") { event.preventDefault(); focusSessionSearch(); return; }
  if (commandKey && event.key === "/") { event.preventDefault(); showShortcuts(); return; }
  if (event.key === "Escape") { closeDrawers(); document.body.classList.remove("mobile-sidebar-open"); }
});
window.addEventListener("popstate", () => { restoreRoute().catch((error) => toast(error.message)); });
["focus", "blur"].forEach((eventName) => window.addEventListener(eventName, () => {
  updateClientPresence().catch(() => {});
}));
document.addEventListener("visibilitychange", () => {
  updateClientPresence().catch(() => {});
});
presenceTimer = window.setInterval(() => {
  updateClientPresence().catch(() => {});
}, 15_000);
window.addEventListener("pagehide", () => { if (state.reconnectTimer) clearTimeout(state.reconnectTimer); if (developmentReloadTimer) clearInterval(developmentReloadTimer); if (presenceTimer) clearInterval(presenceTimer); if (state.abort) state.abort.abort(); setConnection(false); });
enableDevelopmentAutoReload();
bootstrapBrowserSession()
  .then(() => connect())
  .catch((error) => setConnection(false, error.message));
resizePrompt();
restoreRoute().catch((error) => toast(error.message));
