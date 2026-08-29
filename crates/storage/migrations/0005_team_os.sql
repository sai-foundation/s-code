CREATE TABLE team_goals (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    title TEXT NOT NULL,
    outcome_definition TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('planned','active','achieved','cancelled')),
    target_date TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX team_goals_team_status ON team_goals(team_id, status, updated_at);

CREATE TABLE team_tasks (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    goal_id TEXT REFERENCES team_goals(id),
    source TEXT NOT NULL,
    title TEXT NOT NULL,
    priority INTEGER NOT NULL,
    assignee_type TEXT,
    assignee_id TEXT,
    status TEXT NOT NULL CHECK(status IN ('ready','in_progress','blocked','review','verified','cancelled')),
    acceptance_criteria_json TEXT NOT NULL,
    required_evidence_json TEXT NOT NULL,
    blockers_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX team_tasks_queue ON team_tasks(team_id, status, priority DESC, created_at);

CREATE TABLE team_knowledge (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    source_uri TEXT NOT NULL,
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    version TEXT NOT NULL,
    trust_level TEXT NOT NULL,
    permission TEXT NOT NULL,
    valid_until TEXT,
    owner_team_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(team_id, source_uri, version)
);
CREATE INDEX team_knowledge_active ON team_knowledge(team_id, valid_until, updated_at);

CREATE TABLE team_outcomes (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    goal_id TEXT NOT NULL REFERENCES team_goals(id),
    task_id TEXT NOT NULL UNIQUE REFERENCES team_tasks(id),
    status TEXT NOT NULL CHECK(status IN ('verified')),
    evidence_json TEXT NOT NULL,
    pull_request_url TEXT,
    completed_at TEXT NOT NULL
);
CREATE INDEX team_outcomes_team_completed ON team_outcomes(team_id, completed_at DESC);
