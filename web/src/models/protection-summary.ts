import type { ProtectionRule } from "./privacy-protections";

export function protectionPath(rule: ProtectionRule, root: string | null) {
  const fullPath = rule.canonical_path || rule.path;
  const prefix = root ? `${root.replace(/\/$/, "")}/` : null;
  const project = Boolean(root && (fullPath === root || (prefix && fullPath.startsWith(prefix))));
  const relative = project && prefix ? fullPath.slice(prefix.length) || "." : fullPath;
  return {
    group: project ? "Project" : "External",
    name: rule.path.replace(/\/$/, "").split("/").at(-1) || rule.path,
    location: relative,
    fullPath: rule.canonical_path && rule.canonical_path !== rule.path ? `${rule.path}\nResolved: ${rule.canonical_path}` : rule.path,
  };
}

export function protectionGroups(rules: ProtectionRule[], root: string | null) {
  return ["Project", "External"].map(label => ({
    label,
    rules: rules.filter(rule => protectionPath(rule, root).group === label),
  })).filter(group => group.rules.length);
}
