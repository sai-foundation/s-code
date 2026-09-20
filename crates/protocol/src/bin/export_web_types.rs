#![recursion_limit = "256"]
use s_code_protocol::{PrivacyPage, PrivacyRequest, PrivacySource};

use s_code_protocol::{
    AddMarketplace, AgentFollowUp, AgentResultSummary, AgentRunSummary, AgentWait, Approval,
    Artifact, ArtifactPage, Attachment, AuditEventPage, AuditEventSummary,
    BackgroundTerminalOutput, BackgroundTerminalPreview, BackgroundTerminalSpec,
    BackgroundTerminalStatus, BackgroundTerminalSummary, CancelTurn, CancelTurnInput,
    CapabilityManifest, ClearSessionGoal, ClientEvent, ClientPresence, CompactSession,
    CompactSessionResult, ContextSummary, ContextSummaryItem, ContinueTeamGoal, CreateAttachment,
    CreateDurableTask, CreateMemory, CreateSession, CreateSideConversation, CreateTurn,
    CreateTurnInput, DurableTask, DurableTaskSummary, ExtensionDescriptor, ExtensionInstallPreview,
    HookInstallation, HookSpec, InstallHook, InstallMcpHttpServer, InstallMcpServer, InstallPlugin,
    InstallSkill, MarketplaceInstallation, MarketplaceSource, McpHttpInstallation,
    McpHttpServerSpec, McpInstallation, McpOAuthDiscovery, McpOAuthLaunch, McpOAuthSpec,
    McpOAuthStatus, McpProgress, McpResource, McpResourceContent, McpResourcePage, McpResourceRead,
    McpResourceTemplate, McpResourceTemplatePage, MemoryItem, Message, ModelCatalogEntry,
    PermissionProfile, PluginAppDescriptor, PluginAppSpec, PluginBundle, PluginDetail,
    PluginInstallation, PluginInterface, PluginSummary, PreviewBackgroundTerminal,
    PreviewHookInstall, PreviewMarketplaceAdd, PreviewMcpHttpServerInstall,
    PreviewMcpServerInstall, PreviewPluginInstall, PreviewSkillInstall, RemoveClientPresence,
    RemoveHook, RemoveMarketplace, RemoveMcpHttpServer, RemoveMcpServer, RemovePlugin, RemoveSkill,
    ResizeBackgroundTerminal, ResolveApproval, RetryTurn, RetryTurnResult, ReviewReport,
    SessionBranchTree, SessionExport, SessionGoal, SessionImpactPreview, SessionPreferences,
    SessionUsage, SetPluginEnabled, SetSessionGoal, SetSkillEnabled, SideConversation,
    SideConversationStart, SkillInstallation, SkillSpec, StartBackgroundTerminal, StartSessionWork,
    StopBackgroundTerminal, TeamBudget, TeamCapacity, TeamDashboardSummary, TeamGoal,
    TeamGoalContinuation, TeamGoalRun, TeamGoalRunStatus, TeamGovernanceSummary, TeamOutcome,
    TeamOwnership, TeamTask, TranscriptSnapshot, TurnUndoImpactPreview, UpdateClientPresence,
    UpdateSession, UpdateSessionGoal, UpdateSessionPreferences, UpdateTeamGoalRun,
    UpgradeMarketplace, WriteBackgroundTerminal,
};
use std::{
    env, fs,
    path::{Path, PathBuf},
};
use ts_rs::{Config, TS};

fn normalize_typescript(directory: &Path) -> Result<(), Box<dyn std::error::Error>> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            normalize_typescript(&path)?;
            continue;
        }
        if path.extension().and_then(|extension| extension.to_str()) != Some("ts") {
            continue;
        }
        let contents = fs::read_to_string(&path)?;
        let mut normalized = contents
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n");
        if contents.ends_with('\n') {
            normalized.push('\n');
        }
        if normalized != contents {
            fs::write(path, normalized)?;
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: export-web-types <output-directory>")?;
    fs::create_dir_all(&output)?;
    let config = Config::new().with_out_dir(&output).with_large_int("number");
    TranscriptSnapshot::export_all(&config)?;
    CapabilityManifest::export_all(&config)?;
    CreateSession::export_all(&config)?;
    StartSessionWork::export_all(&config)?;
    UpdateSession::export_all(&config)?;
    CancelTurn::export_all(&config)?;
    Approval::export_all(&config)?;
    ResolveApproval::export_all(&config)?;
    McpProgress::export_all(&config)?;
    SessionUsage::export_all(&config)?;
    SessionExport::export_all(&config)?;
    SessionImpactPreview::export_all(&config)?;
    TurnUndoImpactPreview::export_all(&config)?;
    MemoryItem::export_all(&config)?;
    CreateMemory::export_all(&config)?;
    CompactSession::export_all(&config)?;
    CompactSessionResult::export_all(&config)?;
    SessionBranchTree::export_all(&config)?;
    SideConversation::export_all(&config)?;
    CreateSideConversation::export_all(&config)?;
    SideConversationStart::export_all(&config)?;
    RetryTurn::export_all(&config)?;
    RetryTurnResult::export_all(&config)?;
    ReviewReport::export_all(&config)?;
    ClientEvent::export_all(&config)?;
    ClientPresence::export_all(&config)?;
    UpdateClientPresence::export_all(&config)?;
    RemoveClientPresence::export_all(&config)?;
    Artifact::export_all(&config)?;
    ArtifactPage::export_all(&config)?;
    ExtensionDescriptor::export_all(&config)?;
    ExtensionInstallPreview::export_all(&config)?;
    PreviewMcpServerInstall::export_all(&config)?;
    InstallMcpServer::export_all(&config)?;
    RemoveMcpServer::export_all(&config)?;
    McpInstallation::export_all(&config)?;
    McpHttpServerSpec::export_all(&config)?;
    McpOAuthSpec::export_all(&config)?;
    McpOAuthStatus::export_all(&config)?;
    McpOAuthLaunch::export_all(&config)?;
    McpOAuthDiscovery::export_all(&config)?;
    PreviewMcpHttpServerInstall::export_all(&config)?;
    InstallMcpHttpServer::export_all(&config)?;
    RemoveMcpHttpServer::export_all(&config)?;
    McpHttpInstallation::export_all(&config)?;
    McpResource::export_all(&config)?;
    McpResourcePage::export_all(&config)?;
    McpResourceTemplate::export_all(&config)?;
    McpResourceTemplatePage::export_all(&config)?;
    McpResourceContent::export_all(&config)?;
    McpResourceRead::export_all(&config)?;
    SkillSpec::export_all(&config)?;
    SkillInstallation::export_all(&config)?;
    PreviewSkillInstall::export_all(&config)?;
    InstallSkill::export_all(&config)?;
    SetSkillEnabled::export_all(&config)?;
    RemoveSkill::export_all(&config)?;
    HookSpec::export_all(&config)?;
    HookInstallation::export_all(&config)?;
    PreviewHookInstall::export_all(&config)?;
    InstallHook::export_all(&config)?;
    RemoveHook::export_all(&config)?;
    MarketplaceSource::export_all(&config)?;
    MarketplaceInstallation::export_all(&config)?;
    PreviewMarketplaceAdd::export_all(&config)?;
    AddMarketplace::export_all(&config)?;
    UpgradeMarketplace::export_all(&config)?;
    RemoveMarketplace::export_all(&config)?;
    PluginInterface::export_all(&config)?;
    PluginAppSpec::export_all(&config)?;
    PluginBundle::export_all(&config)?;
    PluginInstallation::export_all(&config)?;
    PluginSummary::export_all(&config)?;
    PluginDetail::export_all(&config)?;
    PluginAppDescriptor::export_all(&config)?;
    PreviewPluginInstall::export_all(&config)?;
    InstallPlugin::export_all(&config)?;
    RemovePlugin::export_all(&config)?;
    SetPluginEnabled::export_all(&config)?;
    BackgroundTerminalSpec::export_all(&config)?;
    BackgroundTerminalStatus::export_all(&config)?;
    BackgroundTerminalPreview::export_all(&config)?;
    BackgroundTerminalSummary::export_all(&config)?;
    BackgroundTerminalOutput::export_all(&config)?;
    PreviewBackgroundTerminal::export_all(&config)?;
    StartBackgroundTerminal::export_all(&config)?;
    WriteBackgroundTerminal::export_all(&config)?;
    ResizeBackgroundTerminal::export_all(&config)?;
    StopBackgroundTerminal::export_all(&config)?;
    DurableTask::export_all(&config)?;
    DurableTaskSummary::export_all(&config)?;
    CreateDurableTask::export_all(&config)?;
    AgentRunSummary::export_all(&config)?;
    AgentResultSummary::export_all(&config)?;
    AgentWait::export_all(&config)?;
    AgentFollowUp::export_all(&config)?;
    TeamGoal::export_all(&config)?;
    ContinueTeamGoal::export_all(&config)?;
    TeamGoalContinuation::export_all(&config)?;
    TeamGoalRun::export_all(&config)?;
    TeamGoalRunStatus::export_all(&config)?;
    UpdateTeamGoalRun::export_all(&config)?;
    TeamTask::export_all(&config)?;
    TeamDashboardSummary::export_all(&config)?;
    TeamGovernanceSummary::export_all(&config)?;
    TeamOwnership::export_all(&config)?;
    TeamCapacity::export_all(&config)?;
    TeamBudget::export_all(&config)?;
    TeamOutcome::export_all(&config)?;
    AuditEventSummary::export_all(&config)?;
    AuditEventPage::export_all(&config)?;
    Attachment::export_all(&config)?;
    Message::export_all(&config)?;
    CreateAttachment::export_all(&config)?;
    CreateTurn::export_all(&config)?;
    CreateTurnInput::export_all(&config)?;
    CancelTurnInput::export_all(&config)?;
    ModelCatalogEntry::export_all(&config)?;
    PermissionProfile::export_all(&config)?;
    SessionPreferences::export_all(&config)?;
    UpdateSessionPreferences::export_all(&config)?;
    SessionGoal::export_all(&config)?;
    SetSessionGoal::export_all(&config)?;
    UpdateSessionGoal::export_all(&config)?;
    ClearSessionGoal::export_all(&config)?;
    PrivacyPage::export_all(&config)?;
    PrivacyRequest::export_all(&config)?;
    PrivacySource::export_all(&config)?;
    ContextSummary::export_all(&config)?;
    ContextSummaryItem::export_all(&config)?;
    fs::write(
        output.join("index.ts"),
        concat!(
            "// Generated from s-code-protocol. Do not edit by hand.\n",
            "export type { ClientEvent } from \"./ClientEvent\";\n",
            "export type { CapabilityManifest } from \"./CapabilityManifest\";\n",
            "export type { Capability } from \"./Capability\";\n",
            "export type { CapabilityMaturity } from \"./CapabilityMaturity\";\n",
            "export type { CapabilityContract } from \"./CapabilityContract\";\n",
            "export type { ClientNotification } from \"./ClientNotification\";\n",
            "export type { ClientKind } from \"./ClientKind\";\n",
            "export type { ClientPresence } from \"./ClientPresence\";\n",
            "export type { UpdateClientPresence } from \"./UpdateClientPresence\";\n",
            "export type { RemoveClientPresence } from \"./RemoveClientPresence\";\n",
            "export type { TranscriptSnapshot } from \"./TranscriptSnapshot\";\n",
            "export type { TranscriptItem } from \"./TranscriptItem\";\n",
            "export type { TranscriptItemContent } from \"./TranscriptItemContent\";\n",
            "export type { TranscriptItemKind } from \"./TranscriptItemKind\";\n",
            "export type { TranscriptItemStatus } from \"./TranscriptItemStatus\";\n",
            "export type { McpProgress } from \"./McpProgress\";\n",
            "export type { SessionUsage } from \"./SessionUsage\";\n",
            "export type { ApprovalRequest } from \"./ApprovalRequest\";\n",
            "export type { QuestionAnswer } from \"./QuestionAnswer\";\n",
            "export type { QuestionOption } from \"./QuestionOption\";\n",
            "export type { QuestionPrompt } from \"./QuestionPrompt\";\n",
            "export type { QuestionRequest } from \"./QuestionRequest\";\n",
            "export type { QuestionStatus } from \"./QuestionStatus\";\n",
            "export type { Artifact } from \"./Artifact\";\n",
            "export type { ArtifactKind } from \"./ArtifactKind\";\n",
            "export type { ArtifactIndexEntry } from \"./ArtifactIndexEntry\";\n",
            "export type { ArtifactMetadata } from \"./ArtifactMetadata\";\n",
            "export type { ArtifactPage } from \"./ArtifactPage\";\n",
            "export type { ExtensionDescriptor } from \"./ExtensionDescriptor\";\n",
            "export type { ExtensionKind } from \"./ExtensionKind\";\n",
            "export type { ExtensionPermission } from \"./ExtensionPermission\";\n",
            "export type { ExtensionPermissionKind } from \"./ExtensionPermissionKind\";\n",
            "export type { ExtensionStatus } from \"./ExtensionStatus\";\n",
            "export type { ExtensionTrust } from \"./ExtensionTrust\";\n",
            "export type { McpServerSpec } from \"./McpServerSpec\";\n",
            "export type { ExtensionInstallPreview } from \"./ExtensionInstallPreview\";\n",
            "export type { ExtensionConfirmation } from \"./ExtensionConfirmation\";\n",
            "export type { PreviewMcpServerInstall } from \"./PreviewMcpServerInstall\";\n",
            "export type { InstallMcpServer } from \"./InstallMcpServer\";\n",
            "export type { RemoveMcpServer } from \"./RemoveMcpServer\";\n",
            "export type { McpInstallation } from \"./McpInstallation\";\n",
            "export type { McpHttpServerSpec } from \"./McpHttpServerSpec\";\n",
            "export type { McpOAuthSpec } from \"./McpOAuthSpec\";\n",
            "export type { McpOAuthStatus } from \"./McpOAuthStatus\";\n",
            "export type { McpOAuthLaunch } from \"./McpOAuthLaunch\";\n",
            "export type { McpOAuthDiscovery } from \"./McpOAuthDiscovery\";\n",
            "export type { PreviewMcpHttpServerInstall } from \"./PreviewMcpHttpServerInstall\";\n",
            "export type { InstallMcpHttpServer } from \"./InstallMcpHttpServer\";\n",
            "export type { RemoveMcpHttpServer } from \"./RemoveMcpHttpServer\";\n",
            "export type { McpHttpInstallation } from \"./McpHttpInstallation\";\n",
            "export type { McpResource } from \"./McpResource\";\n",
            "export type { McpResourcePage } from \"./McpResourcePage\";\n",
            "export type { McpResourceTemplate } from \"./McpResourceTemplate\";\n",
            "export type { McpResourceTemplatePage } from \"./McpResourceTemplatePage\";\n",
            "export type { McpResourceContent } from \"./McpResourceContent\";\n",
            "export type { McpResourceRead } from \"./McpResourceRead\";\n",
            "export type { SkillSpec } from \"./SkillSpec\";\n",
            "export type { SkillInstallation } from \"./SkillInstallation\";\n",
            "export type { PreviewSkillInstall } from \"./PreviewSkillInstall\";\n",
            "export type { InstallSkill } from \"./InstallSkill\";\n",
            "export type { SetSkillEnabled } from \"./SetSkillEnabled\";\n",
            "export type { RemoveSkill } from \"./RemoveSkill\";\n",
            "export type { HookEvent } from \"./HookEvent\";\n",
            "export type { HookSpec } from \"./HookSpec\";\n",
            "export type { HookInstallation } from \"./HookInstallation\";\n",
            "export type { PreviewHookInstall } from \"./PreviewHookInstall\";\n",
            "export type { InstallHook } from \"./InstallHook\";\n",
            "export type { RemoveHook } from \"./RemoveHook\";\n",
            "export type { MarketplaceSourceKind } from \"./MarketplaceSourceKind\";\n",
            "export type { MarketplaceSource } from \"./MarketplaceSource\";\n",
            "export type { MarketplaceInstallation } from \"./MarketplaceInstallation\";\n",
            "export type { PreviewMarketplaceAdd } from \"./PreviewMarketplaceAdd\";\n",
            "export type { AddMarketplace } from \"./AddMarketplace\";\n",
            "export type { UpgradeMarketplace } from \"./UpgradeMarketplace\";\n",
            "export type { RemoveMarketplace } from \"./RemoveMarketplace\";\n",
            "export type { PluginInterface } from \"./PluginInterface\";\n",
            "export type { PluginAppSpec } from \"./PluginAppSpec\";\n",
            "export type { PluginAgentSpec } from \"./PluginAgentSpec\";\n",
            "export type { PluginSkillAsset } from \"./PluginSkillAsset\";\n",
            "export type { PluginBundle } from \"./PluginBundle\";\n",
            "export type { PluginInstallation } from \"./PluginInstallation\";\n",
            "export type { PluginComponentSummary } from \"./PluginComponentSummary\";\n",
            "export type { PluginSummary } from \"./PluginSummary\";\n",
            "export type { PluginDetail } from \"./PluginDetail\";\n",
            "export type { PluginAppDescriptor } from \"./PluginAppDescriptor\";\n",
            "export type { PreviewPluginInstall } from \"./PreviewPluginInstall\";\n",
            "export type { InstallPlugin } from \"./InstallPlugin\";\n",
            "export type { RemovePlugin } from \"./RemovePlugin\";\n",
            "export type { SetPluginEnabled } from \"./SetPluginEnabled\";\n",
            "export type { BackgroundTerminalSpec } from \"./BackgroundTerminalSpec\";\n",
            "export type { BackgroundTerminalStatus } from \"./BackgroundTerminalStatus\";\n",
            "export type { BackgroundTerminalPreview } from \"./BackgroundTerminalPreview\";\n",
            "export type { BackgroundTerminalSummary } from \"./BackgroundTerminalSummary\";\n",
            "export type { BackgroundTerminalOutput } from \"./BackgroundTerminalOutput\";\n",
            "export type { PreviewBackgroundTerminal } from \"./PreviewBackgroundTerminal\";\n",
            "export type { StartBackgroundTerminal } from \"./StartBackgroundTerminal\";\n",
            "export type { WriteBackgroundTerminal } from \"./WriteBackgroundTerminal\";\n",
            "export type { ResizeBackgroundTerminal } from \"./ResizeBackgroundTerminal\";\n",
            "export type { StopBackgroundTerminal } from \"./StopBackgroundTerminal\";\n",
            "export type { DurableTask } from \"./DurableTask\";\n",
            "export type { DurableTaskSummary } from \"./DurableTaskSummary\";\n",
            "export type { DurableTaskStatus } from \"./DurableTaskStatus\";\n",
            "export type { CreateDurableTask } from \"./CreateDurableTask\";\n",
            "export type { AgentRunSummary } from \"./AgentRunSummary\";\n",
            "export type { AgentResultSummary } from \"./AgentResultSummary\";\n",
            "export type { AgentWait } from \"./AgentWait\";\n",
            "export type { AgentFollowUp } from \"./AgentFollowUp\";\n",
            "export type { GoalStatus } from \"./GoalStatus\";\n",
            "export type { TeamGoal } from \"./TeamGoal\";\n",
            "export type { ContinueTeamGoal } from \"./ContinueTeamGoal\";\n",
            "export type { TeamGoalContinuation } from \"./TeamGoalContinuation\";\n",
            "export type { TeamGoalRun } from \"./TeamGoalRun\";\n",
            "export type { TeamGoalRunStatus } from \"./TeamGoalRunStatus\";\n",
            "export type { UpdateTeamGoalRun } from \"./UpdateTeamGoalRun\";\n",
            "export type { TeamTaskStatus } from \"./TeamTaskStatus\";\n",
            "export type { TeamTask } from \"./TeamTask\";\n",
            "export type { TeamDashboardSummary } from \"./TeamDashboardSummary\";\n",
            "export type { TeamGovernanceSummary } from \"./TeamGovernanceSummary\";\n",
            "export type { TeamOwnership } from \"./TeamOwnership\";\n",
            "export type { TeamCapacity } from \"./TeamCapacity\";\n",
            "export type { TeamBudget } from \"./TeamBudget\";\n",
            "export type { OutcomeEvidence } from \"./OutcomeEvidence\";\n",
            "export type { TeamOutcome } from \"./TeamOutcome\";\n",
            "export type { AuditEventSummary } from \"./AuditEventSummary\";\n",
            "export type { AuditEventPage } from \"./AuditEventPage\";\n",
            "export type { Attachment } from \"./Attachment\";\n",
            "export type { AttachmentMetadata } from \"./AttachmentMetadata\";\n",
            "export type { Message } from \"./Message\";\n",
            "export type { CreateAttachment } from \"./CreateAttachment\";\n",
            "export type { CreateTurn } from \"./CreateTurn\";\n",
            "export type { CancelTurnInput } from \"./CancelTurnInput\";\n",
            "export type { CreateTurnInput } from \"./CreateTurnInput\";\n",
            "export type { TurnInput } from \"./TurnInput\";\n",
            "export type { TurnInputMode } from \"./TurnInputMode\";\n",
            "export type { TurnInputStatus } from \"./TurnInputStatus\";\n",
            "export type { Scope } from \"./Scope\";\n",
            "export type { Session } from \"./Session\";\n",
            "export type { CreateSession } from \"./CreateSession\";\n",
            "export type { StartSessionWork } from \"./StartSessionWork\";\n",
            "export type { UpdateSession } from \"./UpdateSession\";\n",
            "export type { SessionExport } from \"./SessionExport\";\n",
            "export type { SessionExportFormat } from \"./SessionExportFormat\";\n",
            "export type { SessionImpactPreview } from \"./SessionImpactPreview\";\n",
            "export type { SessionBranchNode } from \"./SessionBranchNode\";\n",
            "export type { SessionBranchTree } from \"./SessionBranchTree\";\n",
            "export type { SideConversation } from \"./SideConversation\";\n",
            "export type { SideConversationStatus } from \"./SideConversationStatus\";\n",
            "export type { CreateSideConversation } from \"./CreateSideConversation\";\n",
            "export type { SideConversationStart } from \"./SideConversationStart\";\n",
            "export type { RetryTurn } from \"./RetryTurn\";\n",
            "export type { RetryTurnResult } from \"./RetryTurnResult\";\n",
            "export type { TurnUndoImpactPreview } from \"./TurnUndoImpactPreview\";\n",
            "export type { UndoPathAction } from \"./UndoPathAction\";\n",
            "export type { UndoPathImpact } from \"./UndoPathImpact\";\n",
            "export type { MemoryScope } from \"./MemoryScope\";\n",
            "export type { MemoryItem } from \"./MemoryItem\";\n",
            "export type { CreateMemory } from \"./CreateMemory\";\n",
            "export type { CompactSession } from \"./CompactSession\";\n",
            "export type { CompactSessionResult } from \"./CompactSessionResult\";\n",
            "export type { PrivacyPage } from \"./PrivacyPage\";\n",
            "export type { PrivacyRequest } from \"./PrivacyRequest\";\n",
            "export type { PrivacySource } from \"./PrivacySource\";\n",
            "export type { ContextSummary } from \"./ContextSummary\";\n",
            "export type { ContextSummaryItem } from \"./ContextSummaryItem\";\n",
            "export type { ReviewFinding } from \"./ReviewFinding\";\n",
            "export type { ReviewFindingStatus } from \"./ReviewFindingStatus\";\n",
            "export type { ReviewLocation } from \"./ReviewLocation\";\n",
            "export type { ReviewReport } from \"./ReviewReport\";\n",
            "export type { ReviewSeverity } from \"./ReviewSeverity\";\n",
            "export type { ModelCatalogEntry } from \"./ModelCatalogEntry\";\n",
            "export type { PermissionMode } from \"./PermissionMode\";\n",
            "export type { PermissionProfile } from \"./PermissionProfile\";\n",
            "export type { SessionPreferences } from \"./SessionPreferences\";\n",
            "export type { SessionGoal } from \"./SessionGoal\";\n",
            "export type { SessionGoalStatus } from \"./SessionGoalStatus\";\n",
            "export type { SetSessionGoal } from \"./SetSessionGoal\";\n",
            "export type { UpdateSessionGoal } from \"./UpdateSessionGoal\";\n",
            "export type { ClearSessionGoal } from \"./ClearSessionGoal\";\n",
            "export type { Turn } from \"./Turn\";\n",
            "export type { CancelTurn } from \"./CancelTurn\";\n",
            "export type { Approval } from \"./Approval\";\n",
            "export type { ResolveApproval } from \"./ResolveApproval\";\n",
        ),
    )?;
    let generated_root = output
        .parent()
        .ok_or("protocol output directory has no generated parent")?;
    let api_output = generated_root.join("api");
    fs::create_dir_all(&api_output)?;
    fs::write(
        api_output.join("client.ts"),
        r#"// Generated from s-code-protocol. Do not edit by hand.
export interface ApiRequestOptions extends RequestInit {
  allowDisconnected?: boolean;
}

export interface ApiErrorPayload {
  detail?: unknown;
}

/**
 * Versioned same-origin daemon client. Callers that do not supply a generated
 * response type receive `unknown`; credentials are always the HttpOnly cookie.
 */
export async function requestEndpoint<T = unknown>(
  path: `/v1/${string}`,
  options: ApiRequestOptions = {},
): Promise<T> {
  const { allowDisconnected: _allowDisconnected, ...requestOptions } = options;
  const response = await fetch(path, {
    ...requestOptions,
    cache: "no-store",
    credentials: "same-origin",
    referrerPolicy: "no-referrer",
    headers: {
      "x-s-code-csrf": "1",
      ...(requestOptions.body ? { "content-type": "application/json" } : {}),
      ...(requestOptions.headers || {}),
    },
  });
  if (!response.ok) {
    let detail = `${response.status}`;
    const contentType = response.headers.get("content-type") || "";
    try {
      if (contentType.includes("application/json")) {
        const payload = await response.json() as ApiErrorPayload;
        if (typeof payload.detail === "string") detail = payload.detail;
      } else {
        const text = (await response.text()).trim();
        if (text) detail = text.slice(0, 4096);
      }
    } catch {
      // The status code remains the safe fallback for a malformed response.
    }
    throw new Error(detail);
  }
  if (response.status === 204) return undefined as T;
  return await response.json() as T;
}

export const requestJson = requestEndpoint;
"#,
    )?;
    fs::write(
        api_output.join("sdk.ts"),
        r#"// Generated from s-code-protocol. Do not edit by hand.
import { requestEndpoint } from "./client";
import type {
  CancelTurn,
  CapabilityManifest,
  ClientPresence,
  CreateDurableTask,
  CreateSession,
  StartSessionWork,
  CreateTurn,
  DurableTask,
  DurableTaskSummary,
  RemoveClientPresence,
  PrivacyPage,
  ResolveApproval,
  Session,
  TeamGovernanceSummary,
  TranscriptSnapshot,
  Turn,
  UpdateClientPresence,
  UpdateSession,
} from "../protocol";

export interface ScopeQuery {
  organization_id: string;
  team_id: string;
  actor_id: string;
}

function scopeQuery(scope: ScopeQuery): string {
  return new URLSearchParams({ ...scope }).toString();
}

function encoded(value: string): string {
  return encodeURIComponent(value);
}

/** Typed v1 client for the shared Session/Turn/Item/Approval/Task protocol. */
export class SCodeClient {
  privacy(sessionId: string, scope: ScopeQuery, before?: number): Promise<PrivacyPage> {
    const suffix = before == null ? "" : `&before=${before}`;
    return requestEndpoint(`/v1/sessions/${encoded(sessionId)}/privacy?${scopeQuery(scope)}${suffix}`);
  }

  capabilities(): Promise<CapabilityManifest> {
    return requestEndpoint("/v1/capabilities");
  }

  listSessions(scope: ScopeQuery): Promise<Session[]> {
    return requestEndpoint(`/v1/sessions?${scopeQuery(scope)}`);
  }

  createSession(input: CreateSession): Promise<Session> {
    return requestEndpoint("/v1/sessions", {
      method: "POST",
      body: JSON.stringify(input),
    });
  }

  startSessionWork(id: string, input: StartSessionWork): Promise<Session> {
    return requestEndpoint(`/v1/sessions/${encoded(id)}/work`, { method: "POST", body: JSON.stringify(input) });
  }

  updateSession(id: string, input: UpdateSession): Promise<Session> {
    return requestEndpoint(`/v1/sessions/${encoded(id)}`, {
      method: "PATCH",
      body: JSON.stringify(input),
    });
  }

  transcriptSnapshot(
    sessionId: string,
    scope: ScopeQuery,
    before?: string,
    limit = 500,
  ): Promise<TranscriptSnapshot> {
    const query = new URLSearchParams({ ...scope, limit: String(limit) });
    if (before) query.set("before", before);
    return requestEndpoint(`/v1/sessions/${encoded(sessionId)}/snapshot?${query}`);
  }

  createTurn(sessionId: string, input: CreateTurn): Promise<Turn> {
    return requestEndpoint(`/v1/sessions/${encoded(sessionId)}/turns`, {
      method: "POST",
      body: JSON.stringify(input),
    });
  }

  cancelTurn(turnId: string, input: CancelTurn): Promise<Turn> {
    return requestEndpoint(`/v1/turns/${encoded(turnId)}/cancel`, {
      method: "POST",
      body: JSON.stringify(input),
    });
  }

  resolveApproval(approvalId: string, input: ResolveApproval): Promise<unknown> {
    return requestEndpoint(`/v1/approvals/${encoded(approvalId)}`, {
      method: "POST",
      body: JSON.stringify(input),
    });
  }

  listDurableTasks(scope: ScopeQuery): Promise<DurableTaskSummary[]> {
    return requestEndpoint(`/v1/durable-task-summaries?${scopeQuery(scope)}`);
  }

  createDurableTask(input: CreateDurableTask): Promise<DurableTask> {
    return requestEndpoint("/v1/durable-tasks", {
      method: "POST",
      body: JSON.stringify(input),
    });
  }

  listClientPresence(scope: ScopeQuery): Promise<ClientPresence[]> {
    return requestEndpoint(`/v1/client-presence?${scopeQuery(scope)}`);
  }

  updateClientPresence(input: UpdateClientPresence): Promise<ClientPresence[]> {
    return requestEndpoint("/v1/client-presence", {
      method: "PUT",
      body: JSON.stringify(input),
    });
  }

  removeClientPresence(
    clientId: string,
    input: RemoveClientPresence,
  ): Promise<ClientPresence[]> {
    return requestEndpoint(`/v1/client-presence/${encoded(clientId)}`, {
      method: "DELETE",
      body: JSON.stringify(input),
    });
  }

  teamGovernance(
    teamId: string,
    scope: Pick<ScopeQuery, "organization_id" | "actor_id">,
  ): Promise<TeamGovernanceSummary> {
    const query = new URLSearchParams({ ...scope });
    return requestEndpoint(
      `/v1/teams/${encoded(teamId)}/governance?${query}`,
    );
  }

  eventsUrl(scope: ScopeQuery, after = 0): `/v1/${string}` {
    return `/v1/events?${new URLSearchParams({
      ...scope,
      after: String(after),
    })}`;
  }
}
"#,
    )?;
    let openapi = serde_json::json!({
        "openapi": "3.1.0",
        "info": {
            "title": "S-Code shared client protocol",
            "version": s_code_protocol::PROTOCOL_VERSION,
        },
        "servers": [{"url": "/"}],
        "paths": {
            "/v1/capabilities": {
                "get": {
                    "operationId": "capabilities",
                    "responses": {"200": {"description": "Capability manifest", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/CapabilityManifest"}}}}},
                }
            },
            "/v1/sessions": {
                "get": {
                    "operationId": "listSessions",
                    "parameters": [{"$ref": "#/components/parameters/organization_id"}, {"$ref": "#/components/parameters/team_id"}, {"$ref": "#/components/parameters/actor_id"}],
                    "responses": {
                        "200": {
                            "description": "Sessions",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "array",
                                        "items": {"$ref": "#/components/schemas/Session"},
                                    },
                                },
                            },
                        },
                    },
                },
                "post": {
                    "operationId": "createSession",
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/CreateSession"}}}},
                    "responses": {"201": {"description": "Created Session", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Session"}}}}},
                }
            },
            "/v1/sessions/{session_id}/work": {
                "post": {"operationId":"startSessionWork","parameters":[{"$ref":"#/components/parameters/session_id"}],"requestBody":{"required":true,"content":{"application/json":{"schema":{"$ref":"#/components/schemas/StartSessionWork"}}}},"responses":{"200":{"description":"Same conversation in Work mode","content":{"application/json":{"schema":{"$ref":"#/components/schemas/Session"}}}}}}},
            "/v1/sessions/{session_id}": {
                "patch": {
                    "operationId": "updateSession",
                    "parameters": [{"$ref": "#/components/parameters/session_id"}],
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/UpdateSession"}}}},
                    "responses": {"200": {"description": "Updated Session", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Session"}}}}},
                }
            },
            "/v1/sessions/{session_id}/snapshot": {
                "get": {
                    "operationId": "transcriptSnapshot",
                    "parameters": [{"$ref": "#/components/parameters/session_id"}, {"$ref": "#/components/parameters/organization_id"}, {"$ref": "#/components/parameters/team_id"}, {"$ref": "#/components/parameters/actor_id"}],
                    "responses": {"200": {"description": "Versioned Item snapshot", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/TranscriptSnapshot"}}}}},
                }
            },
            "/v1/sessions/{session_id}/turns": {
                "post": {
                    "operationId": "createTurn",
                    "parameters": [{"$ref": "#/components/parameters/session_id"}],
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/CreateTurn"}}}},
                    "responses": {"201": {"description": "Created Turn", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Turn"}}}}},
                }
            },
            "/v1/turns/{turn_id}/cancel": {
                "post": {
                    "operationId": "cancelTurn",
                    "parameters": [{"$ref": "#/components/parameters/turn_id"}],
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/CancelTurn"}}}},
                    "responses": {"200": {"description": "Cancelled Turn", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Turn"}}}}},
                }
            },
            "/v1/approvals/{approval_id}": {
                "post": {
                    "operationId": "resolveApproval",
                    "parameters": [{"$ref": "#/components/parameters/approval_id"}],
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/ResolveApproval"}}}},
                    "responses": {"200": {"description": "Resolved Tool outcome", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/ToolCallOutcome"}}}}},
                }
            },
            "/v1/durable-task-summaries": {
                "get": {
                    "operationId": "listDurableTasks",
                    "parameters": [{"$ref": "#/components/parameters/organization_id"}, {"$ref": "#/components/parameters/team_id"}, {"$ref": "#/components/parameters/actor_id"}],
                    "responses": {
                        "200": {
                            "description": "Content-free Task summaries",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "array",
                                        "items": {"$ref": "#/components/schemas/DurableTaskSummary"},
                                    },
                                },
                            },
                        },
                    },
                }
            },
            "/v1/durable-tasks": {
                "post": {
                    "operationId": "createDurableTask",
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/CreateDurableTask"}}}},
                    "responses": {"202": {"description": "Accepted Task", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/DurableTask"}}}}},
                }
            },
            "/v1/sessions/{session_id}/privacy": {
                "get": {
                    "operationId": "sessionPrivacy",
                    "parameters": [{"$ref":"#/components/parameters/session_id"},{"$ref":"#/components/parameters/organization_id"},{"$ref":"#/components/parameters/team_id"},{"$ref":"#/components/parameters/actor_id"},{"name":"before","in":"query","schema":{"type":"integer","minimum":0}}],
                    "responses": {"200":{"description":"Up to 50 model dispatches, newest first; metadata only","content":{"application/json":{"schema":{"$ref":"#/components/schemas/PrivacyPage"}}}},"403":{"description":"Session belongs to another account"}}
                }
            },
            "/v1/client-presence": {
                "get": {
                    "operationId": "listClientPresence",
                    "parameters": [{"$ref": "#/components/parameters/organization_id"}, {"$ref": "#/components/parameters/team_id"}, {"$ref": "#/components/parameters/actor_id"}],
                    "responses": {
                        "200": {
                            "description": "Content-free Presence",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "array",
                                        "items": {"$ref": "#/components/schemas/ClientPresence"},
                                    },
                                },
                            },
                        },
                    },
                },
                "put": {
                    "operationId": "updateClientPresence",
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/UpdateClientPresence"}}}},
                    "responses": {
                        "200": {
                            "description": "Current Presence",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "array",
                                        "items": {"$ref": "#/components/schemas/ClientPresence"},
                                    },
                                },
                            },
                        },
                    },
                }
            },
            "/v1/client-presence/{client_id}": {
                "delete": {
                    "operationId": "removeClientPresence",
                    "parameters": [{"$ref": "#/components/parameters/client_id"}],
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/RemoveClientPresence"}}}},
                    "responses": {
                        "200": {
                            "description": "Remaining Presence",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "array",
                                        "items": {"$ref": "#/components/schemas/ClientPresence"},
                                    },
                                },
                            },
                        },
                    },
                }
            },
            "/v1/teams/{team_id}/governance": {
                "get": {
                    "operationId": "teamGovernance",
                    "parameters": [
                        {"name": "team_id", "in": "path", "required": true, "schema": {"type": "string"}},
                        {"$ref": "#/components/parameters/organization_id"},
                        {"$ref": "#/components/parameters/actor_id"},
                    ],
                    "responses": {
                        "200": {
                            "description": "Content-free projection of effective Team governance",
                            "content": {
                                "application/json": {
                                    "schema": {"$ref": "#/components/schemas/TeamGovernanceSummary"},
                                },
                            },
                        },
                    },
                }
            }
        },
        "components": {
            "parameters": {
                "organization_id": {"name": "organization_id", "in": "query", "required": true, "schema": {"type": "string"}},
                "team_id": {"name": "team_id", "in": "query", "required": true, "schema": {"type": "string"}},
                "actor_id": {"name": "actor_id", "in": "query", "required": true, "schema": {"type": "string"}},
                "session_id": {"name": "session_id", "in": "path", "required": true, "schema": {"type": "string"}},
                "turn_id": {"name": "turn_id", "in": "path", "required": true, "schema": {"type": "string"}},
                "approval_id": {"name": "approval_id", "in": "path", "required": true, "schema": {"type": "string"}},
                "client_id": {"name": "client_id", "in": "path", "required": true, "schema": {"type": "string"}},
            },
            "schemas": {
                "Scope": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["organization_id", "team_id", "actor_id", "goal_id", "task_id"],
                    "properties": {
                        "organization_id": {"type": "string"},
                        "team_id": {"type": "string"},
                        "actor_id": {"type": "string"},
                        "goal_id": {"type": ["string", "null"]},
                        "task_id": {"type": ["string", "null"]},
                    },
                },
                "CapabilityManifest": {"type": "object", "required": ["protocol_version", "server_version", "capabilities", "contracts"], "properties": {"protocol_version": {"type": "string"}, "server_version": {"type": "string"}, "capabilities": {"type": "array"}, "contracts": {"type": "array"}}},
                "PrivacySource":{"type":"object","required":["source","kind","content_bytes","partial"],"properties":{"source":{"type":"string"},"kind":{"type":"string"},"content_bytes":{"type":"integer","minimum":0},"partial":{"type":"boolean"}}},
                "PrivacyRequest":{"type":"object","required":["id","sequence","turn_id","started_at","destination","model","purpose","status","request_bytes","sources","unattributed"],"properties":{"id":{"type":"string"},"sequence":{"type":"integer","minimum":0},"turn_id":{"type":"string"},"started_at":{"type":"string","format":"date-time"},"destination":{"type":"string"},"model":{"type":"string"},"purpose":{"type":"string"},"status":{"type":"string"},"request_bytes":{"type":"integer","minimum":0},"sources":{"type":"array","items":{"$ref":"#/components/schemas/PrivacySource"}},"unattributed":{"type":"array","items":{"type":"string"}}}},
                "PrivacyPage":{"type":"object","required":["requests","next_before"],"properties":{"requests":{"type":"array","items":{"$ref":"#/components/schemas/PrivacyRequest"}},"next_before":{"type":["integer","null"],"minimum":0}}},
                "CreateSession": {"type": "object", "additionalProperties": false, "required": ["scope", "title", "model"], "properties": {"scope": {"$ref": "#/components/schemas/Scope"}, "mode":{"type":"string","enum":["chat","work"],"default":"work"}, "workspace_uri": {"type": "string", "default":"", "description":"Empty for Chat; empty Work creates a managed directory"}, "title": {"type": "string"}, "model": {"type": "string"}}},
                "StartSessionWork":{"type":"object","additionalProperties":false,"required":["scope","reason"],"properties":{"scope":{"$ref":"#/components/schemas/Scope"},"reason":{"type":"string","minLength":1,"maxLength":300}}},
                "UpdateSession": {"type": "object", "additionalProperties": false, "required": ["scope"], "properties": {"scope": {"$ref": "#/components/schemas/Scope"}, "title": {"type": ["string", "null"]}, "status": {"enum": ["active", "archived", "deleted"]}, "model": {"type": ["string", "null"]}}},
                "Session": {"type": "object", "required": ["id", "mode", "scope", "workspace_uri", "title", "model", "status", "created_at", "updated_at"], "properties": {"id": {"type": "string"}, "mode":{"enum":["chat","work"]},"work_reason":{"type":["string","null"]}, "scope": {"$ref": "#/components/schemas/Scope"}, "workspace_uri": {"type": "string"}, "title": {"type": "string"}, "model": {"type": "string"}, "status": {"enum": ["active", "archived", "deleted"]}, "created_at": {"type": "string", "format": "date-time"}, "updated_at": {"type": "string", "format": "date-time"}}},
                "CreateTurn": {"type": "object", "additionalProperties": false, "required": ["scope", "content"], "properties": {"scope": {"$ref": "#/components/schemas/Scope"}, "content": true, "attachment_ids": {"type": "array", "items": {"type": "string"}}, "generate_title": {"type": "boolean", "default": true}}},
                "CancelTurn": {"type": "object", "additionalProperties": false, "required": ["scope"], "properties": {"scope": {"$ref": "#/components/schemas/Scope"}}},
                "Turn": {"type": "object", "required": ["id", "session_id", "scope", "status"], "properties": {"id": {"type": "string"}, "session_id": {"type": "string"}, "scope": {"$ref": "#/components/schemas/Scope"}, "status": {"type": "string"}, "checkpoint": true, "error_code": {"type": ["string", "null"]}}},
                "TranscriptSnapshot": {"type": "object", "required": ["session", "turns", "items", "cursor", "snapshot_revision", "item_count"], "properties": {"session": {"$ref": "#/components/schemas/Session"}, "turns": {"type": "array", "items": {"$ref": "#/components/schemas/Turn"}}, "items": {"type": "array", "items": {"type": "object"}}, "cursor": {"type": "integer", "minimum": 0}, "snapshot_revision": {"type": "integer", "minimum": 0}, "item_count": {"type": "integer", "minimum": 0}}},
                "ResolveApproval": {"type": "object", "additionalProperties": false, "required": ["scope", "approved", "approval_scope"], "properties": {"scope": {"$ref": "#/components/schemas/Scope"}, "approved": {"type": "boolean"}, "approval_scope": {"enum": ["once", "session"]}}},
                "Approval": {"type": "object", "required": ["id", "tool_call_id", "scope", "approval_scope", "status"], "properties": {"id": {"type": "string"}, "tool_call_id": {"type": "string"}, "scope": {"$ref": "#/components/schemas/Scope"}, "approval_scope": {"enum": ["once", "session"]}, "status": {"enum": ["pending", "approved", "rejected", "expired"]}}},
                "ToolCallOutcome": {"type": "object", "description": "Closed tagged union generated by s-code-protocol", "required": ["outcome"], "properties": {"outcome": {"enum": ["completed", "awaiting_approval", "denied", "failed"]}}},
                "CreateDurableTask": {"type": "object", "additionalProperties": false, "required": ["scope", "kind", "payload", "idempotency_key", "max_attempts", "max_runtime_seconds", "max_cost_micros"], "properties": {"scope": {"$ref": "#/components/schemas/Scope"}, "kind": {"type": "string"}, "payload": true, "idempotency_key": {"type": "string"}, "max_attempts": {"type": "integer"}, "max_runtime_seconds": {"type": "integer"}, "max_cost_micros": {"type": "integer"}, "max_runner_cost_micros": {"type": "integer"}}},
                "DurableTask": {"type": "object", "required": ["id", "scope", "kind", "status"], "properties": {"id": {"type": "string"}, "scope": {"$ref": "#/components/schemas/Scope"}, "kind": {"type": "string"}, "status": {"type": "string"}}},
                "DurableTaskSummary": {"type": "object", "required": ["id", "scope", "kind", "status"], "properties": {"id": {"type": "string"}, "scope": {"$ref": "#/components/schemas/Scope"}, "kind": {"type": "string"}, "status": {"type": "string"}}},
                "ClientPresence": {"type": "object", "required": ["client_id", "client_kind", "actor_id", "focused", "remote", "revocable", "last_seen_at", "expires_at"], "properties": {"client_id": {"type": "string"}, "client_kind": {"enum": ["cli", "web", "ide"]}, "actor_id": {"type": "string"}, "device_id": {"type": ["string", "null"]}, "session_id": {"type": ["string", "null"]}, "focused": {"type": "boolean"}, "remote": {"type": "boolean"}, "revocable": {"type": "boolean"}, "last_seen_at": {"type": "string", "format": "date-time"}, "expires_at": {"type": "string", "format": "date-time"}}},
                "UpdateClientPresence": {"type": "object", "additionalProperties": false, "required": ["scope", "client_id", "client_kind", "session_id", "focused"], "properties": {"scope": {"$ref": "#/components/schemas/Scope"}, "client_id": {"type": "string"}, "client_kind": {"enum": ["cli", "web", "ide"]}, "session_id": {"type": ["string", "null"]}, "focused": {"type": "boolean"}}},
                "RemoveClientPresence": {"type": "object", "additionalProperties": false, "required": ["scope", "revoke_remote_grant"], "properties": {"scope": {"$ref": "#/components/schemas/Scope"}, "revoke_remote_grant": {"type": "boolean"}}},
                "TeamGovernanceSummary": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["source", "configuration_sequence", "policy_sequence", "issued_at", "expires_at", "audit_content_enabled", "audit_retention_days", "data_residency_region", "audit_content_categories"],
                    "properties": {
                        "source": {"type": "string"},
                        "configuration_sequence": {"type": ["integer", "null"], "minimum": 0},
                        "policy_sequence": {"type": ["integer", "null"], "minimum": 0},
                        "issued_at": {"type": ["string", "null"], "format": "date-time"},
                        "expires_at": {"type": ["string", "null"], "format": "date-time"},
                        "audit_content_enabled": {"type": "boolean"},
                        "audit_retention_days": {"type": ["integer", "null"], "minimum": 0},
                        "data_residency_region": {"type": ["string", "null"]},
                        "audit_content_categories": {"type": "array", "items": {"type": "string"}},
                    },
                },
            }
        }
    });
    fs::write(
        api_output.join("openapi.json"),
        serde_json::to_vec_pretty(&openapi)?,
    )?;
    fs::write(
        api_output.join("contract.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": "1",
            "protocol_version": s_code_protocol::PROTOCOL_VERSION,
            "transport": "same-origin-http",
            "authentication": "http-only-cookie",
            "csrf_header": "x-s-code-csrf",
            "default_response_type": "unknown",
            "generated_types": "../protocol/index.ts",
            "generated_sdk": "sdk.ts",
            "openapi": "openapi.json"
        }))?,
    )?;
    normalize_typescript(&output)?;
    normalize_typescript(&api_output)?;
    Ok(())
}
