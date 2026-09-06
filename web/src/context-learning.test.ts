import { beforeAll, describe, expect, it, vi } from "vitest";

let contextFunctions: string;
let runInNewContext: (code: string, globals: Record<string, unknown>) => unknown;

beforeAll(async () => {
  // Runtime test tools keep Node-only type declarations out of the browser
  // project's typecheck; these modules already ship with Node and Vite.
  const { readFileSync } = await vi.importActual<{
    readFileSync(path: URL, encoding: "utf8"): string;
  }>("node:fs");
  ({ runInNewContext } = await vi.importActual<{
    runInNewContext(code: string, globals: Record<string, unknown>): unknown;
  }>("node:vm"));
  const { parseSync, transformWithOxc } = await vi.importActual<{
    parseSync(filename: string, source: string): {
      errors: unknown[];
      program: { body: { type: string; id?: { name: string } | null; start: number; end: number }[] };
    };
    transformWithOxc(source: string, filename: string): Promise<{ code: string }>;
  }>("vite");
  // Exercise the actual functions without starting main.ts's browser bootstrap.
  // AST ranges avoid copying guards or depending on the next function's name.
  const source = readFileSync(new URL("./main.ts", import.meta.url), "utf8");
  const parsed = parseSync("main.ts", source);
  expect(parsed.errors).toEqual([]);
  const names = ["isCurrent", "scope", "showContext"];
  const functions = names.map((name) => {
    const declaration = parsed.program.body.find(
      (node) => node.type === "FunctionDeclaration" && node.id?.name === name,
    );
    if (!declaration) throw new Error(`Missing production function: ${name}`);
    return source.slice(declaration.start, declaration.end);
  });
  contextFunctions = (await transformWithOxc(functions.join("\n"), "context-test.ts")).code;
});

function deferred() {
  let resolve!: () => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<void>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

type Listener = () => void | Promise<void>;

class Element {
  children: Element[] = [];
  listeners = new Map<string, Listener>();
  disabled = false;
  textContent = "";

  append(...nodes: Element[]) { this.children.push(...nodes); }
  replaceChildren(...nodes: Element[]) { this.children = nodes; }
  addEventListener(name: string, listener: Listener) { this.listeners.set(name, listener); }
  setAttribute() {}
  async click() {
    const listener = this.listeners.get("click");
    if (!listener) throw new Error("Expected a clickable production control");
    // Deliberately dispatch even when disabled to verify the handler's guard.
    await listener();
  }
}

function fixture({ delayReads = false, delayDelete = false, savedLessons = [] as Record<string, unknown>[] } = {}) {
  const reads = deferred();
  const deletion = deferred();
  const calls: { path: string; method: string }[] = [];
  const messages: string[] = [];
  const elements: Element[] = [];
  const target = new Element();
  const authenticatedScope = {
    organization_id: "org/one", team_id: "team & two", actor_id: "actor?three",
    goal_id: null, task_id: null,
  };
  const state = {
    connected: true, generation: 3, session: { id: "session-A" },
    capabilities: new Set(["context.explain"]), authenticatedScope,
    usage: { total_tokens: 0, input_tokens: 0, output_tokens: 0, model_calls: 0, tool_calls: 0 },
  };
  const api = async (path: string, options: { method?: string } = {}) => {
    calls.push({ path, method: options.method || "GET" });
    if (options.method === "DELETE") {
      if (delayDelete) await deletion.promise;
      return;
    }
    if (delayReads) await reads.promise;
    if (path.includes("/context?")) {
      return { total_estimated_tokens: 0, conversation_tokens: 0, item_tokens: 0, reserved_output_tokens: 0, items: [] };
    }
    if (path.includes("/learning?")) return { mode: "learn", last_outcome: null };
    if (path.includes("/lessons?")) return savedLessons;
    return [];
  };
  const showContext = runInNewContext(`${contextFunctions}\nshowContext`, {
    state, api, URLSearchParams,
    $: () => target,
    document: { createElement() { const element = new Element(); elements.push(element); return element; } },
    compactContext() {}, createMemory() {}, configureLearning() {},
    forgetProjectLesson() {}, removeMemory() {},
    formScope() { throw new Error("Connected context must use authenticated scope"); },
    renderLearningOutcome() { throw new Error("This fixture has no saved outcome"); },
    toast: (message: string) => messages.push(message),
    setToolMessage: (message: string) => messages.push(message),
  }) as () => Promise<void>;
  const clear = () => {
    const button = elements.find((element) => element.textContent === "Clear learned experience");
    if (!button) throw new Error("Expected Clear even with zero listed lessons");
    return button;
  };
  return { state, showContext, calls, messages, target, reads, deletion, clear, elements };
}

describe("project learning context identity", () => {
  it("shows verified changes as literal text and marks both legacy formats excluded", async () => {
    const base = { id: "lesson-1", applicability: "clock.py", guidance: "Observed before verification", source_turn_id: "turn-1", expires_at: "2026-10-01T00:00:00Z", files: [{ path: "clock.py" }] };
    const source = '<script>doNotExecute()</script>\n';
    const view = fixture({ savedLessons: [
      { ...base, source_observation: { path: "clock.py", change: { previous_sha256: null }, truncated: true, fragments: [{ start_line: 12, text: source }] } },
      { ...base, id: "legacy-1" },
      { ...base, id: "legacy-source", source_observation: { path: "old.py", fragments: [{ start_line: 1, text: "old source" }] } },
    ] });
    await view.showContext();
    const text = view.elements.map((element) => element.textContent);
    expect(text).toContain(source);
    expect(text).toContain("Starting at line 12");
    expect(text).toContain("Earlier source observation · old.py");
    expect(text).toContain("Kept for inspection, excluded from automatic recall");
    expect(text).toContain("old source");
    expect(text).toContain("Verified source change · clock.py · excerpt");
    expect(text).toContain("Earlier generated lesson · kept for inspection, excluded from automatic recall");
  });

  it("discards delayed session-A results after session B is selected", async () => {
    const view = fixture({ delayReads: true });
    const pending = view.showContext();
    expect(view.calls).toHaveLength(4);
    view.state.session = { id: "session-B" };
    view.reads.resolve();
    await pending;
    expect(view.target.children).toHaveLength(0);
    expect(view.messages).toEqual([]);
    expect(view.calls.every((call) => call.path.includes("/session-A/"))).toBe(true);
  });

  it("does not display a delayed session-A error in session B", async () => {
    const view = fixture({ delayReads: true });
    const pending = view.showContext();
    view.state.session = { id: "session-B" };
    view.reads.reject(new Error("Only session A should see this failure"));
    await pending;
    expect(view.messages).toEqual([]);
    expect(view.target.children).toHaveLength(0);
  });

  it("does not let an old session-A Clear control delete session B", async () => {
    const view = fixture();
    await view.showContext();
    view.state.session = { id: "session-B" };
    await view.clear().click();
    expect(view.calls.filter((call) => call.method === "DELETE")).toEqual([]);
  });

  it("does not let an old connection's Clear control act after reconnecting", async () => {
    const view = fixture();
    await view.showContext();
    view.state.generation += 1;
    await view.clear().click();
    expect(view.calls.filter((call) => call.method === "DELETE")).toEqual([]);
  });

  it.each(["another session", "a new connection"])(
    "does not refresh or re-enable stale controls when Clear finishes in %s",
    async (destination) => {
      const view = fixture({ delayDelete: true });
      await view.showContext();
      const button = view.clear();
      const pending = button.click();
      expect(button.disabled).toBe(true);
      expect(view.calls.filter((call) => call.method === "DELETE")).toHaveLength(1);
      if (destination === "another session") view.state.session = { id: "session-B" };
      else view.state.generation += 1;
      view.deletion.resolve();
      await pending;
      expect(view.calls.filter((call) => call.method === "GET")).toHaveLength(4);
      expect(button.disabled).toBe(true);
      expect(view.messages).toEqual([]);
    },
  );

  it("clears the current zero-lesson project once with its authenticated scope and refreshes", async () => {
    const view = fixture({ delayDelete: true });
    await view.showContext();
    const button = view.clear();
    const pending = button.click();
    await button.click();
    const deletes = view.calls.filter((call) => call.method === "DELETE");
    expect(deletes).toHaveLength(1);
    const url = new URL(deletes[0].path, "http://synthetic.invalid");
    expect(url.pathname).toBe("/v1/sessions/session-A/lessons");
    expect(Object.fromEntries(url.searchParams)).toEqual({
      organization_id: view.state.authenticatedScope.organization_id,
      team_id: view.state.authenticatedScope.team_id,
      actor_id: view.state.authenticatedScope.actor_id,
    });
    view.deletion.resolve();
    await pending;
    expect(view.calls.filter((call) => call.method === "GET")).toHaveLength(8);
    expect(view.calls.every((call) => call.path.includes("/session-A/"))).toBe(true);
    expect(button.disabled).toBe(false);
    expect(view.messages).toEqual([]);
  });
});
