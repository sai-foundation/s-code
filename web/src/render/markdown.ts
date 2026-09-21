import type { HighlightToken } from "./highlight";

export interface MarkdownRendererOptions {
  copyText: (text: string, success?: string) => Promise<void>;
  highlight: (
    code: string,
    language: string,
    resolve: (tokens: HighlightToken[]) => void,
  ) => boolean;
}

export interface MarkdownRenderer {
  appendBlocks(target: HTMLElement, text: unknown): void;
  createCodeBlock(code: string, language?: string): HTMLElement;
  renderMessageContent(item: HTMLElement, content: unknown): void;
  safeUrl(value: string, image?: boolean): string | null;
}

interface MarkdownListMarker {
  indent: number;
  ordered: boolean;
  start: number | null;
  content: string;
}

export function createMarkdownRenderer({
  copyText,
  highlight,
}: MarkdownRendererOptions): MarkdownRenderer {
  function safeUrl(value: string, image = false): string | null {
    const raw = String(value).trim().replace(/^<|>$/g, "");
    try {
      const parsed = new URL(raw, window.location.href);
      // Assign the canonical URL whose protocol was checked, not the original
      // spelling. Explicit checks also make the trust boundary auditable.
      if (parsed.protocol === "http:" || parsed.protocol === "https:") return parsed.href;
      if (!image && parsed.protocol === "mailto:") return parsed.href;
      return null;
    } catch {
      return null;
    }
  }

  function appendInline(target: HTMLElement, text: string): void {
    const source = String(text);
    const token = /\\[\\`*_[\]{}()#+.!|>~-]|`+[^`\n]+`+|!\[[^\]\n]*\]\([^) \n]+(?:\s+["'][^"'\n]*["'])?\)|\[[^\]\n]+\]\([^) \n]+(?:\s+["'][^"'\n]*["'])?\)|\*\*[^*\n]+\*\*|__[^_\n]+__|~~[^~\n]+~~|\*[^*\n]+\*|_[^_\n]+_|<(?:https?:\/\/[^ >]+|[^ <@]+@[^ >@]+)>| {2,}\n/g;
    let cursor = 0;
    let match: RegExpExecArray | null;
    while ((match = token.exec(source)) !== null) {
      if (match.index > cursor) {
        target.append(document.createTextNode(source.slice(cursor, match.index)));
      }
      const value = match[0];
      if (value.startsWith("\\")) {
        target.append(document.createTextNode(value.slice(1)));
      } else if (/^`/.test(value)) {
        const ticks = value.match(/^`+/)?.[0].length ?? 1;
        const code = document.createElement("code");
        code.textContent = value.slice(ticks, -ticks);
        target.append(code);
      } else if (value.startsWith("![")) {
        const parts = value.match(/^!\[([^\]]*)\]\(([^ )]+)(?:\s+["']([^"']*)["'])?\)$/);
        const url = parts && safeUrl(parts[2], true);
        if (!url) {
          target.append(document.createTextNode(value));
        } else {
          const image = document.createElement("img");
          image.src = url;
          image.alt = parts[1];
          if (parts[3]) image.title = parts[3];
          image.loading = "lazy";
          image.referrerPolicy = "no-referrer";
          target.append(image);
        }
      } else if (value.startsWith("[")) {
        const parts = value.match(/^\[([^\]]+)\]\(([^ )]+)(?:\s+["']([^"']*)["'])?\)$/);
        const url = parts && safeUrl(parts[2]);
        if (!url) {
          target.append(document.createTextNode(value));
        } else {
          const link = document.createElement("a");
          link.href = url;
          if (parts[3]) link.title = parts[3];
          link.rel = "noreferrer noopener";
          appendInline(link, parts[1]);
          target.append(link);
        }
      } else if (value.startsWith("**") || value.startsWith("__")) {
        const strong = document.createElement("strong");
        appendInline(strong, value.slice(2, -2));
        target.append(strong);
      } else if (value.startsWith("~~")) {
        const deleted = document.createElement("del");
        appendInline(deleted, value.slice(2, -2));
        target.append(deleted);
      } else if (value === "  \n") {
        target.append(document.createElement("br"));
      } else if (value.startsWith("<")) {
        const raw = value.slice(1, -1);
        const href = raw.includes("@") && !raw.includes("://") ? `mailto:${raw}` : raw;
        const url = safeUrl(href);
        if (!url) {
          target.append(document.createTextNode(value));
        } else {
          const link = document.createElement("a");
          link.href = url;
          link.rel = "noreferrer noopener";
          link.textContent = raw;
          target.append(link);
        }
      } else {
        const emphasis = document.createElement("em");
        appendInline(emphasis, value.slice(1, -1));
        target.append(emphasis);
      }
      cursor = match.index + value.length;
    }
    if (cursor < source.length) {
      target.append(document.createTextNode(source.slice(cursor)));
    }
  }

  function listMarker(line: string): MarkdownListMarker | null {
    const match = String(line).match(/^(\s*)([-+*]|\d+[.)])\s+(.+)$/);
    if (!match) return null;
    return {
      indent: match[1].replace(/\t/g, "    ").length,
      ordered: /^\d/.test(match[2]),
      start: /^\d/.test(match[2]) ? Number.parseInt(match[2], 10) : null,
      content: match[3],
    };
  }

  function appendList(
    target: HTMLElement,
    lines: string[],
    start: number,
    indent: number,
  ): { index: number } {
    const first = listMarker(lines[start]);
    if (!first) return { index: start + 1 };
    const list: HTMLOListElement | HTMLUListElement = document.createElement(
      first.ordered ? "ol" : "ul",
    );
    if (first.ordered && first.start !== null && first.start !== 1) {
      (list as HTMLOListElement).start = first.start;
    }
    let index = start;
    while (index < lines.length) {
      const marker = listMarker(lines[index]);
      if (!marker || marker.indent !== indent || marker.ordered !== first.ordered) break;
      const item = document.createElement("li");
      const task = marker.content.match(/^\[([ xX])\]\s+(.+)$/);
      if (task) {
        item.classList.add("task-list-item");
        list.classList.add("task-list");
        const checkbox = document.createElement("input");
        checkbox.type = "checkbox";
        checkbox.checked = task[1].toLowerCase() === "x";
        checkbox.disabled = true;
        checkbox.setAttribute(
          "aria-label",
          checkbox.checked ? "Completed task" : "Incomplete task",
        );
        item.append(checkbox);
        appendInline(item, task[2]);
      } else {
        appendInline(item, marker.content);
      }
      index += 1;
      while (index < lines.length) {
        const nested = listMarker(lines[index]);
        if (nested && nested.indent > indent) {
          const result = appendList(item, lines, index, nested.indent);
          index = result.index;
          continue;
        }
        if (nested || !lines[index].trim() || !/^\s+/.test(lines[index])) break;
        item.append(document.createElement("br"));
        appendInline(item, lines[index].trim());
        index += 1;
      }
      list.append(item);
      if (!lines[index]?.trim()) break;
    }
    target.append(list);
    return { index };
  }

  function splitTableRow(line: string): string[] {
    const trimmed = String(line).trim().replace(/^\|/, "").replace(/\|$/, "");
    const cells: string[] = [];
    let value = "";
    let escaped = false;
    for (const character of trimmed) {
      if (escaped) {
        value += character;
        escaped = false;
      } else if (character === "\\") {
        escaped = true;
      } else if (character === "|") {
        cells.push(value.trim());
        value = "";
      } else {
        value += character;
      }
    }
    cells.push(value.trim());
    return cells;
  }

  function tableAlignments(
    line: string,
  ): Array<"left" | "right" | "center" | null> | null {
    const cells = splitTableRow(line);
    if (!cells.length || cells.some((cell) => !/^:?-{3,}:?$/.test(cell))) return null;
    return cells.map((cell) =>
      cell.startsWith(":") && cell.endsWith(":")
        ? "center"
        : cell.endsWith(":")
          ? "right"
          : cell.startsWith(":")
            ? "left"
            : null,
    );
  }

  function appendTable(
    target: HTMLElement,
    lines: string[],
    start: number,
    alignments: Array<"left" | "right" | "center" | null>,
  ): number {
    const table = document.createElement("table");
    const head = document.createElement("thead");
    const headRow = document.createElement("tr");
    splitTableRow(lines[start]).forEach((content, index) => {
      const cell = document.createElement("th");
      if (alignments[index]) cell.style.textAlign = alignments[index];
      appendInline(cell, content);
      headRow.append(cell);
    });
    head.append(headRow);
    table.append(head);
    const body = document.createElement("tbody");
    let index = start + 2;
    while (index < lines.length && lines[index].includes("|") && lines[index].trim()) {
      const row = document.createElement("tr");
      splitTableRow(lines[index]).forEach((content, cellIndex) => {
        const cell = document.createElement("td");
        if (alignments[cellIndex]) cell.style.textAlign = alignments[cellIndex];
        appendInline(cell, content);
        row.append(cell);
      });
      body.append(row);
      index += 1;
    }
    table.append(body);
    target.append(table);
    return index;
  }

  function appendBlocks(target: HTMLElement, text: unknown): void {
    const lines = String(text).replace(/\r\n?/g, "\n").split("\n");
    let index = 0;
    const beginsBlock = (position: number): boolean => {
      const line = lines[position] || "";
      return (
        !line.trim() ||
        /^( {0,3})(#{1,6})\s+/.test(line) ||
        /^( {0,3})(`{3,}|~{3,})/.test(line) ||
        /^( {0,3})>\s?/.test(line) ||
        /^( {0,3})([-*_])(?:\s*\2){2,}\s*$/.test(line) ||
        Boolean(listMarker(line)) ||
        (line.includes("|") && Boolean(tableAlignments(lines[position + 1] || "")))
      );
    };
    while (index < lines.length) {
      const line = lines[index];
      if (!line.trim()) {
        index += 1;
        continue;
      }
      const fence = line.match(/^( {0,3})(`{3,}|~{3,})\s*([\w.+-]*)\s*$/);
      if (fence) {
        const marker = fence[2][0];
        const minimum = fence[2].length;
        const code: string[] = [];
        index += 1;
        while (
          index < lines.length &&
          !new RegExp(`^ {0,3}${marker}{${minimum},}\\s*$`).test(lines[index])
        ) {
          code.push(lines[index]);
          index += 1;
        }
        if (index < lines.length) index += 1;
        target.append(createCodeBlock(code.join("\n"), fence[3] || "text"));
        continue;
      }
      const heading = line.match(/^( {0,3})(#{1,6})\s+(.+?)\s*#*\s*$/);
      if (heading) {
        const element = document.createElement(`h${Math.min(6, heading[2].length + 1)}`);
        appendInline(element, heading[3]);
        target.append(element);
        index += 1;
        continue;
      }
      if (/^( {0,3})([-*_])(?:\s*\2){2,}\s*$/.test(line)) {
        target.append(document.createElement("hr"));
        index += 1;
        continue;
      }
      if (/^( {0,3})>\s?/.test(line)) {
        const quoteLines: string[] = [];
        while (index < lines.length && /^( {0,3})>\s?/.test(lines[index])) {
          quoteLines.push(lines[index].replace(/^( {0,3})>\s?/, ""));
          index += 1;
        }
        const quote = document.createElement("blockquote");
        appendBlocks(quote, quoteLines.join("\n"));
        target.append(quote);
        continue;
      }
      const marker = listMarker(line);
      if (marker) {
        const result = appendList(target, lines, index, marker.indent);
        index = result.index;
        continue;
      }
      const alignments = line.includes("|")
        ? tableAlignments(lines[index + 1] || "")
        : null;
      if (alignments) {
        index = appendTable(target, lines, index, alignments);
        continue;
      }
      const paragraph: string[] = [];
      while (index < lines.length && (paragraph.length === 0 || !beginsBlock(index))) {
        paragraph.push(lines[index]);
        index += 1;
      }
      const element = document.createElement("p");
      appendInline(element, paragraph.join("\n"));
      target.append(element);
    }
  }

  function createCodeBlock(code: string, language = "text"): HTMLElement {
    const wrapper = document.createElement("section");
    wrapper.className = "code-block";
    wrapper.setAttribute("aria-label", `${language || "text"} code`);
    const header = document.createElement("div");
    header.className = "code-header";
    const label = document.createElement("span");
    label.textContent = language || "text";
    const copy = document.createElement("button");
    copy.type = "button";
    copy.className = "code-copy";
    copy.textContent = "Copy";
    copy.setAttribute("aria-label", `Copy ${language || "text"} code`);
    copy.addEventListener("click", () => void copyText(code, "Code copied"));
    const pre = document.createElement("pre");
    const codeElement = document.createElement("code");
    codeElement.textContent = code;
    highlight(code, language, (tokens) => {
      if (!codeElement.isConnected || !tokens.length) return;
      const fragment = document.createDocumentFragment();
      for (const token of tokens) {
        if (token.kind === "plain") {
          fragment.append(document.createTextNode(token.text));
        } else {
          const span = document.createElement("span");
          span.className = `syntax-${token.kind}`;
          span.textContent = token.text;
          fragment.append(span);
        }
      }
      codeElement.replaceChildren(fragment);
    });
    pre.append(codeElement);
    header.append(label, copy);
    wrapper.append(header, pre);
    return wrapper;
  }

  function renderMessageContent(item: HTMLElement, content: unknown): void {
    let body = item.querySelector<HTMLElement>(".message-body");
    if (!body) {
      body = document.createElement("div");
      body.className = "message-body";
      item.append(body);
    }
    body.replaceChildren();
    if (typeof content !== "string") {
      body.append(createCodeBlock(JSON.stringify(content, null, 2), "json"));
      return;
    }
    appendBlocks(body, content);
  }

  return {
    appendBlocks,
    createCodeBlock,
    renderMessageContent,
    safeUrl,
  };
}
