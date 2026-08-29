export type TeamSection =
  | "overview"
  | "work"
  | "goals"
  | "agents"
  | "capacity"
  | "ownership"
  | "budgets"
  | "approvals"
  | "outcomes"
  | "audit";

export type AppRoute =
  | { type: "home" }
  | { type: "team"; section: TeamSection }
  | { type: "projects" }
  | { type: "project"; projectId: string }
  | { type: "artifacts" }
  | { type: "artifact"; artifactId: string }
  | { type: "extensions" }
  | { type: "session"; sessionId: string };

export function parseRoute(pathname: string): AppRoute {
  if (pathname === "/team") return { type: "team", section: "overview" };
  const teamSection = pathname.match(/^\/team\/([^/]+)$/)?.[1] as TeamSection | undefined;
  if (teamSection && [
    "work",
    "goals",
    "agents",
    "capacity",
    "ownership",
    "budgets",
    "approvals",
    "outcomes",
    "audit",
  ].includes(teamSection)) {
    return { type: "team", section: teamSection };
  }
  if (pathname === "/projects") return { type: "projects" };
  if (pathname === "/artifacts") return { type: "artifacts" };
  if (pathname === "/settings/extensions") return { type: "extensions" };
  const artifact = pathname.match(/^\/artifacts\/([^/]+)$/);
  if (artifact) {
    return { type: "artifact", artifactId: decodeURIComponent(artifact[1]) };
  }
  const project = pathname.match(/^\/projects\/([^/]+)$/);
  if (project) {
    return { type: "project", projectId: decodeURIComponent(project[1]) };
  }
  const match = pathname.match(/^\/chat\/([^/]+)$/);
  if (match) {
    return { type: "session", sessionId: decodeURIComponent(match[1]) };
  }
  return { type: "home" };
}

export function teamRoute(section: TeamSection): string {
  return section === "overview" ? "/team" : `/team/${section}`;
}

export function sessionRoute(sessionId: string | null): string {
  return sessionId ? `/chat/${encodeURIComponent(sessionId)}` : "/";
}

export function projectRoute(projectId: string | null): string {
  return projectId ? `/projects/${encodeURIComponent(projectId)}` : "/projects";
}

export function artifactRoute(artifactId: string | null): string {
  return artifactId ? `/artifacts/${encodeURIComponent(artifactId)}` : "/artifacts";
}

export function extensionsRoute(): string {
  return "/settings/extensions";
}
