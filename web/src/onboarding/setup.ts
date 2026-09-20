import type { ApiRequestOptions } from "../api/client";

type Api = <T>(path: `/v1/${string}`, options?: ApiRequestOptions) => Promise<T>;
type Provider = { id: string; name: string; base_url: string; key_url: string; requires_key: boolean };
type Model = { id: string; name: string };
type Offer = { title: string; description: string; terms: string; url: string; ends_at: string };
export type Catalog = { providers: Provider[]; configured: boolean; credentials_available: boolean; needs_setup: boolean };

/** Keys live only in this dialog and authenticated request bodies, never browser storage. */
export function providerSetup(api: Api, onSaved: (model: string) => void) {
  let active: HTMLDialogElement | null = null;
  return async function open(catalog?: Catalog) {
    if (active) return;
    const dialog = document.createElement("dialog");
    active = dialog;
    dialog.className = "provider-setup";
    dialog.setAttribute("aria-labelledby", "setup-title");
    dialog.innerHTML = `
      <div class="setup-art" aria-hidden="true"><span class="setup-orbit"></span><span class="setup-orbit second"></span><span class="setup-mark">S<span>↗</span></span><div class="setup-art-caption">SAFE · SPEEDY · SELF-EVOLVING</div></div>
      <div class="setup-content">
        <div class="setup-top"><span class="setup-wordmark">S-CODE / START HERE</span><button type="button" class="setup-close" aria-label="Close setup">×</button></div>
        <ol class="setup-progress" aria-label="Setup progress"><li aria-current="step">01 Provider</li><li>02 Connect</li><li>03 Model</li></ol>
        <form novalidate>
          <h1 id="setup-title">Your agent. Your choice.</h1><p class="setup-subtitle">Bring a model. We'll handle the rest.</p>
          <div class="setup-panel" data-step="0"><div class="setup-providers" role="group" aria-label="Model provider"></div><aside class="setup-offers" hidden></aside></div>
          <div class="setup-panel" data-step="1" hidden>
            <label>API endpoint<input name="endpoint" type="url" spellcheck="false" autocomplete="off" required></label>
            <div class="setup-key-heading"><label for="setup-key">API key</label><a class="setup-get-key" target="_blank" rel="noopener noreferrer">Get a key ↗</a></div>
            <div class="setup-key"><input id="setup-key" name="key" type="password" autocomplete="off" spellcheck="false" maxlength="8192"><button class="setup-reveal" type="button" aria-label="Show API key" aria-pressed="false">Show</button></div>
            <p class="setup-note">Saved in a private file on this computer. Sent only to the endpoint above when connecting.</p>
          </div>
          <div class="setup-panel" data-step="2" hidden>
            <div class="setup-verified">✓ API access checked</div>
            <label>Find a model<input name="filter" type="search" placeholder="Search by name or model ID" autocomplete="off"></label>
            <label>Choose your model<select name="model" size="6" required></select></label>
            <p class="setup-note">Choose a chat or coding model. Checking API access does not send a prompt or test model generation.</p>
          </div>
          <p class="setup-status" role="status" aria-live="polite"></p>
          <div class="setup-actions"><button class="setup-back" type="button" hidden>← Back</button><span></span><button class="setup-next primary" type="submit">Continue →</button></div>
        </form>
        <p class="setup-footnote">Your workspace. Your pace. Change providers anytime in Settings.</p>
      </div>`;
    const node = <T extends HTMLElement>(selector: string) => dialog.querySelector<T>(selector)!;
    const form = node<HTMLFormElement>("form");
    const key = node<HTMLInputElement>('[name="key"]');
    const endpoint = node<HTMLInputElement>('[name="endpoint"]');
    const modelSelect = node<HTMLSelectElement>('[name="model"]');
    const filter = node<HTMLInputElement>('[name="filter"]');
    const status = node<HTMLParagraphElement>(".setup-status");
    const next = node<HTMLButtonElement>(".setup-next");
    const back = node<HTMLButtonElement>(".setup-back");
    const close = node<HTMLButtonElement>(".setup-close");
    const controller = new AbortController();
    let step = 0, busy = false, finished = false;
    let selected: Provider | undefined;
    let models: Model[] = [];
    let offers: Offer[] = [];
    const alive = () => !finished && active === dialog;
    function finish() {
      finished = true;
      controller.abort();
      key.value = "";
      dialog.close(); dialog.remove(); active = null;
    }
    close.onclick = () => { if (!busy) finish(); };
    dialog.addEventListener("cancel", (event) => { event.preventDefault(); if (!busy) finish(); });
    function setBusy(value: boolean, message = "") {
      busy = value; status.textContent = message; status.classList.remove("error");
      dialog.setAttribute("aria-busy", String(value));
      next.disabled = value; back.disabled = value; close.disabled = value;
      endpoint.disabled = value; key.disabled = value; modelSelect.disabled = value; filter.disabled = value;
      next.classList.toggle("loading", value);
    }
    function showStep(value: number) {
      step = value; status.textContent = "";
      dialog.querySelectorAll<HTMLElement>("[data-step]").forEach((panel) => { panel.hidden = Number(panel.dataset.step) !== step; });
      dialog.querySelectorAll(".setup-progress li").forEach((item, index) => {
        item.removeAttribute("aria-current"); if (index === step) item.setAttribute("aria-current", "step");
        item.classList.toggle("complete", index < step);
      });
      const headings = ["Your agent. Your choice.", `Connect ${selected?.name || "your provider"}.`, "Find your coding partner."];
      const subtitles = ["Bring a model. We'll handle the rest.", "One key. A world of possibilities.", "These models are available from your provider."];
      node("h1").textContent = headings[step]!;
      node(".setup-subtitle").textContent = subtitles[step]!;
      back.hidden = step === 0;
      next.textContent = ["Continue →", "Connect & find models →", "Start with this model →"][step]!;
      (step === 0 ? node<HTMLButtonElement>('.setup-providers button[aria-pressed="true"]') : step === 1 ? endpoint.readOnly ? key : endpoint : filter)?.focus();
    }
    function renderOffers() {
      const container = node(".setup-offers"); container.replaceChildren();
      container.hidden = selected?.id !== "sai";
      for (const offer of offers) {
        // A compromised offer cannot inject HTML, script links, or a key destination.
        if (Date.parse(offer.ends_at) <= Date.now()) continue;
        let url: URL; try { url = new URL(offer.url); } catch { continue; }
        if (url.origin !== "https://api.sai.foundation" || url.username || url.password) continue;
        const link = document.createElement("a"); link.href = url.href; link.target = "_blank"; link.rel = "noopener noreferrer";
        link.textContent = `${offer.title} ↗`;
        const description = document.createElement("p"); description.textContent = offer.description;
        const terms = document.createElement("small"); terms.textContent = offer.terms;
        container.append(link, description, terms);
      }
      if (!container.childElementCount) container.hidden = true;
    }
    function select(provider: Provider) {
      selected = provider; key.value = ""; key.type = "password"; models = [];
      const reveal = node<HTMLButtonElement>(".setup-reveal"); reveal.textContent = "Show"; reveal.setAttribute("aria-label", "Show API key"); reveal.setAttribute("aria-pressed", "false");
      endpoint.value = provider.base_url;
      endpoint.readOnly = !["local", "openai-compatible"].includes(provider.id);
      key.required = provider.requires_key;
      key.placeholder = provider.requires_key ? "Paste your API key" : "Optional for local or custom endpoints";
      const link = node<HTMLAnchorElement>(".setup-get-key");
      link.hidden = !provider.key_url; link.href = provider.key_url;
      dialog.querySelectorAll<HTMLButtonElement>(".setup-providers button").forEach((button) => button.setAttribute("aria-pressed", String(button.dataset.provider === provider.id)));
      renderOffers();
    }
    function renderModels() {
      const current = modelSelect.value; modelSelect.replaceChildren();
      const query = filter.value.trim().toLowerCase();
      for (const model of models.filter((m) => `${m.name} ${m.id}`.toLowerCase().includes(query))) {
        const option = document.createElement("option"); option.value = model.id;
        option.textContent = model.name === model.id ? model.id : `${model.name} · ${model.id}`;
        modelSelect.append(option);
      }
      if (Array.from(modelSelect.options).some((option) => option.value === current)) modelSelect.value = current;
      else modelSelect.selectedIndex = 0;
      next.disabled = !modelSelect.value;
    }
    filter.oninput = renderModels;
    node<HTMLButtonElement>(".setup-reveal").onclick = () => {
      const show = key.type === "password"; key.type = show ? "text" : "password";
      const button = node<HTMLButtonElement>(".setup-reveal"); button.textContent = show ? "Hide" : "Show";
      button.setAttribute("aria-label", show ? "Hide API key" : "Show API key"); button.setAttribute("aria-pressed", String(show));
    };
    back.onclick = () => { if (!busy) { showStep(step - 1); next.disabled = false; } };
    form.onsubmit = async (event) => {
      event.preventDefault(); if (busy || !selected) return;
      if (step === 0) { showStep(1); return; }
      if (step === 1 && (!endpoint.reportValidity() || !key.reportValidity())) return;
      const connection = { preset: selected.id, base_url: endpoint.value.trim(), api_key: key.value.trim() };
      if (step === 2 && !modelSelect.value) return;
      setBusy(true, step === 1 ? "Connecting securely and finding your models…" : "Checking and saving your connection…");
      try {
        if (step === 1) {
          const response = await api<{ models: Model[] }>("/v1/provider-setup/models", { method: "POST", body: JSON.stringify(connection), signal: controller.signal });
          if (!alive()) return;
          models = response.models; filter.value = ""; setBusy(false); renderModels(); showStep(2);
        } else {
          const response = await api<{ model: string }>("/v1/provider-setup", { method: "PUT", body: JSON.stringify({ connection, model: modelSelect.value }), signal: controller.signal });
          if (!alive()) return;
          finish(); onSaved(response.model);
        }
      } catch (error) {
        if (alive()) { setBusy(false, error instanceof Error ? error.message : "Connection failed. Please try again."); status.classList.add("error"); }
      }
    };
    document.body.append(dialog); dialog.showModal(); setBusy(true, "Loading providers…");
    try {
      const data = catalog ?? await api<Catalog>("/v1/provider-setup", { signal: controller.signal });
      if (!alive()) return;
      for (const provider of data.providers) {
        const button = document.createElement("button"); button.type = "button"; button.dataset.provider = provider.id;
        const mark = document.createElement("span"); mark.className = "setup-provider-mark"; mark.textContent = provider.id === "sai" ? "S↗" : provider.name.slice(0, 2);
        const name = document.createElement("strong"); name.textContent = provider.name;
        const detail = document.createElement("small"); detail.textContent = provider.id === "sai" ? "SAI model gateway" : provider.id === "local" ? "On your computer" : provider.id === "openai-compatible" ? "Your own endpoint" : "Bring your API key";
        button.append(mark, name, detail); button.onclick = () => select(provider);
        node(".setup-providers").append(button);
      }
      if (!data.providers.length) throw new Error("No providers are available");
      select(data.providers[0]!); setBusy(false); showStep(0);
      void api<{ promotions: Offer[] }>("/v1/provider-setup/promotions", { signal: controller.signal }).then((data) => { if (alive()) { offers = data.promotions; renderOffers(); } }).catch(() => {});
    } catch (error) { if (alive()) { setBusy(false, error instanceof Error ? error.message : "Setup unavailable"); next.disabled = true; } }
  };
}
