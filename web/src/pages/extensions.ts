import type { ApiRequestOptions } from "../api/client";
import type {
  ExtensionDescriptor,
  ExtensionInstallPreview,
  HookSpec,
  MarketplaceInstallation,
  MarketplaceSource,
  McpHttpServerSpec,
  McpOAuthDiscovery,
  McpOAuthLaunch,
  McpOAuthStatus,
  McpResourcePage,
  McpResourceRead,
  McpResourceTemplatePage,
  McpServerSpec,
  PluginDetail,
  PluginSummary,
  Scope,
  SkillInstallation,
  SkillSpec,
} from "../models/protocol";
import { extensionsRoute } from "../router";

interface ExtensionElement extends HTMLElement {
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

export interface ExtensionsPageContext {
  lookup(id: string): ExtensionElement;
  api<T = unknown>(path: string, options?: ApiRequestOptions): Promise<T>;
  state: { connected: boolean; capabilities: Set<string> };
  scope(): Scope;
  requestAction(options: ActionOptions): Promise<Record<string, string> | null>;
  toast(message: string): void;
  copyText(text: string, success?: string): Promise<void>;
  closeDrawers(restoreFocus?: boolean): void;
  closeUserMenu(): void;
  catalogQuery(): URLSearchParams;
  routePath(path: string, replace?: boolean): void;
}

export function createExtensionsPage(context: ExtensionsPageContext) {
  const {
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
  } = context;
  let extensionEntries: ExtensionDescriptor[] = [];
  let selectedExtensionId: string | null = null;

  function extensionMcpServerId(extension: ExtensionDescriptor): string {
    return extension.id.startsWith("mcp:") ? extension.id.slice("mcp:".length) : extension.id;
  }

  async function readMcpResource(
    extension: ExtensionDescriptor,
    uri: string,
    output: HTMLElement,
  ) {
    const serverId = extensionMcpServerId(extension);
    const query = catalogQuery();
    query.set("uri", uri);
    output.replaceChildren();
    output.textContent = "Reading resource…";
    const resource = await api<McpResourceRead>(
      `/v1/mcp/${encodeURIComponent(serverId)}/resources/read?${query}`,
    );
    if (selectedExtensionId !== extension.id) return;
    output.replaceChildren();
    if (!resource.contents.length) {
      output.textContent = "The server returned no content.";
      return;
    }
    resource.contents.forEach((content) => {
      const article = document.createElement("article");
      article.className = "mcp-resource-content";
      const heading = document.createElement("strong");
      heading.textContent = `${content.uri} · ${content.mime_type || "unknown type"}`;
      const body = document.createElement("pre");
      body.textContent = content.text
        ?? `[base64 blob · ${content.blob_base64?.length || 0} characters]`;
      article.append(heading, body);
      output.append(article);
    });
  }

  async function browseMcpResources(
    extension: ExtensionDescriptor,
    cursor: string | null = null,
  ) {
    const target = $("extension-detail");
    const serverId = extensionMcpServerId(extension);
    const query = catalogQuery();
    if (cursor) query.set("cursor", cursor);
    let browser = target.querySelector<HTMLElement>(".mcp-resource-browser");
    if (!browser) {
      browser = document.createElement("section");
      browser.className = "mcp-resource-browser";
      target.append(browser);
    }
    browser.replaceChildren();
    browser.textContent = "Loading MCP resources…";
    const [page, templates] = await Promise.all([
      api<McpResourcePage>(
        `/v1/mcp/${encodeURIComponent(serverId)}/resources?${query}`,
      ),
      cursor
        ? Promise.resolve<McpResourceTemplatePage>({
          resource_templates: [],
          next_cursor: null,
        })
        : api<McpResourceTemplatePage>(
          `/v1/mcp/${encodeURIComponent(serverId)}/resource-templates?${catalogQuery()}`,
        ),
    ]);
    if (selectedExtensionId !== extension.id) return;
    browser.replaceChildren();
    const heading = document.createElement("strong");
    heading.textContent = `Resources · ${page.resources.length}`;
    browser.append(heading);
    if (!page.resources.length) {
      const empty = document.createElement("p");
      empty.textContent = "This server exposes no resources on this page.";
      browser.append(empty);
    }
    const output = document.createElement("div");
    output.className = "mcp-resource-output";
    page.resources.forEach((resource) => {
      const row = document.createElement("button");
      row.type = "button";
      row.className = "mcp-resource-row";
      const name = document.createElement("strong");
      name.textContent = resource.name;
      const uri = document.createElement("span");
      uri.textContent = resource.uri;
      const detail = document.createElement("small");
      detail.textContent = [resource.mime_type, resource.description].filter(Boolean).join(" · ");
      row.append(name, uri, detail);
      row.addEventListener("click", () => {
        readMcpResource(extension, resource.uri, output)
          .catch((error) => toast(error.message));
      });
      browser!.append(row);
    });
    if (page.next_cursor) {
      const next = document.createElement("button");
      next.type = "button";
      next.textContent = "Next resource page";
      next.addEventListener("click", () => {
        browseMcpResources(extension, page.next_cursor)
          .catch((error) => toast(error.message));
      });
      browser.append(next);
    }
    if (templates.resource_templates.length) {
      const templateHeading = document.createElement("strong");
      templateHeading.textContent = `Templates · ${templates.resource_templates.length}`;
      browser.append(templateHeading);
      templates.resource_templates.forEach((template) => {
        const row = document.createElement("article");
        row.className = "mcp-resource-template";
        const name = document.createElement("strong");
        name.textContent = template.name;
        const uri = document.createElement("span");
        uri.textContent = template.uri_template;
        const detail = document.createElement("small");
        detail.textContent = [template.mime_type, template.description].filter(Boolean).join(" · ");
        row.append(name, uri, detail);
        browser!.append(row);
      });
    }
    browser.append(output);
  }

  function showExtensionDetail(extension: ExtensionDescriptor) {
    selectedExtensionId = extension.id;
    renderExtensionList();
    const target = $("extension-detail");
    target.replaceChildren();
    const title = document.createElement("h3");
    title.textContent = extension.name;
    const description = document.createElement("p");
    description.textContent = extension.description;
    const facts = document.createElement("dl");
    facts.className = "extension-facts";
    const addFact = (label: string, value: string) => {
      const term = document.createElement("dt");
      term.textContent = label;
      const detail = document.createElement("dd");
      detail.textContent = value;
      facts.append(term, detail);
    };
    addFact("Kind", extension.kind.replaceAll("_", " "));
    addFact("Status", extension.status.replaceAll("_", " "));
    addFact("Source", extension.source_uri);
    addFact("Trust", extension.trust.replaceAll("_", " "));
    addFact("Signature", extension.signature_verified ? "Verified" : "Not an Enterprise-signed package");
    addFact("Allowlist", extension.allowlisted ? "Allowed" : "Not allowed");
    addFact(
      "Authentication",
      extension.oauth_supported
        ? extension.authenticated ? "OAuth connected" : "OAuth required"
        : "No OAuth flow",
    );
    if (extension.publisher) addFact("Publisher", extension.publisher);
    if (extension.version) addFact("Version", extension.version);
    target.append(title, description, facts);
    if (extension.locked_reason) {
      const lock = document.createElement("p");
      lock.className = "extension-lock";
      lock.textContent = `Managed setting · ${extension.locked_reason}`;
      target.append(lock);
    }
    const permissionHeading = document.createElement("strong");
    permissionHeading.textContent = `Effective permissions · ${extension.permissions.length}`;
    target.append(permissionHeading);
    const permissions = document.createElement("div");
    permissions.className = "extension-permissions";
    if (!extension.permissions.length) {
      permissions.textContent = "No runtime permissions declared.";
    }
    extension.permissions.forEach((permission) => {
      const row = document.createElement("article");
      row.className = "extension-permission";
      const kind = document.createElement("strong");
      kind.textContent = permission.kind;
      const value = document.createElement("span");
      value.textContent = permission.value;
      const reason = document.createElement("small");
      reason.textContent = permission.reason;
      row.append(kind, value, reason);
      permissions.append(row);
    });
    target.append(permissions);
    const isLocalMcp = extension.kind === "mcp_server"
      && (
        extension.source_uri.startsWith("opencoding://extensions/mcp/")
        || extension.source_uri.startsWith("opencoding://extensions/mcp-http/")
      );
    const isLocalHook = extension.kind === "hook"
      && extension.source_uri.startsWith("opencoding://extensions/hooks/");
    const isLocalSkill = extension.kind === "skill"
      && extension.source_uri.startsWith("file://");
    const isLocalPlugin = extension.kind === "plugin"
      && extension.id.startsWith("plugin:");
    const actions = document.createElement("div");
    actions.className = "artifact-library-actions";
    if (
      extension.kind === "mcp_server"
      && extension.status === "connected"
      && state.capabilities.has("mcp.resources.v1")
    ) {
      const browse = document.createElement("button");
      browse.type = "button";
      browse.textContent = "Browse resources";
      browse.addEventListener("click", () => {
        browseMcpResources(extension).catch((error) => toast(error.message));
      });
      actions.append(browse);
    }
    const isOauthMcp = isLocalMcp
      && extension.source_uri.startsWith("opencoding://extensions/mcp-http/")
      && extension.oauth_supported
      && state.capabilities.has("mcp.oauth.pkce.v1");
    if (isOauthMcp) {
      const auth = document.createElement("button");
      auth.type = "button";
      auth.textContent = extension.authenticated ? "Log out OAuth" : "Log in with OAuth";
      auth.addEventListener("click", () => {
        const operation = extension.authenticated
          ? logoutMcpOAuth(extension)
          : loginMcpOAuth(extension);
        operation.catch((error) => toast(error.message));
      });
      actions.append(auth);
    }
    if ((isLocalMcp || isLocalSkill || isLocalHook) && extension.permissions_sha256) {
      if (isLocalSkill) {
        const toggle = document.createElement("button");
        toggle.type = "button";
        const enable = extension.status === "disabled";
        toggle.textContent = enable ? "Enable" : "Disable";
        toggle.addEventListener("click", () => {
          setSkillEnabled(extension, enable).catch((error) => toast(error.message));
        });
        actions.append(toggle);
      }
      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "danger";
      remove.textContent = "Remove";
      remove.addEventListener("click", () => {
        const operation = isLocalHook
          ? removeHook(extension)
          : isLocalSkill
            ? removeSkill(extension)
            : removeMcpServer(extension);
        operation.catch((error) => toast(error.message));
      });
      actions.append(remove);
    }
    if (isLocalPlugin) {
      const install = document.createElement("button");
      install.type = "button";
      install.textContent = extension.status === "available" ? "Install" : "Review update";
      install.addEventListener("click", () => {
        installOrUpdatePlugin(extension).catch((error) => toast(error.message));
      });
      actions.append(install);
      if (extension.status === "installed" || extension.status === "disabled") {
        const toggle = document.createElement("button");
        toggle.type = "button";
        const enable = extension.status === "disabled";
        toggle.textContent = enable ? "Enable" : "Disable";
        toggle.addEventListener("click", () => {
          setPluginEnabled(extension, enable).catch((error) => toast(error.message));
        });
        const remove = document.createElement("button");
        remove.type = "button";
        remove.className = "danger";
        remove.textContent = "Remove";
        remove.addEventListener("click", () => {
          removePlugin(extension).catch((error) => toast(error.message));
        });
        actions.append(toggle, remove);
      }
    }
    if (actions.childElementCount) target.append(actions);
    if (isLocalPlugin) {
      enrichPluginDetail(extension, target).catch((error) => toast(error.message));
    }
  }

  function renderExtensionList() {
    const kind = $("extension-filter").value;
    const query = $("extension-search").value.trim().toLocaleLowerCase();
    const visible = extensionEntries.filter(
      (extension) => (kind === "all" || extension.kind === kind)
        && (!query || [
          extension.name,
          extension.description,
          extension.source_uri,
          extension.publisher || "",
        ].some((value) => value.toLocaleLowerCase().includes(query))),
    );
    const list = $("extension-list");
    list.replaceChildren();
    list.classList.toggle("empty", !visible.length);
    $("extension-count").textContent = `${visible.length} extension${visible.length === 1 ? "" : "s"}`;
    if (!visible.length) {
      list.textContent = extensionEntries.length
        ? "No extensions match this kind."
        : "No extensions configured.";
    }
    visible.forEach((extension) => {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "extension-row";
      button.classList.toggle("active", extension.id === selectedExtensionId);
      const heading = document.createElement("span");
      const name = document.createElement("strong");
      name.textContent = extension.name;
      const status = document.createElement("span");
      status.className = "extension-status";
      status.textContent = extension.status.replaceAll("_", " ");
      heading.append(name, status);
      const kind = document.createElement("small");
      kind.textContent = `${extension.kind.replaceAll("_", " ")} · ${extension.trust.replaceAll("_", " ")}`;
      const source = document.createElement("small");
      source.textContent = extension.source_uri;
      button.append(heading, kind, source);
      button.addEventListener("click", () => showExtensionDetail(extension));
      list.append(button);
    });
  }

  async function loadExtensionCatalog() {
    if (!state.connected) {
      extensionEntries = [];
      renderExtensionList();
      $("extension-detail").textContent = "Connect to inspect extensions.";
      return;
    }
    extensionEntries = await api<ExtensionDescriptor[]>(`/v1/extensions?${catalogQuery()}`);
    const selected = extensionEntries.find((extension) => extension.id === selectedExtensionId)
      || extensionEntries[0]
      || null;
    selectedExtensionId = selected?.id || null;
    renderExtensionList();
    if (selected) showExtensionDetail(selected);
    else {
      const empty = document.createElement("p");
      empty.className = "empty";
      empty.textContent = "No extensions configured.";
      $("extension-detail").replaceChildren(empty);
    }
  }

  function parseEnvironmentHandles(value: string): Record<string, string> {
    const handles: Record<string, string> = {};
    value.split(/\r?\n/).map((line) => line.trim()).filter(Boolean).forEach((line) => {
      const separator = line.indexOf("=");
      if (separator <= 0 || separator === line.length - 1) {
        throw new Error("Environment handles must use NAME=ENVIRONMENT_VARIABLE, one per line");
      }
      const name = line.slice(0, separator).trim();
      const handle = line.slice(separator + 1).trim();
      if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name)
        || !/^[A-Za-z_][A-Za-z0-9_]*$/.test(handle)) {
        throw new Error("Environment names and handles must be valid variable names");
      }
      handles[name] = handle;
    });
    return handles;
  }

  function parseHeaderHandles(value: string): Record<string, string> {
    const handles: Record<string, string> = {};
    value.split(/\r?\n/).map((line) => line.trim()).filter(Boolean).forEach((line) => {
      const separator = line.indexOf("=");
      if (separator <= 0 || separator === line.length - 1) {
        throw new Error("Header handles must use HEADER=ENVIRONMENT_VARIABLE, one per line");
      }
      const name = line.slice(0, separator).trim();
      const handle = line.slice(separator + 1).trim();
      if (!/^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/.test(name)
        || !/^[A-Za-z_][A-Za-z0-9_]*$/.test(handle)) {
        throw new Error("Header names and environment-variable handles are invalid");
      }
      handles[name] = handle;
    });
    return handles;
  }

  async function addMcpServer() {
    if (!state.connected) {
      toast("Connect to Opencoding before installing an MCP server.");
      return;
    }
    const values = await requestAction({
      eyebrow: "Community extension",
      title: "Add an MCP server",
      description: "Choose local stdio, header-authenticated HTTP, or OAuth HTTP. Secrets stay in the daemon; enter only environment-variable handles here.",
      confirm: "Preview permissions",
      fields: [
        { name: "id", label: "Server ID", placeholder: "local-tools", required: true, maxlength: 64 },
        {
          name: "transport",
          label: "Transport",
          options: [
            ["stdio", "Local stdio"],
            ["http", "Streamable HTTP · header handles"],
            ["http_oauth", "Streamable HTTP · OAuth 2.1 PKCE"],
          ],
        },
        { name: "program", label: "Absolute executable path (stdio)", placeholder: "/usr/local/bin/example-mcp" },
        { name: "args", label: "Arguments (one per line)", multiline: true, placeholder: "--stdio" },
        { name: "environment", label: "Environment handles (NAME=VARIABLE)", multiline: true, placeholder: "API_TOKEN=EXAMPLE_API_TOKEN" },
        { name: "endpoint", label: "Endpoint (Streamable HTTP)", placeholder: "https://mcp.example.com/mcp" },
        { name: "headers", label: "Header handles (HEADER=VARIABLE)", multiline: true, placeholder: "Authorization=MCP_ACCESS_TOKEN" },
        { name: "oauth_client_id", label: "OAuth public client ID (optional)", placeholder: "Leave empty for dynamic registration" },
        { name: "oauth_scopes", label: "OAuth scopes (one per line, optional)", multiline: true, placeholder: "mcp:tools\nmcp:resources" },
        { name: "timeout", label: "Timeout (milliseconds)", type: "number", min: 100, value: 30000, required: true },
      ],
    });
    if (!values) return;
    const http = values.transport === "http" || values.transport === "http_oauth";
    const oauth = values.transport === "http_oauth";
    const server: McpServerSpec | McpHttpServerSpec = http
      ? {
          id: values.id.trim(),
          endpoint: values.endpoint.trim(),
          header_handles: oauth ? {} : parseHeaderHandles(values.headers),
          oauth: oauth
            ? {
                client_id: values.oauth_client_id.trim() || null,
                scopes: values.oauth_scopes
                  .split(/\r?\n/)
                  .map((scope) => scope.trim())
                  .filter(Boolean),
              }
            : null,
          timeout_ms: Number(values.timeout),
        }
      : {
          id: values.id.trim(),
          program: values.program.trim(),
          args: values.args.split(/\r?\n/).map((line) => line.trim()).filter(Boolean),
          environment_handles: parseEnvironmentHandles(values.environment),
          timeout_ms: Number(values.timeout),
        };
    const endpoint = http ? "/v1/extensions/mcp-http" : "/v1/extensions/mcp";
    const preview = await api<ExtensionInstallPreview>(`${endpoint}/preview`, {
      method: "POST",
      body: JSON.stringify({ scope: scope(), server }),
    });
    const confirmation = await requestAction({
      eyebrow: "Permission review",
      title: `Install ${preview.descriptor.name}?`,
      description: "Review every effective permission. The server starts only after Opencoding restarts.",
      details: [
        ...preview.descriptor.permissions.map(
          (permission) => `${permission.kind}: ${permission.value} — ${permission.reason}`,
        ),
        `Scope: Team ${scope().team_id}`,
        `Source: ${preview.descriptor.source_uri}`,
        `Restart required: ${preview.requires_restart ? "yes" : "no"}`,
      ],
      confirm: "Install MCP server",
    });
    if (!confirmation) return;
    await api<ExtensionInstallPreview>(`${endpoint}/install`, {
      method: "POST",
      body: JSON.stringify({
        scope: scope(),
        server,
        confirmation: {
          confirmed: true,
          permissions_sha256: preview.permissions_sha256,
        },
      }),
    });
    await loadExtensionCatalog();
    toast(oauth
      ? "MCP server installed · select it and log in with OAuth"
      : "MCP server installed · restart Opencoding to connect");
  }

  async function loginMcpOAuth(extension: ExtensionDescriptor) {
    const serverId = extension.id.replace(/^mcp:/, "");
    const discovery = await api<McpOAuthDiscovery>(
      `/v1/extensions/mcp-http/${encodeURIComponent(serverId)}/oauth/preview`,
      {
        method: "POST",
        body: JSON.stringify({ scope: scope() }),
      },
    );
    const confirmed = await requestAction({
      eyebrow: "MCP OAuth",
      title: `Log in to ${extension.name}?`,
      description: "Opencoding will open the provider in a separate window. Access and refresh tokens remain encrypted in the daemon.",
      details: [
        `Identity provider: ${discovery.authorization_server}`,
        `Login endpoint: ${discovery.authorization_endpoint}`,
        `Token endpoint: ${discovery.token_endpoint}`,
        `Scopes: ${discovery.scopes.join(", ") || "none requested"}`,
        "Callback: local daemon only",
      ],
      confirm: "Open OAuth login",
    });
    if (!confirmed) return;
    const launch = await api<McpOAuthLaunch>(
      `/v1/extensions/mcp-http/${encodeURIComponent(serverId)}/oauth`,
      {
        method: "POST",
        body: JSON.stringify({
          scope: scope(),
          confirmation: {
            confirmed: true,
            permissions_sha256: discovery.permissions_sha256,
          },
        }),
      },
    );
    const popup = window.open(
      launch.authorization_url,
      `opencoding-mcp-oauth-${serverId}`,
      "popup,width=720,height=760,noopener,noreferrer",
    );
    if (!popup) {
      await copyText(launch.authorization_url, "OAuth login URL copied");
      toast("The browser blocked the login window · URL copied");
    } else {
      toast("Complete the login in the provider window");
    }
    const expiresAt = Date.parse(launch.expires_at);
    while (Date.now() < expiresAt) {
      await new Promise((resolve) => window.setTimeout(resolve, 1000));
      const status = await api<McpOAuthStatus>(
        `/v1/extensions/mcp-http/${encodeURIComponent(serverId)}/oauth?${catalogQuery()}`,
      );
      if (status.authenticated) {
        await loadExtensionCatalog();
        toast("OAuth login complete · restart Opencoding to connect");
        return;
      }
    }
    throw new Error("OAuth login expired; start it again");
  }

  async function logoutMcpOAuth(extension: ExtensionDescriptor) {
    const confirmed = await requestAction({
      eyebrow: "MCP OAuth",
      title: `Log out of ${extension.name}?`,
      description: "The daemon will revoke the token when the provider supports revocation, then remove its encrypted local credential.",
      details: [
        `MCP server: ${extension.id}`,
        `Scope: Team ${scope().team_id}`,
        "Restart required to disconnect the running server",
      ],
      confirm: "Log out OAuth",
      danger: true,
    });
    if (!confirmed) return;
    const serverId = extension.id.replace(/^mcp:/, "");
    await api<McpOAuthStatus>(
      `/v1/extensions/mcp-http/${encodeURIComponent(serverId)}/oauth`,
      {
        method: "DELETE",
        body: JSON.stringify({ scope: scope() }),
      },
    );
    await loadExtensionCatalog();
    toast("OAuth login removed · restart Opencoding to disconnect");
  }

  async function removeMcpServer(extension: ExtensionDescriptor) {
    if (!extension.permissions_sha256) {
      throw new Error("This MCP installation has no removable permission revision");
    }
    const confirmed = await requestAction({
      eyebrow: "Community extension",
      title: `Remove ${extension.name}?`,
      description: "The persisted installation will be removed. Restart Opencoding to stop the currently connected process and remove its tools.",
      details: extension.permissions.map(
        (permission) => `${permission.kind}: ${permission.value}`,
      ),
      confirm: "Remove MCP server",
      danger: true,
    });
    if (!confirmed) return;
    const endpoint: `/v1/${string}` = extension.source_uri.startsWith("opencoding://extensions/mcp-http/")
      ? `/v1/extensions/mcp-http/${encodeURIComponent(extension.id)}`
      : `/v1/extensions/${encodeURIComponent(extension.id)}`;
    await api<ExtensionDescriptor>(endpoint, {
      method: "DELETE",
      body: JSON.stringify({
        scope: scope(),
        confirmation: {
          confirmed: true,
          permissions_sha256: extension.permissions_sha256,
        },
      }),
    });
    selectedExtensionId = null;
    await loadExtensionCatalog();
    toast("MCP server removed · restart Opencoding to finish");
  }

  function localPathToFileUri(path: string): string {
    const normalized = path.trim().replaceAll("\\", "/");
    if (/^[A-Za-z]:\//.test(normalized)) {
      return new URL(`file:///${normalized}`).toString();
    }
    if (!normalized.startsWith("/")) {
      throw new Error("Marketplace source must be an absolute local path");
    }
    return new URL(normalized, "file:///").toString();
  }

  async function managePluginMarketplaces() {
    if (!state.connected) {
      toast("Connect to Opencoding before managing Plugin Marketplaces.");
      return;
    }
    const values = await requestAction({
      eyebrow: "Community Plugins",
      title: "Manage Plugin Marketplaces",
      description: "Community sources are explicit local directories or marketplace.json files. Adding, refreshing, and removal all require a permission review.",
      confirm: "Continue",
      fields: [
        { name: "action", label: "Action", options: [["list", "List"], ["add", "Add"], ["upgrade", "Refresh"], ["remove", "Remove"]] },
        { name: "name", label: "Marketplace name", placeholder: "local" },
        { name: "path", label: "Absolute local source path (Add only)", placeholder: "/opt/opencoding-plugins" },
      ],
    });
    if (!values) return;
    if (values.action === "list") {
      const marketplaces = await api<MarketplaceInstallation[]>(`/v1/marketplaces?${catalogQuery()}`);
      await requestAction({
        eyebrow: "Community Plugins",
        title: "Configured Marketplaces",
        description: marketplaces.length
          ? "These local sources are indexed for this Team."
          : "No Plugin Marketplaces are configured.",
        details: marketplaces.map((marketplace) =>
          `${marketplace.source.name} · revision ${marketplace.revision} · ${marketplace.source.source_uri}`
        ),
        confirm: "Close",
      });
      return;
    }
    const name = values.name.trim();
    if (!name) throw new Error("Marketplace name is required");
    let preview: ExtensionInstallPreview;
    let source: MarketplaceSource | null = null;
    if (values.action === "add") {
      source = {
        name,
        source_uri: localPathToFileUri(values.path),
        source_kind: "local",
      };
      preview = await api<ExtensionInstallPreview>("/v1/marketplaces/preview", {
        method: "POST",
        body: JSON.stringify({ scope: scope(), source }),
      });
    } else {
      preview = await api<ExtensionInstallPreview>(
        `/v1/marketplaces/${encodeURIComponent(name)}/preview-upgrade?${catalogQuery()}`,
        { method: "POST" },
      );
    }
    const confirmed = await requestAction({
      eyebrow: "Marketplace permission review",
      title: `${values.action === "remove" ? "Remove" : values.action === "upgrade" ? "Refresh" : "Add"} ${name}?`,
      description: values.action === "remove"
        ? "Installed Plugins must be removed first. This removes the source registration, not arbitrary files."
        : "Review the exact local source and manifest digest before continuing.",
      details: preview.descriptor.permissions.map(
        (permission) => `${permission.kind}: ${permission.value} — ${permission.reason}`,
      ),
      confirm: values.action === "remove" ? "Remove Marketplace" : values.action === "upgrade" ? "Refresh Marketplace" : "Add Marketplace",
      danger: values.action === "remove",
    });
    if (!confirmed) return;
    if (values.action === "add" && source) {
      await api<MarketplaceInstallation>("/v1/marketplaces", {
        method: "POST",
        body: JSON.stringify({
          scope: scope(),
          source,
          confirmation: { confirmed: true, permissions_sha256: preview.permissions_sha256 },
        }),
      });
    } else {
      await api<MarketplaceInstallation | ExtensionDescriptor>(
        `/v1/marketplaces/${encodeURIComponent(name)}`,
        {
          method: values.action === "remove" ? "DELETE" : "POST",
          body: JSON.stringify({
            scope: scope(),
            confirmation: { confirmed: true, permissions_sha256: preview.permissions_sha256 },
          }),
        },
      );
    }
    await loadExtensionCatalog();
    toast(`Marketplace ${values.action === "upgrade" ? "refreshed" : values.action === "add" ? "added" : "removed"}`);
  }

  function pluginSelector(extension: ExtensionDescriptor): string {
    return extension.id.startsWith("plugin:") ? extension.id.slice("plugin:".length) : extension.id;
  }

  async function installOrUpdatePlugin(extension: ExtensionDescriptor) {
    const selector = pluginSelector(extension);
    const separator = selector.lastIndexOf("@");
    if (separator <= 0 || separator === selector.length - 1) {
      throw new Error("Plugin identity is missing its Marketplace");
    }
    const plugin_name = selector.slice(0, separator);
    const marketplace_name = selector.slice(separator + 1);
    const preview = await api<ExtensionInstallPreview>("/v1/plugins/preview", {
      method: "POST",
      body: JSON.stringify({ scope: scope(), marketplace_name, plugin_name }),
    });
    const confirmed = await requestAction({
      eyebrow: "Plugin permission review",
      title: `${extension.status === "installed" ? "Update" : "Install"} ${preview.descriptor.name}?`,
      description: "The reviewed Plugin package is frozen and installed atomically. Bundled MCP servers activate after restart.",
      details: [
        ...preview.descriptor.permissions.map(
          (permission) => `${permission.kind}: ${permission.value} — ${permission.reason}`,
        ),
        `Version: ${preview.descriptor.version || "unspecified"}`,
        `Restart required: ${preview.requires_restart ? "yes" : "no"}`,
      ],
      confirm: extension.status === "installed" ? "Update Plugin" : "Install Plugin",
    });
    if (!confirmed) return;
    await api<PluginSummary>("/v1/plugins/install", {
      method: "POST",
      body: JSON.stringify({
        scope: scope(),
        marketplace_name,
        plugin_name,
        confirmation: { confirmed: true, permissions_sha256: preview.permissions_sha256 },
      }),
    });
    await loadExtensionCatalog();
    toast(preview.requires_restart
      ? "Plugin installed · restart Opencoding to activate bundled MCP servers"
      : "Plugin installed");
  }

  async function setPluginEnabled(extension: ExtensionDescriptor, enabled: boolean) {
    const selector = pluginSelector(extension);
    const detail = await api<PluginDetail>(`/v1/plugins/${encodeURIComponent(selector)}?${catalogQuery()}`);
    if (detail.summary.installed_revision == null) {
      throw new Error("Plugin is not installed");
    }
    await api<ExtensionDescriptor>(`/v1/plugins/${encodeURIComponent(selector)}`, {
      method: "PUT",
      body: JSON.stringify({
        scope: scope(),
        enabled,
        expected_revision: detail.summary.installed_revision,
      }),
    });
    await loadExtensionCatalog();
    toast(`${enabled ? "Enabled" : "Disabled"} ${extension.name}`);
  }

  async function removePlugin(extension: ExtensionDescriptor) {
    const selector = pluginSelector(extension);
    const detail = await api<PluginDetail>(`/v1/plugins/${encodeURIComponent(selector)}?${catalogQuery()}`);
    const digest = detail.summary.descriptor.permissions_sha256;
    if (!digest) throw new Error("Plugin installation has no removable permission revision");
    const confirmed = await requestAction({
      eyebrow: "Community Plugin",
      title: `Remove ${extension.name}?`,
      description: "The frozen Plugin bundle and all of its Skill, Hook, MCP, App, and Agent contents will be removed together.",
      details: detail.summary.components.map(
        (component) => `${component.kind}: ${component.name}`,
      ),
      confirm: "Remove Plugin",
      danger: true,
    });
    if (!confirmed) return;
    await api<ExtensionDescriptor>(`/v1/plugins/${encodeURIComponent(selector)}`, {
      method: "DELETE",
      body: JSON.stringify({
        scope: scope(),
        confirmation: { confirmed: true, permissions_sha256: digest },
      }),
    });
    selectedExtensionId = null;
    await loadExtensionCatalog();
    toast("Plugin removed");
  }

  async function enrichPluginDetail(extension: ExtensionDescriptor, target: HTMLElement) {
    const detail = await api<PluginDetail>(
      `/v1/plugins/${encodeURIComponent(pluginSelector(extension))}?${catalogQuery()}`,
    );
    if (selectedExtensionId !== extension.id) return;
    const heading = document.createElement("strong");
    heading.textContent = `Contents · ${detail.summary.components.length}`;
    const contents = document.createElement("div");
    contents.className = "extension-permissions";
    detail.summary.components.forEach((component) => {
      const row = document.createElement("article");
      row.className = "extension-permission";
      const kind = document.createElement("strong");
      kind.textContent = component.kind;
      const name = document.createElement("span");
      name.textContent = component.name;
      const description = document.createElement("small");
      description.textContent = component.description;
      row.append(kind, name, description);
      contents.append(row);
    });
    target.append(heading, contents);
  }

  function parseLines(value: string): string[] {
    return value.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
  }

  async function addSkill() {
    if (!state.connected) {
      toast("Connect to Opencoding before installing a Skill.");
      return;
    }
    const values = await requestAction({
      eyebrow: "Local instructions",
      title: "Add a Skill",
      description: "Opencoding reads and freezes one reviewed SKILL.md revision. Automatic matching is optional; $skill-id always invokes an enabled Skill.",
      confirm: "Preview permissions",
      fields: [
        { name: "id", label: "Skill ID", placeholder: "secure-review", required: true, maxlength: 64 },
        { name: "name", label: "Display name", placeholder: "Secure review", required: true, maxlength: 80 },
        { name: "summary", label: "Description", placeholder: "Review access-control-sensitive changes.", required: true, maxlength: 500 },
        { name: "source", label: "SKILL.md file URI", placeholder: "file:///Users/me/.opencoding/skills/secure-review/SKILL.md", required: true },
        { name: "terms", label: "Automatic match terms (one per line)", multiline: true, placeholder: "access control review\nsecurity review" },
        { name: "dependencies", label: "Required MCP server IDs (one per line)", multiline: true, placeholder: "repository" },
        {
          name: "activation",
          label: "Activation",
          value: "automatic",
          options: [
            ["automatic", "Automatic terms + $skill-id"],
            ["manual", "$skill-id only"],
          ],
        },
      ],
    });
    if (!values) return;
    const terms = parseLines(values.terms);
    if (values.activation === "automatic" && !terms.length) {
      throw new Error("Automatic Skill activation requires at least one match term");
    }
    const skill: SkillSpec = {
      id: values.id.trim(),
      name: values.name.trim(),
      description: values.summary.trim(),
      source_uri: values.source.trim(),
      activation_terms: terms,
      mcp_dependencies: parseLines(values.dependencies),
      auto_match: values.activation === "automatic",
    };
    const preview = await api<ExtensionInstallPreview>("/v1/extensions/skills/preview", {
      method: "POST",
      body: JSON.stringify({ scope: scope(), skill }),
    });
    const confirmation = await requestAction({
      eyebrow: "Permission review",
      title: `Install ${preview.descriptor.name}?`,
      description: "Confirm the exact reviewed file digest, activation scope and dependencies. The Skill becomes available immediately.",
      details: [
        ...preview.descriptor.permissions.map(
          (permission) => `${permission.kind}: ${permission.value} — ${permission.reason}`,
        ),
        `Scope: Team ${scope().team_id}`,
        `Source: ${preview.descriptor.source_uri}`,
        `Restart required: ${preview.requires_restart ? "yes" : "no"}`,
      ],
      confirm: "Install Skill",
    });
    if (!confirmation) return;
    await api<ExtensionInstallPreview>("/v1/extensions/skills/install", {
      method: "POST",
      body: JSON.stringify({
        scope: scope(),
        skill,
        confirmation: {
          confirmed: true,
          permissions_sha256: preview.permissions_sha256,
        },
      }),
    });
    await loadExtensionCatalog();
    toast(`Skill installed · invoke with $${skill.id}`);
  }

  async function setSkillEnabled(extension: ExtensionDescriptor, enabled: boolean) {
    const skillId = extension.id.startsWith("skill:") ? extension.id.slice("skill:".length) : extension.id;
    const installation = await api<SkillInstallation>(
      `/v1/extensions/skills/${encodeURIComponent(skillId)}?${catalogQuery()}`,
    );
    await api<ExtensionDescriptor>(`/v1/extensions/skills/${encodeURIComponent(skillId)}`, {
      method: "PUT",
      body: JSON.stringify({
        scope: scope(),
        enabled,
        expected_revision: installation.revision,
      }),
    });
    await loadExtensionCatalog();
    toast(`Skill ${enabled ? "enabled" : "disabled"}`);
  }

  async function removeSkill(extension: ExtensionDescriptor) {
    if (!extension.permissions_sha256) {
      throw new Error("This Skill installation has no removable permission revision");
    }
    const confirmed = await requestAction({
      eyebrow: "Local instructions",
      title: `Remove ${extension.name}?`,
      description: "The reviewed instructions will stop matching new Turns immediately.",
      details: extension.permissions.map(
        (permission) => `${permission.kind}: ${permission.value}`,
      ),
      confirm: "Remove Skill",
      danger: true,
    });
    if (!confirmed) return;
    const skillId = extension.id.startsWith("skill:") ? extension.id.slice("skill:".length) : extension.id;
    await api<ExtensionDescriptor>(`/v1/extensions/skills/${encodeURIComponent(skillId)}`, {
      method: "DELETE",
      body: JSON.stringify({
        scope: scope(),
        confirmation: {
          confirmed: true,
          permissions_sha256: extension.permissions_sha256,
        },
      }),
    });
    selectedExtensionId = null;
    await loadExtensionCatalog();
    toast("Skill removed");
  }

  async function addHook() {
    if (!state.connected) {
      toast("Connect to Opencoding before installing a Hook.");
      return;
    }
    const values = await requestAction({
      eyebrow: "Local automation",
      title: "Add a Tool Hook",
      description: "Hooks receive bounded structured JSON. Use an absolute executable path; secrets remain in environment variables.",
      confirm: "Preview permissions",
      fields: [
        { name: "id", label: "Hook ID", placeholder: "validate-edits", required: true, maxlength: 64 },
        { name: "name", label: "Display name", placeholder: "Validate edits", required: true, maxlength: 80 },
        {
          name: "event",
          label: "Event",
          value: "pre_tool_use",
          options: [
            ["pre_tool_use", "Before Tool Use"],
            ["post_tool_use", "After Tool Use"],
          ],
        },
        { name: "program", label: "Absolute executable path", placeholder: "/usr/local/bin/example-hook", required: true },
        { name: "args", label: "Arguments (one per line)", multiline: true, placeholder: "--format\njson" },
        { name: "environment", label: "Environment handles (NAME=VARIABLE)", multiline: true, placeholder: "API_TOKEN=EXAMPLE_API_TOKEN" },
        { name: "timeout", label: "Timeout (milliseconds)", type: "number", min: 100, max: 10000, value: 3000, required: true },
        {
          name: "modify",
          label: "May replace Tool arguments",
          value: "false",
          options: [
            ["false", "No"],
            ["true", "Yes — before Tool Use only"],
          ],
        },
      ],
    });
    if (!values) return;
    if (values.event === "post_tool_use" && values.modify === "true") {
      throw new Error("Only a Before Tool Use Hook may replace Tool arguments");
    }
    const hook: HookSpec = {
      id: values.id.trim(),
      name: values.name.trim(),
      event: values.event as HookSpec["event"],
      program: values.program.trim(),
      args: values.args.split(/\r?\n/).map((line) => line.trim()).filter(Boolean),
      environment_handles: parseEnvironmentHandles(values.environment),
      timeout_ms: Number(values.timeout),
      can_modify_input: values.modify === "true",
    };
    const preview = await api<ExtensionInstallPreview>("/v1/extensions/hooks/preview", {
      method: "POST",
      body: JSON.stringify({ scope: scope(), hook }),
    });
    const confirmation = await requestAction({
      eyebrow: "Permission review",
      title: `Install ${preview.descriptor.name}?`,
      description: "Review every effective permission. The Hook becomes active immediately after installation.",
      details: [
        ...preview.descriptor.permissions.map(
          (permission) => `${permission.kind}: ${permission.value} — ${permission.reason}`,
        ),
        `Scope: Team ${scope().team_id}`,
        `Source: ${preview.descriptor.source_uri}`,
        `Restart required: ${preview.requires_restart ? "yes" : "no"}`,
      ],
      confirm: "Install Hook",
    });
    if (!confirmation) return;
    await api<ExtensionInstallPreview>("/v1/extensions/hooks/install", {
      method: "POST",
      body: JSON.stringify({
        scope: scope(),
        hook,
        confirmation: {
          confirmed: true,
          permissions_sha256: preview.permissions_sha256,
        },
      }),
    });
    await loadExtensionCatalog();
    toast("Hook installed");
  }

  async function removeHook(extension: ExtensionDescriptor) {
    if (!extension.permissions_sha256) {
      throw new Error("This Hook installation has no removable permission revision");
    }
    const confirmed = await requestAction({
      eyebrow: "Local automation",
      title: `Remove ${extension.name}?`,
      description: "The Hook will stop running for new Tool calls immediately.",
      details: extension.permissions.map(
        (permission) => `${permission.kind}: ${permission.value}`,
      ),
      confirm: "Remove Hook",
      danger: true,
    });
    if (!confirmed) return;
    const hookId = extension.id.startsWith("hook:") ? extension.id.slice("hook:".length) : extension.id;
    await api<ExtensionDescriptor>(`/v1/extensions/hooks/${encodeURIComponent(hookId)}`, {
      method: "DELETE",
      body: JSON.stringify({
        scope: scope(),
        confirmation: {
          confirmed: true,
          permissions_sha256: extension.permissions_sha256,
        },
      }),
    });
    selectedExtensionId = null;
    await loadExtensionCatalog();
    toast("Hook removed");
  }

  function showExtensions({ replace = false, updateRoute = true } = {}) {
    closeDrawers(false);
    document.body.classList.add("team-mode");
    $("team-view").hidden = true;
    $("projects-view").hidden = true;
    $("artifacts-view").hidden = true;
    $("extensions-view").hidden = false;
    $("workspace-shell").hidden = true;
    $("open-team").classList.remove("active");
    $("open-team").removeAttribute("aria-current");
    document.body.classList.remove("mobile-sidebar-open");
    closeUserMenu();
    if (updateRoute) routePath(extensionsRoute(), replace);
    $("extensions-view").scrollTop = 0;
    loadExtensionCatalog().catch((error) => toast(error.message));
    $("extensions-title").focus({ preventScroll: true });
  }


  return {
    addHook,
    addMcpServer,
    addSkill,
    loadExtensionCatalog,
    managePluginMarketplaces,
    renderExtensionList,
    showExtensions,
  };
}
