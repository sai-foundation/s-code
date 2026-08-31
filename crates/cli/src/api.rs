use anyhow::{Context, Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use opencoding_config::{LocalDaemonConnection, read_local_daemon_connection};
use opencoding_protocol::{
    AddMarketplace, AgentFollowUp, AgentResultSummary, AgentRunSummary, AgentWait, ApprovalScope,
    Artifact, AttachmentMetadata, BackgroundTerminalOutput, BackgroundTerminalPreview,
    BackgroundTerminalSpec, BackgroundTerminalSummary, CancelTurn, CancelTurnInput,
    CapabilityManifest, ClearSessionGoal, ClientKind, ClientPresence, CompactSession,
    CompactSessionResult, ContextSummary, CreateAttachment, CreateMemory, CreateReview,
    CreateSession, CreateTurn, CreateTurnInput, DurableTaskSummary, ExtensionConfirmation,
    ExtensionDescriptor, ExtensionInstallPreview, ForkSession, HookSpec, Id, InstallHook,
    InstallMcpHttpServer, InstallMcpServer, InstallPlugin, InstallSkill, LogoutMcpOAuth,
    MarketplaceInstallation, MarketplaceSource, McpHttpServerSpec, McpOAuthDiscovery,
    McpOAuthLaunch, McpOAuthStatus, McpResourcePage, McpResourceRead, McpResourceTemplatePage,
    McpServerSpec, MemoryItem, MemoryScope, Message, ModelCatalogEntry, PermissionProfile,
    PluginAppDescriptor, PluginDetail, PluginSummary, PreviewBackgroundTerminal,
    PreviewHookInstall, PreviewMarketplaceAdd, PreviewMcpHttpServerInstall, PreviewMcpOAuth,
    PreviewMcpServerInstall, PreviewPluginInstall, PreviewSkillInstall, QuestionAnswer,
    QuestionRequest, RemoveClientPresence, RemoveHook, RemoveMarketplace, RemoveMcpHttpServer,
    RemoveMcpServer, RemovePlugin, RemoveSkill, ResizeBackgroundTerminal, ResolveApproval,
    ResolveQuestion, RetryTurn, RetryTurnResult, Scope, Session, SessionGoal,
    SessionLifecycleRequest, SessionPreferences, SessionStatus, SetPluginEnabled, SetSessionGoal,
    SetSkillEnabled, SideConversation, SideConversationStart, SkillInstallation, SkillSpec,
    StartBackgroundTerminal, StartMcpOAuth, StopBackgroundTerminal, SubmitToolCall, TeamGoalRun,
    TeamGoalRunStatus, TranscriptSnapshot, Turn, TurnInput, TurnInputMode, TurnUndoImpactPreview,
    UpdateClientPresence, UpdateSession, UpdateSessionGoal, UpdateSessionPreferences,
    UpdateTeamGoalRun, UpgradeMarketplace, WriteBackgroundTerminal,
};
use reqwest::{
    Client, Response, StatusCode,
    header::{AUTHORIZATION, HeaderValue},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, RwLock};

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct WorkspacePathMatch {
    pub(crate) path: String,
}

#[derive(Clone)]
pub(crate) struct Api {
    client: Client,
    connection: Arc<RwLock<ApiConnection>>,
    local_discovery: Option<LocalDiscovery>,
    pub(crate) scope: Scope,
}

#[derive(Clone)]
struct ApiConnection {
    base: String,
    token: String,
}

type LocalDiscovery =
    Arc<dyn Fn() -> std::result::Result<LocalDaemonConnection, String> + Send + Sync>;

impl Api {
    pub(crate) fn new(base: String, token: String, scope: Scope) -> Self {
        Self::new_with_discovery(base, token, scope, None)
    }

    pub(crate) fn new_local(base: String, token: String, scope: Scope) -> Self {
        Self::new_with_discovery(
            base,
            token,
            scope,
            Some(Arc::new(read_local_daemon_connection)),
        )
    }

    fn new_with_discovery(
        base: String,
        token: String,
        scope: Scope,
        local_discovery: Option<LocalDiscovery>,
    ) -> Self {
        Self {
            client: Client::new(),
            connection: Arc::new(RwLock::new(ApiConnection {
                base: base.trim_end_matches('/').into(),
                token,
            })),
            local_discovery,
            scope,
        }
    }

    pub(crate) fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let connection = self
            .connection
            .read()
            .expect("CLI daemon connection lock is not poisoned")
            .clone();
        self.client
            .request(method, format!("{}{}", connection.base, path))
            .bearer_auth(&connection.token)
            .header("x-opencoding-csrf", "1")
    }

    pub(crate) async fn send(&self, request: reqwest::RequestBuilder) -> Result<Response> {
        let retry = request.try_clone();
        let response = request.send().await?;
        if response.status() != StatusCode::UNAUTHORIZED {
            return Ok(response);
        }
        let (Some(discover), Some(retry)) = (self.local_discovery.as_ref(), retry) else {
            return Ok(response);
        };
        let Ok(discovered) = discover() else {
            return Ok(response);
        };

        let mut retry = retry.build()?;
        let path = retry.url().path().to_owned();
        let query = retry
            .url()
            .query()
            .map(|query| format!("?{query}"))
            .unwrap_or_default();
        let base = discovered.daemon_url.trim_end_matches('/');
        *retry.url_mut() = format!("{base}{path}{query}")
            .parse()
            .context("discovered local daemon URL is invalid")?;
        let mut authorization = HeaderValue::from_str(&format!("Bearer {}", discovered.token))
            .context("discovered local daemon credential is invalid")?;
        authorization.set_sensitive(true);
        retry.headers_mut().insert(AUTHORIZATION, authorization);

        {
            let mut connection = self
                .connection
                .write()
                .expect("CLI daemon connection lock is not poisoned");
            connection.base = base.into();
            connection.token = discovered.token;
        }
        drop(response);
        Ok(self.client.execute(retry).await?)
    }

    async fn json<T: serde::de::DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T> {
        let response = self.send(request).await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("daemon returned {status}: {body}"));
        }
        Ok(response.json().await?)
    }

    pub(crate) async fn sessions(&self) -> Result<Vec<Session>> {
        self.json(self.request(
            reqwest::Method::GET,
            &format!(
                "/v1/sessions?organization_id={}&team_id={}&actor_id={}",
                encode(&self.scope.organization_id.0),
                encode(&self.scope.team_id.0),
                encode(&self.scope.actor_id.0),
            ),
        ))
        .await
    }

    pub(crate) async fn capabilities(&self) -> Result<CapabilityManifest> {
        self.json(self.request(reqwest::Method::GET, "/v1/capabilities"))
            .await
    }

    pub(crate) async fn update_client_presence(
        &self,
        client_id: &str,
        session_id: Option<Id>,
        focused: bool,
    ) -> Result<Vec<ClientPresence>> {
        self.json(
            self.request(reqwest::Method::PUT, "/v1/client-presence")
                .json(&UpdateClientPresence {
                    scope: self.scope.clone(),
                    client_id: client_id.to_owned(),
                    client_kind: ClientKind::Cli,
                    session_id,
                    focused,
                }),
        )
        .await
    }

    pub(crate) async fn client_presence(&self) -> Result<Vec<ClientPresence>> {
        self.json(self.request(
            reqwest::Method::GET,
            &format!("/v1/client-presence?{}", self.catalog_query()),
        ))
        .await
    }

    pub(crate) async fn remove_client_presence(
        &self,
        client_id: &str,
        revoke_remote_grant: bool,
    ) -> Result<Vec<ClientPresence>> {
        self.json(
            self.request(
                reqwest::Method::DELETE,
                &format!("/v1/client-presence/{}", encode(client_id)),
            )
            .json(&RemoveClientPresence {
                scope: self.scope.clone(),
                revoke_remote_grant,
            }),
        )
        .await
    }

    fn catalog_query(&self) -> String {
        format!(
            "organization_id={}&team_id={}&actor_id={}",
            encode(&self.scope.organization_id.0),
            encode(&self.scope.team_id.0),
            encode(&self.scope.actor_id.0),
        )
    }

    pub(crate) async fn model_catalog(&self) -> Result<Vec<ModelCatalogEntry>> {
        self.json(self.request(
            reqwest::Method::GET,
            &format!("/v1/models?{}", self.catalog_query()),
        ))
        .await
    }

    pub(crate) async fn permission_profiles(&self) -> Result<Vec<PermissionProfile>> {
        self.json(self.request(
            reqwest::Method::GET,
            &format!("/v1/permission-profiles?{}", self.catalog_query()),
        ))
        .await
    }

    pub(crate) async fn extension_catalog(&self) -> Result<Vec<ExtensionDescriptor>> {
        self.json(self.request(
            reqwest::Method::GET,
            &format!("/v1/extensions?{}", self.catalog_query()),
        ))
        .await
    }

    pub(crate) async fn preview_mcp_server(
        &self,
        server: McpServerSpec,
    ) -> Result<ExtensionInstallPreview> {
        self.json(
            self.request(reqwest::Method::POST, "/v1/extensions/mcp/preview")
                .json(&PreviewMcpServerInstall {
                    scope: self.scope.clone(),
                    server,
                }),
        )
        .await
    }

    pub(crate) async fn install_mcp_server(
        &self,
        server: McpServerSpec,
        permissions_sha256: String,
    ) -> Result<ExtensionInstallPreview> {
        self.json(
            self.request(reqwest::Method::POST, "/v1/extensions/mcp/install")
                .json(&InstallMcpServer {
                    scope: self.scope.clone(),
                    server,
                    confirmation: ExtensionConfirmation {
                        confirmed: true,
                        permissions_sha256,
                    },
                }),
        )
        .await
    }

    pub(crate) async fn remove_mcp_server(
        &self,
        extension_id: &str,
        permissions_sha256: String,
    ) -> Result<ExtensionDescriptor> {
        self.json(
            self.request(
                reqwest::Method::DELETE,
                &format!("/v1/extensions/{}", encode(extension_id)),
            )
            .json(&RemoveMcpServer {
                scope: self.scope.clone(),
                confirmation: ExtensionConfirmation {
                    confirmed: true,
                    permissions_sha256,
                },
            }),
        )
        .await
    }

    pub(crate) async fn preview_mcp_http_server(
        &self,
        server: McpHttpServerSpec,
    ) -> Result<ExtensionInstallPreview> {
        self.json(
            self.request(reqwest::Method::POST, "/v1/extensions/mcp-http/preview")
                .json(&PreviewMcpHttpServerInstall {
                    scope: self.scope.clone(),
                    server,
                }),
        )
        .await
    }

    pub(crate) async fn install_mcp_http_server(
        &self,
        server: McpHttpServerSpec,
        permissions_sha256: String,
    ) -> Result<ExtensionInstallPreview> {
        self.json(
            self.request(reqwest::Method::POST, "/v1/extensions/mcp-http/install")
                .json(&InstallMcpHttpServer {
                    scope: self.scope.clone(),
                    server,
                    confirmation: ExtensionConfirmation {
                        confirmed: true,
                        permissions_sha256,
                    },
                }),
        )
        .await
    }

    pub(crate) async fn remove_mcp_http_server(
        &self,
        extension_id: &str,
        permissions_sha256: String,
    ) -> Result<ExtensionDescriptor> {
        self.json(
            self.request(
                reqwest::Method::DELETE,
                &format!("/v1/extensions/mcp-http/{}", encode(extension_id)),
            )
            .json(&RemoveMcpHttpServer {
                scope: self.scope.clone(),
                confirmation: ExtensionConfirmation {
                    confirmed: true,
                    permissions_sha256,
                },
            }),
        )
        .await
    }

    pub(crate) async fn preview_mcp_oauth(&self, server_id: &str) -> Result<McpOAuthDiscovery> {
        self.json(
            self.request(
                reqwest::Method::POST,
                &format!(
                    "/v1/extensions/mcp-http/{}/oauth/preview",
                    encode(server_id)
                ),
            )
            .json(&PreviewMcpOAuth {
                scope: self.scope.clone(),
            }),
        )
        .await
    }

    pub(crate) async fn start_mcp_oauth(
        &self,
        server_id: &str,
        permissions_sha256: String,
    ) -> Result<McpOAuthLaunch> {
        self.json(
            self.request(
                reqwest::Method::POST,
                &format!("/v1/extensions/mcp-http/{}/oauth", encode(server_id)),
            )
            .json(&StartMcpOAuth {
                scope: self.scope.clone(),
                confirmation: ExtensionConfirmation {
                    confirmed: true,
                    permissions_sha256,
                },
            }),
        )
        .await
    }

    pub(crate) async fn mcp_oauth_status(&self, server_id: &str) -> Result<McpOAuthStatus> {
        self.json(self.request(
            reqwest::Method::GET,
            &format!(
                "/v1/extensions/mcp-http/{}/oauth?{}",
                encode(server_id),
                self.catalog_query()
            ),
        ))
        .await
    }

    pub(crate) async fn logout_mcp_oauth(&self, server_id: &str) -> Result<McpOAuthStatus> {
        self.json(
            self.request(
                reqwest::Method::DELETE,
                &format!("/v1/extensions/mcp-http/{}/oauth", encode(server_id)),
            )
            .json(&LogoutMcpOAuth {
                scope: self.scope.clone(),
            }),
        )
        .await
    }

    pub(crate) async fn mcp_resources(
        &self,
        server_id: &str,
        cursor: Option<&str>,
    ) -> Result<McpResourcePage> {
        let cursor = cursor
            .map(|cursor| format!("&cursor={}", encode(cursor)))
            .unwrap_or_default();
        self.json(self.request(
            reqwest::Method::GET,
            &format!(
                "/v1/mcp/{}/resources?{}{}",
                encode(server_id),
                self.catalog_query(),
                cursor,
            ),
        ))
        .await
    }

    pub(crate) async fn mcp_resource_templates(
        &self,
        server_id: &str,
        cursor: Option<&str>,
    ) -> Result<McpResourceTemplatePage> {
        let cursor = cursor
            .map(|cursor| format!("&cursor={}", encode(cursor)))
            .unwrap_or_default();
        self.json(self.request(
            reqwest::Method::GET,
            &format!(
                "/v1/mcp/{}/resource-templates?{}{}",
                encode(server_id),
                self.catalog_query(),
                cursor,
            ),
        ))
        .await
    }

    pub(crate) async fn read_mcp_resource(
        &self,
        server_id: &str,
        uri: &str,
    ) -> Result<McpResourceRead> {
        self.json(self.request(
            reqwest::Method::GET,
            &format!(
                "/v1/mcp/{}/resources/read?{}&uri={}",
                encode(server_id),
                self.catalog_query(),
                encode(uri),
            ),
        ))
        .await
    }

    pub(crate) async fn preview_hook(&self, hook: HookSpec) -> Result<ExtensionInstallPreview> {
        self.json(
            self.request(reqwest::Method::POST, "/v1/extensions/hooks/preview")
                .json(&PreviewHookInstall {
                    scope: self.scope.clone(),
                    hook,
                }),
        )
        .await
    }

    pub(crate) async fn preview_skill(&self, skill: SkillSpec) -> Result<ExtensionInstallPreview> {
        self.json(
            self.request(reqwest::Method::POST, "/v1/extensions/skills/preview")
                .json(&PreviewSkillInstall {
                    scope: self.scope.clone(),
                    skill,
                }),
        )
        .await
    }

    pub(crate) async fn install_skill(
        &self,
        skill: SkillSpec,
        permissions_sha256: String,
    ) -> Result<ExtensionInstallPreview> {
        self.json(
            self.request(reqwest::Method::POST, "/v1/extensions/skills/install")
                .json(&InstallSkill {
                    scope: self.scope.clone(),
                    skill,
                    confirmation: ExtensionConfirmation {
                        confirmed: true,
                        permissions_sha256,
                    },
                }),
        )
        .await
    }

    pub(crate) async fn skill_installation(&self, skill_id: &str) -> Result<SkillInstallation> {
        self.json(self.request(
            reqwest::Method::GET,
            &format!(
                "/v1/extensions/skills/{}?{}",
                encode(skill_id),
                self.catalog_query()
            ),
        ))
        .await
    }

    pub(crate) async fn set_skill_enabled(
        &self,
        skill_id: &str,
        enabled: bool,
        expected_revision: u64,
    ) -> Result<ExtensionDescriptor> {
        self.json(
            self.request(
                reqwest::Method::PUT,
                &format!("/v1/extensions/skills/{}", encode(skill_id)),
            )
            .json(&SetSkillEnabled {
                scope: self.scope.clone(),
                enabled,
                expected_revision,
            }),
        )
        .await
    }

    pub(crate) async fn remove_skill(
        &self,
        skill_id: &str,
        permissions_sha256: String,
    ) -> Result<ExtensionDescriptor> {
        self.json(
            self.request(
                reqwest::Method::DELETE,
                &format!("/v1/extensions/skills/{}", encode(skill_id)),
            )
            .json(&RemoveSkill {
                scope: self.scope.clone(),
                confirmation: ExtensionConfirmation {
                    confirmed: true,
                    permissions_sha256,
                },
            }),
        )
        .await
    }

    pub(crate) async fn install_hook(
        &self,
        hook: HookSpec,
        permissions_sha256: String,
    ) -> Result<ExtensionInstallPreview> {
        self.json(
            self.request(reqwest::Method::POST, "/v1/extensions/hooks/install")
                .json(&InstallHook {
                    scope: self.scope.clone(),
                    hook,
                    confirmation: ExtensionConfirmation {
                        confirmed: true,
                        permissions_sha256,
                    },
                }),
        )
        .await
    }

    pub(crate) async fn remove_hook(
        &self,
        hook_id: &str,
        permissions_sha256: String,
    ) -> Result<ExtensionDescriptor> {
        self.json(
            self.request(
                reqwest::Method::DELETE,
                &format!("/v1/extensions/hooks/{}", encode(hook_id)),
            )
            .json(&RemoveHook {
                scope: self.scope.clone(),
                confirmation: ExtensionConfirmation {
                    confirmed: true,
                    permissions_sha256,
                },
            }),
        )
        .await
    }

    pub(crate) async fn marketplaces(&self) -> Result<Vec<MarketplaceInstallation>> {
        self.json(self.request(
            reqwest::Method::GET,
            &format!("/v1/marketplaces?{}", self.catalog_query()),
        ))
        .await
    }

    pub(crate) async fn preview_marketplace(
        &self,
        source: MarketplaceSource,
    ) -> Result<ExtensionInstallPreview> {
        self.json(
            self.request(reqwest::Method::POST, "/v1/marketplaces/preview")
                .json(&PreviewMarketplaceAdd {
                    scope: self.scope.clone(),
                    source,
                }),
        )
        .await
    }

    pub(crate) async fn add_marketplace(
        &self,
        source: MarketplaceSource,
        permissions_sha256: String,
    ) -> Result<MarketplaceInstallation> {
        self.json(
            self.request(reqwest::Method::POST, "/v1/marketplaces")
                .json(&AddMarketplace {
                    scope: self.scope.clone(),
                    source,
                    confirmation: ExtensionConfirmation {
                        confirmed: true,
                        permissions_sha256,
                    },
                }),
        )
        .await
    }

    pub(crate) async fn preview_marketplace_upgrade(
        &self,
        name: &str,
    ) -> Result<ExtensionInstallPreview> {
        self.json(self.request(
            reqwest::Method::POST,
            &format!(
                "/v1/marketplaces/{}/preview-upgrade?{}",
                encode(name),
                self.catalog_query()
            ),
        ))
        .await
    }

    pub(crate) async fn upgrade_marketplace(
        &self,
        name: &str,
        permissions_sha256: String,
    ) -> Result<MarketplaceInstallation> {
        self.json(
            self.request(
                reqwest::Method::POST,
                &format!("/v1/marketplaces/{}", encode(name)),
            )
            .json(&UpgradeMarketplace {
                scope: self.scope.clone(),
                confirmation: ExtensionConfirmation {
                    confirmed: true,
                    permissions_sha256,
                },
            }),
        )
        .await
    }

    pub(crate) async fn remove_marketplace(
        &self,
        name: &str,
        permissions_sha256: String,
    ) -> Result<ExtensionDescriptor> {
        self.json(
            self.request(
                reqwest::Method::DELETE,
                &format!("/v1/marketplaces/{}", encode(name)),
            )
            .json(&RemoveMarketplace {
                scope: self.scope.clone(),
                confirmation: ExtensionConfirmation {
                    confirmed: true,
                    permissions_sha256,
                },
            }),
        )
        .await
    }

    pub(crate) async fn plugins(&self) -> Result<Vec<PluginSummary>> {
        self.json(self.request(
            reqwest::Method::GET,
            &format!("/v1/plugins?{}", self.catalog_query()),
        ))
        .await
    }

    pub(crate) async fn plugin(&self, id: &str) -> Result<PluginDetail> {
        self.json(self.request(
            reqwest::Method::GET,
            &format!("/v1/plugins/{}?{}", encode(id), self.catalog_query()),
        ))
        .await
    }

    pub(crate) async fn preview_plugin(
        &self,
        marketplace_name: String,
        plugin_name: String,
    ) -> Result<ExtensionInstallPreview> {
        self.json(
            self.request(reqwest::Method::POST, "/v1/plugins/preview")
                .json(&PreviewPluginInstall {
                    scope: self.scope.clone(),
                    marketplace_name,
                    plugin_name,
                }),
        )
        .await
    }

    pub(crate) async fn install_plugin(
        &self,
        marketplace_name: String,
        plugin_name: String,
        permissions_sha256: String,
    ) -> Result<PluginSummary> {
        self.json(
            self.request(reqwest::Method::POST, "/v1/plugins/install")
                .json(&InstallPlugin {
                    scope: self.scope.clone(),
                    marketplace_name,
                    plugin_name,
                    confirmation: ExtensionConfirmation {
                        confirmed: true,
                        permissions_sha256,
                    },
                }),
        )
        .await
    }

    pub(crate) async fn set_plugin_enabled(
        &self,
        id: &str,
        enabled: bool,
        expected_revision: u64,
    ) -> Result<ExtensionDescriptor> {
        self.json(
            self.request(reqwest::Method::PUT, &format!("/v1/plugins/{}", encode(id)))
                .json(&SetPluginEnabled {
                    scope: self.scope.clone(),
                    enabled,
                    expected_revision,
                }),
        )
        .await
    }

    pub(crate) async fn remove_plugin(
        &self,
        id: &str,
        permissions_sha256: String,
    ) -> Result<ExtensionDescriptor> {
        self.json(
            self.request(
                reqwest::Method::DELETE,
                &format!("/v1/plugins/{}", encode(id)),
            )
            .json(&RemovePlugin {
                scope: self.scope.clone(),
                confirmation: ExtensionConfirmation {
                    confirmed: true,
                    permissions_sha256,
                },
            }),
        )
        .await
    }

    pub(crate) async fn apps(&self) -> Result<Vec<PluginAppDescriptor>> {
        self.json(self.request(
            reqwest::Method::GET,
            &format!("/v1/apps?{}", self.catalog_query()),
        ))
        .await
    }

    pub(crate) async fn app(&self, id: &str) -> Result<PluginAppDescriptor> {
        self.json(self.request(
            reqwest::Method::GET,
            &format!("/v1/apps/{}?{}", encode(id), self.catalog_query()),
        ))
        .await
    }

    pub(crate) async fn session_preferences(&self, session: &Id) -> Result<SessionPreferences> {
        self.json(self.request(
            reqwest::Method::GET,
            &format!(
                "/v1/sessions/{}/preferences?{}",
                encode(&session.0),
                self.catalog_query()
            ),
        ))
        .await
    }

    pub(crate) async fn update_session_preferences(
        &self,
        session: &Id,
        input: UpdateSessionPreferences,
    ) -> Result<SessionPreferences> {
        self.json(
            self.request(
                reqwest::Method::PATCH,
                &format!("/v1/sessions/{}/preferences", encode(&session.0)),
            )
            .json(&input),
        )
        .await
    }

    pub(crate) async fn session_goal(&self, session: &Id) -> Result<Option<SessionGoal>> {
        self.json(self.request(
            reqwest::Method::GET,
            &format!(
                "/v1/sessions/{}/goal?{}",
                encode(&session.0),
                self.catalog_query()
            ),
        ))
        .await
    }

    pub(crate) async fn set_session_goal(
        &self,
        session: &Id,
        objective: String,
    ) -> Result<SessionGoal> {
        self.json(
            self.request(
                reqwest::Method::PUT,
                &format!("/v1/sessions/{}/goal", encode(&session.0)),
            )
            .json(&SetSessionGoal {
                scope: self.scope.clone(),
                objective,
                auto_continue: true,
                token_budget: None,
            }),
        )
        .await
    }

    pub(crate) async fn update_session_goal(
        &self,
        session: &Id,
        input: UpdateSessionGoal,
    ) -> Result<SessionGoal> {
        self.json(
            self.request(
                reqwest::Method::PATCH,
                &format!("/v1/sessions/{}/goal", encode(&session.0)),
            )
            .json(&input),
        )
        .await
    }

    pub(crate) async fn clear_session_goal(
        &self,
        session: &Id,
        expected_revision: u64,
    ) -> Result<SessionGoal> {
        self.json(
            self.request(
                reqwest::Method::DELETE,
                &format!("/v1/sessions/{}/goal", encode(&session.0)),
            )
            .json(&ClearSessionGoal {
                scope: self.scope.clone(),
                expected_revision,
            }),
        )
        .await
    }

    pub(crate) async fn durable_tasks(&self) -> Result<Vec<DurableTaskSummary>> {
        self.json(self.request(
            reqwest::Method::GET,
            &format!("/v1/durable-task-summaries?{}", self.catalog_query()),
        ))
        .await
    }

    pub(crate) async fn control_durable_task(
        &self,
        task: &Id,
        action: &str,
    ) -> Result<DurableTaskSummary> {
        if !matches!(action, "pause" | "resume" | "cancel") {
            return Err(anyhow!("unsupported durable task action"));
        }
        self.json(
            self.request(
                reqwest::Method::POST,
                &format!("/v1/durable-tasks/{}/{}", encode(&task.0), encode(action)),
            )
            .json(&self.scope),
        )
        .await
    }

    pub(crate) async fn background_terminals(&self) -> Result<Vec<BackgroundTerminalSummary>> {
        self.json(self.request(
            reqwest::Method::GET,
            &format!("/v1/background-terminals?{}", self.catalog_query()),
        ))
        .await
    }

    pub(crate) async fn preview_background_terminal(
        &self,
        terminal: BackgroundTerminalSpec,
    ) -> Result<BackgroundTerminalPreview> {
        self.json(
            self.request(reqwest::Method::POST, "/v1/background-terminals/preview")
                .json(&PreviewBackgroundTerminal {
                    scope: self.scope.clone(),
                    terminal,
                }),
        )
        .await
    }

    pub(crate) async fn start_background_terminal(
        &self,
        terminal: BackgroundTerminalSpec,
        permissions_sha256: String,
    ) -> Result<BackgroundTerminalSummary> {
        self.json(
            self.request(reqwest::Method::POST, "/v1/background-terminals")
                .json(&StartBackgroundTerminal {
                    scope: self.scope.clone(),
                    terminal,
                    confirmation: ExtensionConfirmation {
                        confirmed: true,
                        permissions_sha256,
                    },
                }),
        )
        .await
    }

    pub(crate) async fn background_terminal_output(
        &self,
        terminal: &Id,
        offset: u64,
    ) -> Result<BackgroundTerminalOutput> {
        self.json(self.request(
            reqwest::Method::GET,
            &format!(
                "/v1/background-terminals/{}/output?{}&offset={offset}&limit=65536",
                encode(&terminal.0),
                self.catalog_query()
            ),
        ))
        .await
    }

    pub(crate) async fn write_background_terminal(
        &self,
        terminal: &Id,
        content: &[u8],
    ) -> Result<BackgroundTerminalSummary> {
        self.json(
            self.request(
                reqwest::Method::POST,
                &format!("/v1/background-terminals/{}/input", encode(&terminal.0)),
            )
            .json(&WriteBackgroundTerminal {
                scope: self.scope.clone(),
                content_base64: STANDARD.encode(content),
            }),
        )
        .await
    }

    pub(crate) async fn resize_background_terminal(
        &self,
        terminal: &Id,
        rows: u16,
        cols: u16,
    ) -> Result<BackgroundTerminalSummary> {
        self.json(
            self.request(
                reqwest::Method::POST,
                &format!("/v1/background-terminals/{}/resize", encode(&terminal.0)),
            )
            .json(&ResizeBackgroundTerminal {
                scope: self.scope.clone(),
                rows,
                cols,
            }),
        )
        .await
    }

    pub(crate) async fn stop_background_terminal(
        &self,
        terminal: &Id,
        expected_revision: u64,
    ) -> Result<BackgroundTerminalSummary> {
        self.json(
            self.request(
                reqwest::Method::POST,
                &format!("/v1/background-terminals/{}/stop", encode(&terminal.0)),
            )
            .json(&StopBackgroundTerminal {
                scope: self.scope.clone(),
                expected_revision,
            }),
        )
        .await
    }

    pub(crate) async fn side_conversations(&self) -> Result<Vec<SideConversation>> {
        self.json(self.request(
            reqwest::Method::GET,
            &format!("/v1/side-conversations?{}", self.catalog_query()),
        ))
        .await
    }

    pub(crate) async fn create_side_conversation(
        &self,
        session: &Id,
        prompt: String,
    ) -> Result<SideConversationStart> {
        self.json(
            self.request(
                reqwest::Method::POST,
                &format!("/v1/sessions/{}/side-conversations", encode(&session.0)),
            )
            .json(&json!({
                "scope": self.scope,
                "source_turn_id": null,
                "prompt": prompt,
            })),
        )
        .await
    }

    pub(crate) async fn promote_side_conversation(
        &self,
        conversation: &Id,
    ) -> Result<SideConversation> {
        self.json(
            self.request(
                reqwest::Method::POST,
                &format!("/v1/side-conversations/{}/promote", encode(&conversation.0)),
            )
            .json(&json!({"scope": self.scope})),
        )
        .await
    }

    pub(crate) async fn close_side_conversation(
        &self,
        conversation: &Id,
    ) -> Result<SideConversation> {
        self.json(
            self.request(
                reqwest::Method::DELETE,
                &format!("/v1/side-conversations/{}", encode(&conversation.0)),
            )
            .json(&json!({"scope": self.scope})),
        )
        .await
    }

    pub(crate) async fn agent_runs(&self) -> Result<Vec<AgentRunSummary>> {
        self.json(self.request(
            reqwest::Method::GET,
            &format!("/v1/agents?{}", self.catalog_query()),
        ))
        .await
    }

    pub(crate) async fn agent_run(&self, agent: &Id) -> Result<AgentRunSummary> {
        self.json(self.request(
            reqwest::Method::GET,
            &format!("/v1/agents/{}?{}", encode(&agent.0), self.catalog_query()),
        ))
        .await
    }

    pub(crate) async fn agent_results(&self) -> Result<Vec<AgentResultSummary>> {
        self.json(self.request(
            reqwest::Method::GET,
            &format!("/v1/agent-results?{}", self.catalog_query()),
        ))
        .await
    }

    pub(crate) async fn agent_result(&self, agent: &Id) -> Result<AgentResultSummary> {
        self.json(self.request(
            reqwest::Method::GET,
            &format!(
                "/v1/agent-results/{}?{}",
                encode(&agent.0),
                self.catalog_query()
            ),
        ))
        .await
    }

    pub(crate) async fn wait_agent(
        &self,
        agent: &Id,
        timeout_seconds: u64,
    ) -> Result<AgentRunSummary> {
        self.json(
            self.request(
                reqwest::Method::POST,
                &format!("/v1/agents/{}/wait", encode(&agent.0)),
            )
            .json(&AgentWait {
                scope: self.scope.clone(),
                timeout_seconds,
            }),
        )
        .await
    }

    pub(crate) async fn close_agent(&self, agent: &Id) -> Result<AgentRunSummary> {
        self.json(
            self.request(
                reqwest::Method::POST,
                &format!("/v1/agents/{}/close", encode(&agent.0)),
            )
            .json(&self.scope),
        )
        .await
    }

    pub(crate) async fn team_goal_runs(&self, goal: &Id) -> Result<Vec<TeamGoalRun>> {
        self.json(self.request(
            reqwest::Method::GET,
            &format!(
                "/v1/teams/{}/goals/{}/runs?organization_id={}&actor_id={}",
                encode(&self.scope.team_id.0),
                encode(&goal.0),
                encode(&self.scope.organization_id.0),
                encode(&self.scope.actor_id.0),
            ),
        ))
        .await
    }

    pub(crate) async fn control_team_goal_run(
        &self,
        goal: &Id,
        run: &Id,
        status: TeamGoalRunStatus,
        expected_revision: u64,
    ) -> Result<TeamGoalRun> {
        self.json(
            self.request(
                reqwest::Method::PATCH,
                &format!(
                    "/v1/teams/{}/goals/{}/runs/{}",
                    encode(&self.scope.team_id.0),
                    encode(&goal.0),
                    encode(&run.0),
                ),
            )
            .json(&UpdateTeamGoalRun {
                scope: self.scope.clone(),
                status,
                expected_revision,
            }),
        )
        .await
    }

    pub(crate) async fn create_agent_follow_up(
        &self,
        agent: &Id,
        content: String,
    ) -> Result<DurableTaskSummary> {
        self.json(
            self.request(
                reqwest::Method::POST,
                &format!("/v1/agents/{}/follow-up", encode(&agent.0)),
            )
            .json(&AgentFollowUp {
                scope: self.scope.clone(),
                content: Value::String(content),
                idempotency_key: format!("cli-follow-up:{}", Id::new("request").0),
                max_attempts: 3,
                max_runtime_seconds: 3_600,
                max_cost_micros: 5_000_000,
            }),
        )
        .await
    }

    pub(crate) async fn messages(&self, session: &Id) -> Result<Vec<Message>> {
        let query = format!(
            "organization_id={}&team_id={}&actor_id={}",
            encode(&self.scope.organization_id.0),
            encode(&self.scope.team_id.0),
            encode(&self.scope.actor_id.0)
        );
        self.json(self.request(
            reqwest::Method::GET,
            &format!("/v1/sessions/{}/messages?{query}", encode(&session.0)),
        ))
        .await
    }

    pub(crate) async fn turns(&self, session: &Id) -> Result<Vec<Turn>> {
        let query = format!(
            "organization_id={}&team_id={}&actor_id={}",
            encode(&self.scope.organization_id.0),
            encode(&self.scope.team_id.0),
            encode(&self.scope.actor_id.0)
        );
        self.json(self.request(
            reqwest::Method::GET,
            &format!("/v1/sessions/{}/turns?{query}", encode(&session.0)),
        ))
        .await
    }

    pub(crate) async fn transcript_snapshot(&self, session: &Id) -> Result<TranscriptSnapshot> {
        self.transcript_snapshot_page(session, None, 1000).await
    }

    pub(crate) async fn transcript_snapshot_page(
        &self,
        session: &Id,
        before: Option<&str>,
        limit: usize,
    ) -> Result<TranscriptSnapshot> {
        let query = format!(
            "organization_id={}&team_id={}&actor_id={}&limit={}{}",
            encode(&self.scope.organization_id.0),
            encode(&self.scope.team_id.0),
            encode(&self.scope.actor_id.0),
            limit.clamp(1, 1000),
            before
                .map(|cursor| format!("&before={}", encode(cursor)))
                .unwrap_or_default(),
        );
        self.json(self.request(
            reqwest::Method::GET,
            &format!("/v1/sessions/{}/snapshot?{query}", encode(&session.0)),
        ))
        .await
    }

    pub(crate) async fn workspace_paths(
        &self,
        session: &Id,
        query: &str,
        limit: usize,
    ) -> Result<Vec<WorkspacePathMatch>> {
        let query = format!(
            "organization_id={}&team_id={}&actor_id={}&query={}&limit={}",
            encode(&self.scope.organization_id.0),
            encode(&self.scope.team_id.0),
            encode(&self.scope.actor_id.0),
            encode(query),
            limit.clamp(1, 100),
        );
        self.json(self.request(
            reqwest::Method::GET,
            &format!("/v1/sessions/{}/files?{query}", encode(&session.0)),
        ))
        .await
    }

    pub(crate) async fn context_summary(&self, session: &Id) -> Result<ContextSummary> {
        let query = format!(
            "organization_id={}&team_id={}&actor_id={}",
            encode(&self.scope.organization_id.0),
            encode(&self.scope.team_id.0),
            encode(&self.scope.actor_id.0)
        );
        self.json(self.request(
            reqwest::Method::GET,
            &format!("/v1/sessions/{}/context?{query}", encode(&session.0)),
        ))
        .await
    }

    pub(crate) async fn compact_session(
        &self,
        session: &Id,
        focus: Option<String>,
    ) -> Result<CompactSessionResult> {
        self.json(
            self.request(
                reqwest::Method::POST,
                &format!("/v1/sessions/{}/compact", encode(&session.0)),
            )
            .json(&CompactSession {
                scope: self.scope.clone(),
                focus,
            }),
        )
        .await
    }

    pub(crate) async fn memories(&self, session: &Id) -> Result<Vec<MemoryItem>> {
        self.json(self.request(
            reqwest::Method::GET,
            &format!(
                "/v1/sessions/{}/memories?{}",
                encode(&session.0),
                self.catalog_query()
            ),
        ))
        .await
    }

    pub(crate) async fn create_memory(
        &self,
        session: &Id,
        memory_scope: MemoryScope,
        citation: String,
        content: String,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<MemoryItem> {
        self.json(
            self.request(
                reqwest::Method::POST,
                &format!("/v1/sessions/{}/memories", encode(&session.0)),
            )
            .json(&CreateMemory {
                scope: self.scope.clone(),
                memory_scope,
                content,
                citation,
                expires_at,
            }),
        )
        .await
    }

    pub(crate) async fn delete_memory(&self, memory: &Id) -> Result<()> {
        let response = self
            .send(self.request(
                reqwest::Method::DELETE,
                &format!(
                    "/v1/memories/{}?{}",
                    encode(&memory.0),
                    self.catalog_query()
                ),
            ))
            .await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("daemon returned {status}: {body}"));
        }
        Ok(())
    }

    pub(crate) async fn create_session(
        &self,
        workspace_uri: String,
        model: String,
    ) -> Result<Session> {
        self.json(
            self.request(reqwest::Method::POST, "/v1/sessions")
                .json(&CreateSession {
                    scope: self.scope.clone(),
                    workspace_uri,
                    title: "Terminal Team Session".into(),
                    model,
                }),
        )
        .await
    }

    pub(crate) async fn start_turn(
        &self,
        session: &Id,
        content: String,
        attachment_ids: Vec<Id>,
        generate_title: bool,
    ) -> Result<Id> {
        let turn: opencoding_protocol::Turn = self
            .json(
                self.request(
                    reqwest::Method::POST,
                    &format!("/v1/sessions/{}/turns", encode(&session.0)),
                )
                .json(&CreateTurn {
                    scope: self.scope.clone(),
                    content: Value::String(content),
                    attachment_ids,
                    generate_title,
                }),
            )
            .await?;
        Ok(turn.id)
    }

    pub(crate) async fn upload_attachment(
        &self,
        session: &Id,
        file_name: String,
        media_type: String,
        content_base64: String,
    ) -> Result<AttachmentMetadata> {
        self.json(
            self.request(
                reqwest::Method::POST,
                &format!("/v1/sessions/{}/attachments", encode(&session.0)),
            )
            .json(&CreateAttachment {
                scope: self.scope.clone(),
                file_name,
                media_type,
                content_base64,
            }),
        )
        .await
    }

    pub(crate) async fn delete_attachment(&self, id: &Id) -> Result<()> {
        let query = format!(
            "organization_id={}&team_id={}&actor_id={}",
            encode(&self.scope.organization_id.0),
            encode(&self.scope.team_id.0),
            encode(&self.scope.actor_id.0)
        );
        let response = self
            .send(self.request(
                reqwest::Method::DELETE,
                &format!("/v1/attachments/{}?{query}", encode(&id.0)),
            ))
            .await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("daemon returned {status}: {body}"));
        }
        Ok(())
    }

    pub(crate) async fn submit_turn_input(
        &self,
        session: &Id,
        target_turn: &Id,
        mode: TurnInputMode,
        content: String,
    ) -> Result<TurnInput> {
        let idempotency_key = Id::new("cli-input").0;
        self.json(
            self.request(
                reqwest::Method::POST,
                &format!("/v1/sessions/{}/inputs", encode(&session.0)),
            )
            .json(&CreateTurnInput {
                scope: self.scope.clone(),
                target_turn_id: target_turn.clone(),
                mode,
                content: Value::String(content),
                idempotency_key,
            }),
        )
        .await
    }

    pub(crate) async fn pending_turn_inputs(&self, session: &Id) -> Result<Vec<TurnInput>> {
        let query = format!(
            "organization_id={}&team_id={}&actor_id={}",
            encode(&self.scope.organization_id.0),
            encode(&self.scope.team_id.0),
            encode(&self.scope.actor_id.0)
        );
        self.json(self.request(
            reqwest::Method::GET,
            &format!("/v1/sessions/{}/inputs?{query}", encode(&session.0)),
        ))
        .await
    }

    pub(crate) async fn turn_input(&self, id: &Id) -> Result<TurnInput> {
        let query = format!(
            "organization_id={}&team_id={}&actor_id={}",
            encode(&self.scope.organization_id.0),
            encode(&self.scope.team_id.0),
            encode(&self.scope.actor_id.0)
        );
        self.json(self.request(
            reqwest::Method::GET,
            &format!("/v1/turn-inputs/{}?{query}", encode(&id.0)),
        ))
        .await
    }

    pub(crate) async fn cancel_turn_input(&self, id: &Id) -> Result<TurnInput> {
        self.json(
            self.request(
                reqwest::Method::DELETE,
                &format!("/v1/turn-inputs/{}", encode(&id.0)),
            )
            .json(&CancelTurnInput {
                scope: self.scope.clone(),
            }),
        )
        .await
    }

    pub(crate) async fn start_review(
        &self,
        session: &Id,
        target: String,
        instructions: Option<String>,
    ) -> Result<Id> {
        let turn: Turn = self
            .json(
                self.request(
                    reqwest::Method::POST,
                    &format!("/v1/sessions/{}/reviews", encode(&session.0)),
                )
                .json(&CreateReview {
                    scope: self.scope.clone(),
                    target,
                    instructions,
                }),
            )
            .await?;
        Ok(turn.id)
    }

    pub(crate) async fn approval(
        &self,
        id: &str,
        approved: bool,
        approval_scope: ApprovalScope,
    ) -> Result<()> {
        let _: Value = self
            .json(
                self.request(
                    reqwest::Method::POST,
                    &format!("/v1/approvals/{}", encode(id)),
                )
                .json(&ResolveApproval {
                    scope: self.scope.clone(),
                    approved,
                    approval_scope,
                }),
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn answer_question(
        &self,
        id: &Id,
        answers: Vec<QuestionAnswer>,
    ) -> Result<QuestionRequest> {
        self.json(
            self.request(
                reqwest::Method::POST,
                &format!("/v1/questions/{}", encode(&id.0)),
            )
            .json(&ResolveQuestion {
                scope: self.scope.clone(),
                answers,
            }),
        )
        .await
    }

    pub(crate) async fn artifact(&self, id: &Id) -> Result<Artifact> {
        let query = format!(
            "organization_id={}&team_id={}&actor_id={}",
            encode(&self.scope.organization_id.0),
            encode(&self.scope.team_id.0),
            encode(&self.scope.actor_id.0)
        );
        self.json(self.request(
            reqwest::Method::GET,
            &format!("/v1/artifacts/{}?{query}", encode(&id.0)),
        ))
        .await
    }

    pub(crate) async fn diff(&self, session: &Id) -> Result<Value> {
        let outcome = self
            .submit_tool(
                session,
                "git_diff",
                json!({"paths": [], "max_bytes": 524288}),
            )
            .await?;
        completed_tool_result(outcome)
    }

    pub(crate) async fn submit_tool(
        &self,
        session: &Id,
        tool: &str,
        arguments: Value,
    ) -> Result<Value> {
        self.json(
            self.request(
                reqwest::Method::POST,
                &format!("/v1/sessions/{}/tools", encode(&session.0)),
            )
            .json(&SubmitToolCall {
                scope: self.scope.clone(),
                tool: tool.into(),
                arguments,
            }),
        )
        .await
    }

    pub(crate) async fn undo_turn(&self, turn: &Id) -> Result<Value> {
        self.json(
            self.request(
                reqwest::Method::POST,
                &format!("/v1/turns/{}/undo", encode(&turn.0)),
            )
            .json(&json!({"scope": self.scope})),
        )
        .await
    }

    pub(crate) async fn turn_undo_impact(&self, turn: &Id) -> Result<TurnUndoImpactPreview> {
        let query = format!(
            "organization_id={}&team_id={}&actor_id={}",
            encode(&self.scope.organization_id.0),
            encode(&self.scope.team_id.0),
            encode(&self.scope.actor_id.0)
        );
        self.json(self.request(
            reqwest::Method::GET,
            &format!("/v1/turns/{}/undo-impact?{query}", encode(&turn.0)),
        ))
        .await
    }

    pub(crate) async fn cancel_turn(&self, turn: &Id) -> Result<()> {
        let _: Value = self
            .json(
                self.request(
                    reqwest::Method::POST,
                    &format!("/v1/turns/{}/cancel", encode(&turn.0)),
                )
                .json(&CancelTurn {
                    scope: self.scope.clone(),
                }),
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn update_session(
        &self,
        session: &Id,
        title: Option<String>,
        status: Option<SessionStatus>,
    ) -> Result<Session> {
        self.json(
            self.request(
                reqwest::Method::PATCH,
                &format!("/v1/sessions/{}", encode(&session.0)),
            )
            .json(&UpdateSession {
                scope: self.scope.clone(),
                title,
                status,
                model: None,
            }),
        )
        .await
    }

    pub(crate) async fn update_session_model(
        &self,
        session: &Id,
        model: String,
    ) -> Result<Session> {
        self.json(
            self.request(
                reqwest::Method::PATCH,
                &format!("/v1/sessions/{}", encode(&session.0)),
            )
            .json(&UpdateSession {
                scope: self.scope.clone(),
                title: None,
                status: None,
                model: Some(model),
            }),
        )
        .await
    }

    pub(crate) async fn fork_session(
        &self,
        session: &Id,
        title: Option<String>,
    ) -> Result<Session> {
        self.json(
            self.request(
                reqwest::Method::POST,
                &format!("/v1/sessions/{}/fork", encode(&session.0)),
            )
            .json(&ForkSession {
                scope: self.scope.clone(),
                title,
                source_turn_id: None,
            }),
        )
        .await
    }

    pub(crate) async fn retry_turn(
        &self,
        turn: &Id,
        content: Option<Value>,
    ) -> Result<RetryTurnResult> {
        self.json(
            self.request(
                reqwest::Method::POST,
                &format!("/v1/turns/{}/retry", encode(&turn.0)),
            )
            .json(&RetryTurn {
                scope: self.scope.clone(),
                content,
            }),
        )
        .await
    }

    pub(crate) async fn archive_session(&self, session: &Id) -> Result<Session> {
        self.json(
            self.request(
                reqwest::Method::POST,
                &format!("/v1/sessions/{}/cancel", encode(&session.0)),
            )
            .json(&SessionLifecycleRequest {
                scope: self.scope.clone(),
            }),
        )
        .await
    }

    pub(crate) async fn delete_session(&self, session: &Id) -> Result<Session> {
        self.json(
            self.request(
                reqwest::Method::DELETE,
                &format!("/v1/sessions/{}", encode(&session.0)),
            )
            .json(&SessionLifecycleRequest {
                scope: self.scope.clone(),
            }),
        )
        .await
    }
}

pub(crate) fn completed_tool_result(outcome: Value) -> Result<Value> {
    let state = outcome["outcome"]
        .as_str()
        .context("tool response is missing its outcome")?;
    if state != "completed" {
        return Err(anyhow!("tool did not complete: {state}"));
    }
    outcome
        .pointer("/tool_call/result")
        .filter(|result| !result.is_null())
        .cloned()
        .context("completed tool response is missing its result")
}

pub(crate) fn encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::State,
        http::HeaderMap,
        routing::{get, post},
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    async fn unauthorized(State(hits): State<Arc<AtomicUsize>>) -> StatusCode {
        hits.fetch_add(1, Ordering::SeqCst);
        StatusCode::UNAUTHORIZED
    }

    async fn authorized(
        State(hits): State<Arc<AtomicUsize>>,
        headers: HeaderMap,
    ) -> (StatusCode, Json<Value>) {
        hits.fetch_add(1, Ordering::SeqCst);
        if headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            == Some("Bearer current-token")
        {
            (StatusCode::OK, Json(json!({"value": 42})))
        } else {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "unauthorized"})),
            )
        }
    }

    async fn serve(router: Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (format!("http://{address}"), task)
    }

    fn scope() -> Scope {
        Scope {
            organization_id: Id("org".into()),
            team_id: Id("team".into()),
            actor_id: Id("user".into()),
            goal_id: None,
            task_id: None,
        }
    }

    #[tokio::test]
    async fn local_api_refreshes_connection_after_unauthorized_and_retries_once() {
        let stale_hits = Arc::new(AtomicUsize::new(0));
        let current_hits = Arc::new(AtomicUsize::new(0));
        let (stale_base, stale_task) = serve(
            Router::new()
                .route("/v1/test", post(unauthorized))
                .with_state(stale_hits.clone()),
        )
        .await;
        let (current_base, current_task) = serve(
            Router::new()
                .route("/v1/test", post(authorized))
                .with_state(current_hits.clone()),
        )
        .await;
        let discovery_hits = Arc::new(AtomicUsize::new(0));
        let discovery = {
            let discovery_hits = discovery_hits.clone();
            let current_base = current_base.clone();
            Arc::new(move || {
                discovery_hits.fetch_add(1, Ordering::SeqCst);
                Ok(LocalDaemonConnection::new(
                    current_base.clone(),
                    "current-token".into(),
                ))
            }) as LocalDiscovery
        };
        let api =
            Api::new_with_discovery(stale_base, "stale-token".into(), scope(), Some(discovery));

        let first: Value = api
            .json(
                api.request(reqwest::Method::POST, "/v1/test")
                    .json(&json!({"request": "preserved"})),
            )
            .await
            .unwrap();
        let second: Value = api
            .json(
                api.request(reqwest::Method::POST, "/v1/test")
                    .json(&json!({"request": "preserved"})),
            )
            .await
            .unwrap();

        assert_eq!(first["value"], 42);
        assert_eq!(second["value"], 42);
        assert_eq!(stale_hits.load(Ordering::SeqCst), 1);
        assert_eq!(current_hits.load(Ordering::SeqCst), 2);
        assert_eq!(discovery_hits.load(Ordering::SeqCst), 1);
        stale_task.abort();
        current_task.abort();
    }

    #[tokio::test]
    async fn explicit_api_does_not_replace_user_supplied_credentials() {
        let hits = Arc::new(AtomicUsize::new(0));
        let (base, task) = serve(
            Router::new()
                .route("/v1/test", get(unauthorized))
                .with_state(hits.clone()),
        )
        .await;
        let api = Api::new(base, "explicit-token".into(), scope());

        let response = api
            .send(api.request(reqwest::Method::GET, "/v1/test"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        task.abort();
    }
}
