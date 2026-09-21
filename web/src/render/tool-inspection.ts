export interface ToolInspectionOptions {
  className: string;
  load: () => Promise<Record<string, unknown> | null>;
  loaded?: (call: Record<string, unknown>) => void;
  autoLoad?: boolean;
}

/** Exact server-redacted arguments/results stay in the originating card. */
export function appendToolInspection(container: HTMLElement, options: ToolInspectionOptions) {
  const details = document.createElement("details"); details.className = options.className;
  const summary = document.createElement("summary"); summary.textContent = "Inspect arguments and result";
  const status = document.createElement("p"); status.setAttribute("role", "status");
  const pre = document.createElement("pre"); pre.hidden = true; pre.tabIndex = 0;
  const retry = document.createElement("button"); retry.type = "button"; retry.textContent = "Reload details"; retry.hidden = true;
  let loading = false, loaded = false;
  const load = async () => {
    if (loading) return;
    loading = true; retry.disabled = true; status.textContent = "Loading server-redacted details…";
    try {
      const call = await options.load();
      if (!call || !details.isConnected) return;
      pre.textContent = JSON.stringify(call, null, 2); pre.hidden = false; loaded = true;
      status.textContent = "Exact recorded values, with server secret filtering.";
      options.loaded?.(call);
    } catch (error) {
      if (details.isConnected) status.textContent = `Details unavailable: ${(error as Error).message}`;
    } finally {
      loading = false; retry.disabled = false; retry.hidden = false;
    }
  };
  details.addEventListener("toggle", () => { if (details.open && !loaded) void load(); });
  retry.addEventListener("click", () => void load());
  details.addEventListener("refresh-tool-details", () => void load());
  details.append(summary, status, pre, retry); container.append(details);
  if (options.autoLoad) void load();
  return details;
}
