export type HighlightToken = {
  kind: "plain" | "comment" | "string" | "number" | "keyword";
  text: string;
};

export class HighlightClient {
  private sequence = 0;
  private readonly pending = new Map<number, (tokens: HighlightToken[]) => void>();
  private readonly worker: Worker | null;

  constructor(workerFactory: () => Worker = () =>
    new Worker("/highlight-worker.js", { name: "opencoding-code-highlight" })) {
    this.worker = typeof Worker === "undefined" ? null : workerFactory();
    this.worker?.addEventListener(
      "message",
      (event: MessageEvent<{ id: number; tokens: HighlightToken[] }>) => {
        const resolve = this.pending.get(event.data.id);
        if (!resolve) return;
        this.pending.delete(event.data.id);
        resolve(Array.isArray(event.data.tokens) ? event.data.tokens : []);
      },
    );
    this.worker?.addEventListener("error", () => {
      for (const resolve of this.pending.values()) resolve([]);
      this.pending.clear();
    });
  }

  request(
    code: string,
    language: string,
    resolve: (tokens: HighlightToken[]) => void,
  ): boolean {
    if (
      !this.worker
      || code.length < 32
      || code.length > 200_000
      || ["text", "plaintext", "diff"].includes(language.toLowerCase())
    ) {
      return false;
    }
    const id = ++this.sequence;
    this.pending.set(id, resolve);
    this.worker.postMessage({ id, code, language });
    return true;
  }

  dispose(): void {
    for (const resolve of this.pending.values()) resolve([]);
    this.pending.clear();
    this.worker?.terminate();
  }
}
