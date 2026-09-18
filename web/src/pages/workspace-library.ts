import { hasWorkspace } from "../models/session-mode";
import type { ApiRequestOptions } from "../api/client";
import type { Artifact, ArtifactIndexEntry, ArtifactPage, Scope, Session } from "../models/protocol";
import { artifactRoute, projectRoute } from "../router";

interface LibraryElement extends HTMLElement {
  value: string;
}

interface ViewOptions {
  replace?: boolean;
  updateRoute?: boolean;
}

interface ContextSummaryResponse {
  items: Array<{
    kind: string;
    source_uri: string;
    estimated_tokens: number;
    trust_level: string;
  }>;
}

export interface WorkspaceLibraryContext {
  lookup(id: string): LibraryElement;
  api<T = unknown>(path: string, options?: ApiRequestOptions): Promise<T>;
  state: { connected: boolean; generation: number; sessions: Session[] };
  catalogQuery(): URLSearchParams;
  isCurrent(generation: number): boolean;
  selectSession(session: Session): Promise<void>;
  closeDrawers(restoreFocus?: boolean): void;
  closeUserMenu(): void;
  routePath(path: string, replace?: boolean): void;
  workspaceName(uri: string): string;
  copyText(text: string, success?: string): Promise<void>;
  toast(message: string): void;
  announce(message: string): void;
  appendMarkdownBlocks(target: HTMLElement, text: unknown): void;
  createCodeBlock(code: string, language?: string): HTMLElement;
  renderReviewReport(target: HTMLElement, value: unknown): void;
}

export function createWorkspaceLibrary(context: WorkspaceLibraryContext) {
  const {
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
  } = context;
  let artifactEntries: ArtifactIndexEntry[] = [];
  let artifactNextCursor: string | null = null;
  let selectedArtifactId: string | null = null;

  function resetAccountLibrary() {
    artifactEntries = [];
    artifactNextCursor = null;
    selectedArtifactId = null;
    $("project-list").replaceChildren();
    $("project-detail").textContent = "Connect to inspect projects.";
    $("artifact-list").replaceChildren();
    $("artifact-detail").replaceChildren();
    $("artifact-detail").hidden = true;
    $("artifact-count").textContent = "0 results";
    $("load-more-artifacts").hidden = true;
  }

  function projectGroups() {
    const byWorkspace = new Map<string, Session[]>();
    state.sessions.forEach((session) => {
      if (!hasWorkspace(session)) return;
      const workspace = session.workspace_uri;
      const group = byWorkspace.get(workspace) || [];
      group.push(session);
      byWorkspace.set(workspace, group);
    });
    return [...byWorkspace.entries()]
      .map(([workspace, sessions]) => ({
        id: sessions[0].id,
        workspace,
        sessions: sessions.sort((left, right) =>
          String(right.updated_at || "").localeCompare(String(left.updated_at || ""))),
      }))
      .sort((left, right) => workspaceName(left.workspace).localeCompare(workspaceName(right.workspace)));
  }

  async function renderProjects(selectedProjectId: string | null = null) {
    const groups = projectGroups();
    const selected = groups.find((group) => group.id === selectedProjectId) || groups[0] || null;
    const list = $("project-list");
    list.replaceChildren();
    groups.forEach((group) => {
      const button = document.createElement("button");
      button.type = "button";
      button.classList.toggle("active", group === selected);
      if (group === selected) button.setAttribute("aria-current", "page");
      const title = document.createElement("strong");
      title.textContent = workspaceName(group.workspace);
      const detail = document.createElement("small");
      detail.textContent = `${group.sessions.length} session${group.sessions.length === 1 ? "" : "s"} · ${group.workspace}`;
      button.append(title, detail);
      button.addEventListener("click", () => showProjects(group.id));
      list.append(button);
    });
    const detail = $("project-detail");
    detail.replaceChildren();
    if (!selected) {
      detail.className = "project-detail empty";
      detail.textContent = "No projects yet.";
      return;
    }
    detail.className = "project-detail";
    const title = document.createElement("h3");
    title.textContent = workspaceName(selected.workspace);
    const uri = document.createElement("p");
    uri.textContent = selected.workspace;
    const contextHeading = document.createElement("strong");
    contextHeading.textContent = "Context sources";
    const context = document.createElement("div");
    context.className = "project-context";
    context.textContent = "Loading context…";
    const sessionsHeading = document.createElement("strong");
    sessionsHeading.textContent = "Recent sessions";
    const sessions = document.createElement("div");
    sessions.className = "project-sessions";
    selected.sessions.forEach((session) => {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "project-session";
      const name = document.createElement("span");
      name.textContent = session.title;
      const meta = document.createElement("small");
      meta.textContent = `${session.status} · ${session.model}`;
      button.append(name, meta);
      button.addEventListener("click", () => selectSession(session));
      sessions.append(button);
    });
    detail.append(title, uri, contextHeading, context, sessionsHeading, sessions);
    if (!state.connected) {
      context.textContent = "Connect to inspect AGENTS.md, rules, memory, and token usage.";
      return;
    }
    const generation = state.generation;
    try {
      const summary = await api<ContextSummaryResponse>(
        `/v1/sessions/${encodeURIComponent(selected.sessions[0].id)}/context?${catalogQuery()}`,
      );
      if (!isCurrent(generation) || $("project-detail") !== detail) return;
      context.replaceChildren();
      if (!summary.items.length) {
        context.textContent = "No project instructions, memory, attachments, or IDE context.";
      } else {
        summary.items.forEach((item) => {
          const row = document.createElement("article");
          const source = document.createElement("span");
          source.textContent = item.source_uri;
          const meta = document.createElement("small");
          meta.textContent = `${String(item.kind).replaceAll("_", " ")} · ${item.estimated_tokens} tokens · ${item.trust_level}`;
          row.append(source, meta);
          context.append(row);
        });
      }
    } catch (error) {
      if (isCurrent(generation)) context.textContent = `Context unavailable: ${error.message}`;
    }
  }

  function showProjects(
    projectId: string | null = null,
    { replace = false, updateRoute = true }: ViewOptions = {},
  ) {
    closeDrawers(false);
    document.body.classList.add("team-mode");
    $("team-view").hidden = true;
    $("projects-view").hidden = false;
    $("artifacts-view").hidden = true;
    $("extensions-view").hidden = true;
    $("workspace-shell").hidden = true;
    $("open-team").classList.remove("active");
    $("open-team").removeAttribute("aria-current");
    document.body.classList.remove("mobile-sidebar-open");
    closeUserMenu();
    const selected = projectGroups().find((group) => group.id === projectId)?.id
      || projectGroups()[0]?.id
      || null;
    if (updateRoute) routePath(projectRoute(selected), replace);
    $("projects-view").scrollTop = 0;
    renderProjects(selected).catch((error) => toast(error.message));
    $("projects-title").focus({ preventScroll: true });
  }

  function artifactContentText(artifact: Artifact): string {
    return typeof artifact.content === "string"
      ? artifact.content
      : JSON.stringify(artifact.content, null, 2);
  }

  function artifactFileName(artifact: Artifact): string {
    const safe = artifact.metadata.title
      .normalize("NFKC")
      .replace(/[^\p{L}\p{N}._-]+/gu, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 80) || "artifact";
    const extension = artifact.metadata.media_type === "text/markdown" ? "md"
      : artifact.metadata.media_type.includes("json") ? "json"
        : artifact.metadata.media_type.startsWith("image/") ? artifact.metadata.media_type.slice(6)
          : artifact.metadata.media_type === "application/pdf" ? "pdf" : "txt";
    return safe.includes(".") ? safe : `${safe}.${extension}`;
  }

  function downloadArtifact(artifact: Artifact) {
    const blob = new Blob([artifactContentText(artifact)], {
      type: `${artifact.metadata.media_type};charset=utf-8`,
    });
    const href = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = href;
    link.download = artifactFileName(artifact);
    link.click();
    URL.revokeObjectURL(href);
  }

  function renderArtifactLibraryContent(target: HTMLElement, artifact: Artifact) {
    target.replaceChildren();
    if (artifact.metadata.media_type === "application/vnd.s-code.review+json") {
      renderReviewReport(target, artifact.content);
      return;
    }
    if (artifact.metadata.media_type === "text/markdown" && typeof artifact.content === "string") {
      const body = document.createElement("div");
      body.className = "message-body";
      appendMarkdownBlocks(body, artifact.content);
      target.append(body);
      return;
    }
    if (
      artifact.metadata.media_type.startsWith("image/")
      && typeof artifact.content === "string"
      && artifact.content.startsWith(`data:${artifact.metadata.media_type};base64,`)
    ) {
      const image = document.createElement("img");
      image.src = artifact.content;
      image.alt = artifact.metadata.title;
      target.append(image);
      return;
    }
    target.append(createCodeBlock(
      artifactContentText(artifact),
      artifact.metadata.media_type.includes("json") ? "json" : "text",
    ));
  }

  async function showArtifactDetail(
    artifactId: string,
    entry: ArtifactIndexEntry | null = null,
  ) {
    const generation = state.generation;
    selectedArtifactId = artifactId;
    renderArtifactList();
    const target = $("artifact-detail");
    target.hidden = false;
    target.closest(".artifact-layout")?.classList.remove("is-empty");
    target.replaceChildren();
    const loading = document.createElement("p");
    loading.className = "empty";
    loading.textContent = "Loading artifact…";
    target.append(loading);
    try {
      const artifact = await api<Artifact>(
        `/v1/artifacts/${encodeURIComponent(artifactId)}?${catalogQuery()}`,
      );
      if (!isCurrent(generation) || $("artifacts-view").hidden || selectedArtifactId !== artifactId) return;
      const source = entry || artifactEntries.find((candidate) => candidate.metadata.id === artifactId) || null;
      target.replaceChildren();
      const heading = document.createElement("div");
      heading.className = "artifact-library-heading";
      const title = document.createElement("h3");
      title.textContent = artifact.metadata.title;
      const meta = document.createElement("p");
      meta.textContent = [
        artifact.metadata.media_type,
        source?.session_title || artifact.metadata.session_id,
        source?.workspace_uri,
        new Date(artifact.metadata.created_at).toLocaleString(),
      ].filter(Boolean).join(" · ");
      heading.append(title, meta);
      const actions = document.createElement("div");
      actions.className = "artifact-library-actions";
      const copy = document.createElement("button");
      copy.type = "button";
      copy.textContent = "Copy";
      copy.addEventListener("click", () => copyText(artifactContentText(artifact), "Artifact copied"));
      const download = document.createElement("button");
      download.type = "button";
      download.textContent = "Download";
      download.addEventListener("click", () => downloadArtifact(artifact));
      actions.append(copy, download);
      const sourceSession = state.sessions.find(
        (session) => session.id === artifact.metadata.session_id,
      );
      if (sourceSession) {
        const openSession = document.createElement("button");
        openSession.type = "button";
        openSession.textContent = "Open source session";
        openSession.addEventListener("click", () => {
          selectSession(sourceSession).catch((error) => toast(error.message));
        });
        actions.prepend(openSession);
      }
      const content = document.createElement("div");
      content.className = "artifact-library-content";
      renderArtifactLibraryContent(content, artifact);
      target.append(heading, actions, content);
      announce(`Opened artifact ${artifact.metadata.title}`);
    } catch (error) {
      if (isCurrent(generation) && selectedArtifactId === artifactId) {
        target.textContent = `Artifact unavailable: ${error.message}`;
      }
    }
  }

  function renderArtifactList() {
    const kind = $("artifact-filter").value;
    const visible = artifactEntries.filter((entry) => kind === "all" || entry.kind === kind);
    const list = $("artifact-list");
    const catalogEmpty = artifactEntries.length === 0;
    list.replaceChildren();
    list.classList.toggle("empty", !visible.length);
    list.closest(".artifact-layout")?.classList.toggle("is-empty", catalogEmpty);
    $("artifact-detail").hidden = catalogEmpty;
    $("artifact-count").textContent = `${visible.length} result${visible.length === 1 ? "" : "s"}${artifactNextCursor ? " loaded" : ""}`;
    if (!visible.length) {
      list.textContent = artifactEntries.length
        ? "No loaded artifacts match this type."
        : state.connected
          ? "No artifacts yet. Reports, diffs, and larger results will appear here."
          : "Connect to inspect artifacts.";
    }
    visible.forEach((entry) => {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "artifact-row";
      button.classList.toggle("active", entry.metadata.id === selectedArtifactId);
      const heading = document.createElement("span");
      const title = document.createElement("strong");
      title.textContent = entry.metadata.title;
      const badge = document.createElement("span");
      badge.className = "artifact-kind";
      badge.textContent = entry.kind.replaceAll("_", " ");
      heading.append(title, badge);
      const session = document.createElement("small");
      session.textContent = `${entry.session_title} · ${workspaceName(entry.workspace_uri)}`;
      const created = document.createElement("small");
      created.textContent = new Date(entry.metadata.created_at).toLocaleString();
      button.append(heading, session, created);
      button.addEventListener("click", () => {
        routePath(artifactRoute(entry.metadata.id));
        showArtifactDetail(entry.metadata.id, entry).catch((error) => toast(error.message));
      });
      list.append(button);
    });
    $("load-more-artifacts").hidden = !artifactNextCursor;
  }

  async function loadArtifactPage(reset = false, requestedId: string | null = null) {
    const generation = state.generation;
    if (!state.connected) {
      artifactEntries = [];
      artifactNextCursor = null;
      renderArtifactList();
      $("artifact-detail").textContent = "Connect to inspect artifacts.";
      return;
    }
    if (reset) {
      artifactEntries = [];
      artifactNextCursor = null;
    }
    const query = new URLSearchParams(catalogQuery());
    query.set("limit", "50");
    if (!reset && artifactNextCursor) query.set("cursor", artifactNextCursor);
    const page = await api<ArtifactPage>(`/v1/artifacts?${query}`);
    if (!isCurrent(generation)) return;
    const known = new Set(artifactEntries.map((entry) => entry.metadata.id));
    artifactEntries.push(...page.artifacts.filter((entry) => !known.has(entry.metadata.id)));
    artifactNextCursor = page.next_cursor;
    renderArtifactList();
    const selected = requestedId
      ? artifactEntries.find((entry) => entry.metadata.id === requestedId) || null
      : artifactEntries[0] || null;
    if (requestedId || selected) {
      const artifactId = requestedId || selected!.metadata.id;
      if (!requestedId) routePath(artifactRoute(artifactId), true);
      await showArtifactDetail(artifactId, selected);
    } else {
      selectedArtifactId = null;
      const empty = document.createElement("p");
      empty.className = "empty";
      empty.textContent = "Select an artifact to inspect it.";
      $("artifact-detail").replaceChildren(empty);
    }
  }

  function showArtifacts(
    artifactId: string | null = null,
    { replace = false, updateRoute = true } = {},
  ) {
    closeDrawers(false);
    document.body.classList.add("team-mode");
    $("team-view").hidden = true;
    $("projects-view").hidden = true;
    $("artifacts-view").hidden = false;
    $("extensions-view").hidden = true;
    $("workspace-shell").hidden = true;
    $("open-team").classList.remove("active");
    $("open-team").removeAttribute("aria-current");
    document.body.classList.remove("mobile-sidebar-open");
    closeUserMenu();
    selectedArtifactId = artifactId;
    if (updateRoute) routePath(artifactRoute(artifactId), replace);
    $("artifacts-view").scrollTop = 0;
    loadArtifactPage(true, artifactId).catch((error) => toast(error.message));
    $("artifacts-title").focus({ preventScroll: true });
  }


  const loadMoreArtifacts = () => loadArtifactPage(false, selectedArtifactId);
  return {
    loadArtifactPage,
    loadMoreArtifacts,
    renderArtifactList,
    renderProjects,
    resetAccountLibrary,
    showArtifacts,
    showProjects,
  };
}
