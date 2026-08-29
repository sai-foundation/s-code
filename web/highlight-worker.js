"use strict";

const KEYWORDS = new Set([
  "as", "async", "await", "break", "case", "catch", "class", "const", "continue",
  "def", "do", "else", "enum", "export", "extends", "false", "fn", "for", "from",
  "function", "if", "impl", "import", "in", "interface", "let", "loop", "match",
  "mod", "move", "mut", "new", "null", "of", "pub", "raise", "return", "self",
  "static", "struct", "super", "switch", "this", "throw", "trait", "true", "try",
  "type", "undefined", "use", "var", "while", "with", "yield",
]);

function tokenize(code) {
  const tokens = [];
  const pattern = /(\/\/[^\n]*|#[^\n]*|\/\*[\s\S]*?\*\/|"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|`(?:\\.|[^`\\])*`|\b\d+(?:\.\d+)?\b|\b[A-Za-z_$][\w$]*\b)/g;
  let cursor = 0;
  for (const match of code.matchAll(pattern)) {
    if (match.index > cursor) {
      tokens.push({ kind: "plain", text: code.slice(cursor, match.index) });
    }
    const text = match[0];
    const kind = text.startsWith("//") || text.startsWith("/*") || text.startsWith("#")
      ? "comment"
      : /^["'`]/.test(text)
        ? "string"
        : /^\d/.test(text)
          ? "number"
          : KEYWORDS.has(text)
            ? "keyword"
            : "plain";
    tokens.push({ kind, text });
    cursor = match.index + text.length;
  }
  if (cursor < code.length) tokens.push({ kind: "plain", text: code.slice(cursor) });
  return tokens;
}

self.addEventListener("message", (event) => {
  const { id, code } = event.data || {};
  if (typeof id !== "number" || typeof code !== "string" || code.length > 200000) {
    self.postMessage({ id, tokens: [] });
    return;
  }
  self.postMessage({ id, tokens: tokenize(code) });
});
