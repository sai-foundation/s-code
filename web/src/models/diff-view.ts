export interface DiffLine { text: string; kind: "header" | "hunk" | "add" | "delete" | "context"; oldLine: number | null; newLine: number | null }
export interface DiffFile { label: string; lines: DiffLine[]; additions: number; deletions: number }
export const DIFF_PAGE_SIZE = 200;

export function diffFiles(diff: string): DiffFile[] {
  if (!diff) return [];
  const files: DiffFile[] = [];
  let file: DiffFile | undefined, oldLine: number | null = null, newLine: number | null = null;
  for (const text of diff.split("\n")) {
    if (!file || text.startsWith("diff --git ")) {
      file = { label: text.startsWith("diff --git ") ? text.slice(11) : "Changes", lines: [], additions: 0, deletions: 0 };
      files.push(file); oldLine = newLine = null;
    }
    const hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(text);
    let kind: DiffLine["kind"] = "context", oldNumber: number | null = null, newNumber: number | null = null;
    if (hunk) { oldLine = Number(hunk[1]); newLine = Number(hunk[2]); kind = "hunk"; }
    else if (text.startsWith("diff --git ") || oldLine === null) kind = "header";
    else if (text.startsWith("+")) { kind = "add"; newNumber = newLine!++; file.additions++; }
    else if (text.startsWith("-")) { kind = "delete"; oldNumber = oldLine++; file.deletions++; }
    else if (text.startsWith(" ")) { oldNumber = oldLine++; newNumber = newLine!++; }
    file.lines.push({ text, kind, oldLine: oldNumber, newLine: newNumber });
  }
  return files;
}

export function diffPage(file: DiffFile, requested: number) {
  const pages = Math.max(1, Math.ceil(file.lines.length / DIFF_PAGE_SIZE));
  const page = Math.max(0, Math.min(requested, pages - 1));
  return { page, pages, lines: file.lines.slice(page * DIFF_PAGE_SIZE, (page + 1) * DIFF_PAGE_SIZE) };
}
