import Foundation

public enum WorkspaceInspection {
    /// The authorized workspace must itself be a repository root. The daemon
    /// remains responsible for Git safety and policy. Linked worktrees count.
    public static func hasGitRepository(_ workspace: URL) -> Bool {
        guard workspace.isFileURL else { return false }
        let root = workspace.resolvingSymlinksInPath().standardizedFileURL
        return FileManager.default.fileExists(atPath: root.appendingPathComponent(".git").path)
    }
    public static let noGitMessage = "Changes requires a Git repository at the workspace root. You can keep working here. For a new project, initialize Git when ready; for an existing repository, open its root folder."
}
