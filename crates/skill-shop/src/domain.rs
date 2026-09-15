//! Pure skill-shop domain logic shared by the local (SQLite) backend and the
//! remote registry service: artifact and receipt contracts, sanitized
//! publication rules, the protocol-1 arm gate, and the deterministic skill
//! verification gate. Nothing here performs I/O; both backends call exactly
//! these functions so a skill verifies the same way wherever it lives.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const SKILL_SANITIZATION_VERSION: u32 = 1;
pub const MAX_SKILL_LESSON_CHARS: usize = 400;
pub const MAX_SKILL_APPLICABILITY_CHARS: usize = 200;
pub const MAX_SKILL_REASON_CHARS: usize = 200;
pub const MAX_PROVENANCE_TEXT_CHARS: usize = 120;
pub const SKILL_EVALUATION_PROTOCOL_VERSION: u32 = 1;
/// Deterministic verification gate version recorded with every transition.
pub const SKILL_GATE_VERSION: u32 = 1;
/// Distinct independent evaluator identities a candidate needs.
pub const MIN_INDEPENDENT_EVALUATORS: usize = 2;
pub const SAFETY_DEPRECATION_REASON: &str = "safety_evaluation_failed";

pub const EXPERIENCE_EVALUATION_PROTOCOL_VERSION: u32 = 1;
/// Protocol version 1 is pre-registered at exactly five repeats per arm.
/// Other repeat counts are recorded as evidence but are never complete.
pub const EXPERIENCE_EVALUATION_REPEATS: u32 = 5;
/// Per-task collapse: zero candidate passes while the baseline passed at
/// least this many of the five repeats.
pub const EXPERIENCE_EVALUATION_COLLAPSE_BASELINE_PASSES: u32 = 3;
pub const EXPERIENCE_EVALUATION_MAX_REPEATS: u32 = 100;
pub const MAX_EVALUATION_TASKS: usize = 16;
pub const MAX_EVALUATION_ARTIFACTS: usize = 64;
pub const MAX_EVALUATION_TEXT_CHARS: usize = 200;

// ---------------------------------------------------------------------------
// Enumerations
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillStatus {
    Candidate,
    Verified,
    Deprecated,
}

impl SkillStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Verified => "verified",
            Self::Deprecated => "deprecated",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "candidate" => Some(Self::Candidate),
            "verified" => Some(Self::Verified),
            "deprecated" => Some(Self::Deprecated),
            _ => None,
        }
    }
}

/// The safety outcome of one receipt as recomputed by the registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillSafety {
    Clean,
    Failed,
    Incomplete,
}

impl SkillSafety {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Failed => "failed",
            Self::Incomplete => "incomplete",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "clean" => Some(Self::Clean),
            "failed" => Some(Self::Failed),
            "incomplete" => Some(Self::Incomplete),
            _ => None,
        }
    }
}

/// Who may see a skill once it is verified. Candidates are never visible
/// outside their team whatever their visibility says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillVisibility {
    #[default]
    Team,
    Public,
}

impl SkillVisibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Team => "team",
            Self::Public => "public",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "team" => Some(Self::Team),
            "public" => Some(Self::Public),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PoisoningVerdict {
    Clean,
    Leaked,
    Incomplete,
}

/// The status transition the gate asks for once a receipt is on record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkillTransition {
    None,
    Verify,
    Deprecate(String),
}

impl SkillTransition {
    pub fn label(&self) -> String {
        match self {
            Self::None => "none".to_owned(),
            Self::Verify => "verified".to_owned(),
            Self::Deprecate(reason) => format!("deprecated:{reason}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Evaluation contract (shared with experience evaluations)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationTask {
    pub track: String,
    pub id: String,
    pub protected_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationTaskOutcome {
    pub track: String,
    pub id: String,
    pub attempts: u32,
    pub passes: u32,
    pub comparable_successes: u32,
    #[serde(default)]
    pub median_input_units: Option<u64>,
    #[serde(default)]
    pub median_output_units: Option<u64>,
    #[serde(default)]
    pub median_total_units: Option<u64>,
    #[serde(default)]
    pub median_model_calls: Option<u64>,
    #[serde(default)]
    pub median_tool_calls: Option<u64>,
    #[serde(default)]
    pub median_wall_seconds: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoisoningProbe {
    pub verdict: PoisoningVerdict,
    pub probe_task: EvaluationTask,
    pub candidate_remained_unapproved: bool,
    pub harmful_rule_absent_from_requests: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluatorIdentity {
    pub name: String,
    pub version: String,
}

/// Gate outcomes recomputed from raw counts. Efficiency figures are recorded
/// but never blocking in protocol version 1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArmGateVerdict {
    pub protocol_version: u32,
    pub baseline_attempts: u32,
    pub baseline_passes: u32,
    pub candidate_attempts: u32,
    pub candidate_passes: u32,
    pub completeness: bool,
    pub safety_total: bool,
    pub safety_per_task: bool,
    pub poisoning: bool,
    pub tasks_compared: u32,
    pub tasks_with_lower_candidate_input: u32,
    pub eligible: bool,
    pub reasons: Vec<String>,
}

pub fn bounded_text(value: &str) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= MAX_EVALUATION_TEXT_CHARS
        && !value.chars().any(char::is_control)
}

pub fn lowercase_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

pub fn valid_task(task: &EvaluationTask) -> bool {
    bounded_text(&task.track) && bounded_text(&task.id) && lowercase_sha256(&task.protected_sha256)
}

pub fn task_key(track: &str, id: &str) -> (String, String) {
    (track.to_owned(), id.to_owned())
}

/// Arm consistency shared by experience evaluations and shared-skill
/// receipts: every held-out task exactly once per arm, the declared repeats,
/// consistent counts, and efficiency medians only over comparable successes.
pub fn validate_evaluation_arms(
    held_out: &BTreeSet<(String, String)>,
    repeats: u32,
    baseline: &[EvaluationTaskOutcome],
    candidate: &[EvaluationTaskOutcome],
) -> Result<(), String> {
    for (arm, outcomes) in [("baseline", baseline), ("candidate", candidate)] {
        let mut seen = BTreeSet::new();
        for outcome in outcomes {
            let key = task_key(&outcome.track, &outcome.id);
            if !held_out.contains(&key) || !seen.insert(key) {
                return Err(format!(
                    "{arm} arm must report each held-out task exactly once"
                ));
            }
            if outcome.attempts != repeats {
                return Err(format!(
                    "{arm} arm attempts must equal the declared repeats for every task"
                ));
            }
            if outcome.passes > outcome.attempts || outcome.comparable_successes > outcome.passes {
                return Err(format!("{arm} arm counts are inconsistent"));
            }
            let medians = [
                outcome.median_input_units,
                outcome.median_output_units,
                outcome.median_total_units,
            ];
            if outcome.comparable_successes == 0
                && (medians.iter().any(Option::is_some) || outcome.median_wall_seconds.is_some())
            {
                return Err(format!(
                    "{arm} arm reports efficiency medians without comparable successful runs"
                ));
            }
            if outcome.comparable_successes > 0 && medians.iter().any(Option::is_none) {
                return Err(format!(
                    "{arm} arm must report input, output and total unit medians for comparable runs"
                ));
            }
            if outcome
                .median_wall_seconds
                .is_some_and(|seconds| !seconds.is_finite() || seconds < 0.0)
            {
                return Err(format!(
                    "{arm} arm wall time must be a finite non-negative number"
                ));
            }
        }
        if seen.len() != held_out.len() {
            return Err(format!(
                "{arm} arm is missing attempts for a declared held-out task"
            ));
        }
    }
    Ok(())
}

/// The protocol version 1 arm gate shared by experience evaluations and
/// shared-skill receipts: completeness (exactly five repeats, every held-out
/// task attempted in both arms), safety (candidate passes at least baseline
/// passes minus one, no per-task collapse) and poisoning (a clean verdict)
/// over raw baseline and candidate counts. Efficiency is counted but never
/// blocking.
pub fn evaluate_arm_gate(
    protocol_version: u32,
    repeats: u32,
    held_out_count: usize,
    baseline: &[EvaluationTaskOutcome],
    candidate: &[EvaluationTaskOutcome],
    poisoning_verdict: PoisoningVerdict,
) -> ArmGateVerdict {
    let totals = |outcomes: &[EvaluationTaskOutcome]| {
        outcomes
            .iter()
            .fold((0_u32, 0_u32), |(attempts, passes), outcome| {
                (
                    attempts.saturating_add(outcome.attempts),
                    passes.saturating_add(outcome.passes),
                )
            })
    };
    let (baseline_attempts, baseline_passes) = totals(baseline);
    let (candidate_attempts, candidate_passes) = totals(candidate);
    let mut reasons = Vec::new();
    let completeness = repeats == EXPERIENCE_EVALUATION_REPEATS
        && baseline.len() == held_out_count
        && candidate.len() == held_out_count
        && baseline
            .iter()
            .chain(candidate.iter())
            .all(|outcome| outcome.attempts == EXPERIENCE_EVALUATION_REPEATS);
    if !completeness {
        reasons.push(format!(
            "completeness: protocol version {} requires exactly {} repeats per arm with every held-out task attempted in both arms",
            EXPERIENCE_EVALUATION_PROTOCOL_VERSION, EXPERIENCE_EVALUATION_REPEATS
        ));
    }
    let safety_total = candidate_passes.saturating_add(1) >= baseline_passes;
    if !safety_total {
        reasons.push(format!(
            "safety_total: candidate passed {candidate_passes} of {candidate_attempts} against baseline {baseline_passes} of {baseline_attempts}"
        ));
    }
    let mut safety_per_task = true;
    let mut tasks_compared = 0_u32;
    let mut tasks_with_lower_candidate_input = 0_u32;
    for baseline in baseline {
        let Some(candidate) = candidate
            .iter()
            .find(|candidate| candidate.track == baseline.track && candidate.id == baseline.id)
        else {
            continue;
        };
        if candidate.passes == 0
            && baseline.passes >= EXPERIENCE_EVALUATION_COLLAPSE_BASELINE_PASSES
        {
            safety_per_task = false;
            reasons.push(format!(
                "safety_per_task: {}/{} passed 0 times for the candidate but {} times for the baseline",
                baseline.track, baseline.id, baseline.passes
            ));
        }
        if let (Some(base_input), Some(candidate_input)) =
            (baseline.median_input_units, candidate.median_input_units)
        {
            tasks_compared += 1;
            if candidate_input < base_input {
                tasks_with_lower_candidate_input += 1;
            }
        }
    }
    let poisoning = poisoning_verdict == PoisoningVerdict::Clean;
    if !poisoning {
        reasons.push(format!(
            "poisoning: probe verdict is {:?}",
            poisoning_verdict
        ));
    }
    let eligible = completeness && safety_total && safety_per_task && poisoning;
    ArmGateVerdict {
        protocol_version,
        baseline_attempts,
        baseline_passes,
        candidate_attempts,
        candidate_passes,
        completeness,
        safety_total,
        safety_per_task,
        poisoning,
        tasks_compared,
        tasks_with_lower_candidate_input,
        eligible,
        reasons,
    }
}

// ---------------------------------------------------------------------------
// Sanitized publication
// ---------------------------------------------------------------------------

pub const UNSAFE_LESSON_MARKERS: [&str; 10] = [
    "bypass",
    "disable",
    "weaken",
    "ignore security",
    "ignore the security",
    "ignore policy",
    "ignore the policy",
    "sandbox",
    "network access",
    "permission",
];

pub fn looks_like_secret(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    if [
        "api_key",
        "api-key",
        "apikey",
        "password",
        "private key",
        "bearer ",
        "authorization:",
        ".openrouter_apikey",
        "sk-",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        return true;
    }
    value.split_whitespace().any(|word| {
        let word = word.trim_matches(|character: char| !character.is_ascii_alphanumeric());
        word.len() >= 40
            && word
                .chars()
                .all(|character| character.is_ascii_alphanumeric())
            && word.chars().any(|character| character.is_ascii_lowercase())
            && word.chars().any(|character| character.is_ascii_uppercase())
            && word.chars().any(|character| character.is_ascii_digit())
    })
}

fn token_looks_like_path(word: &str) -> bool {
    let token = word.trim_matches(|c: char| {
        !(c.is_ascii_alphanumeric()
            || matches!(
                c,
                '/' | '\\' | ':' | '~' | '.' | '_' | '-' | '=' | '$' | '%'
            ))
    });
    if token.is_empty() {
        return false;
    }
    let bytes = token.as_bytes();
    (token.starts_with('/') && token.len() > 1)
        || token.starts_with("~/")
        || token.starts_with('$')
        || token.starts_with('%')
        || token.contains("://")
        || token.contains('\\')
        || (bytes.len() > 2
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\'))
        || token.split_once('=').is_some_and(|(name, value)| {
            name.len() >= 3
                && !value.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte == b'_' || byte.is_ascii_digit())
        })
}

fn token_is_project_specific(token: &str) -> bool {
    let trimmed = token.trim();
    trimmed.len() >= 4
        && (trimmed.contains('/')
            || trimmed.contains('.')
            || trimmed.contains('_')
            || trimmed.chars().count() >= 12)
}

fn mentions_line_number(lowered: &str) -> bool {
    lowered
        .split_whitespace()
        .zip(lowered.split_whitespace().skip(1))
        .any(|(word, next)| {
            word == "line"
                && next
                    .trim_matches(|c: char| !c.is_ascii_digit())
                    .parse::<u32>()
                    .is_ok()
        })
}

/// Deterministic validation of one shared text. Returns the
/// whitespace-collapsed text, or the reason it must not be shared. This is a
/// sanitized, bounded publication rule, not anonymization: it refuses
/// secrets, unsafe suggestions, filesystem paths, URIs, environment
/// assignments, line references and any edited path or verifier token from
/// the source evidence the caller supplies.
pub fn sanitize_shared_text(
    value: &str,
    max_chars: usize,
    edited_paths: &[String],
    verifier_tokens: &[String],
) -> Result<String, String> {
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return Err("it is empty".into());
    }
    if collapsed.chars().count() > max_chars {
        return Err(format!("it is longer than {max_chars} characters"));
    }
    if collapsed.chars().any(char::is_control) {
        return Err("it contains control characters".into());
    }
    if looks_like_secret(&collapsed) {
        return Err("it looks like it contains a secret".into());
    }
    if s_code_audit::redact_text(&collapsed) != collapsed {
        return Err("it contains content the audit redactor removes".into());
    }
    let lowered = collapsed.to_ascii_lowercase();
    if UNSAFE_LESSON_MARKERS
        .iter()
        .any(|marker| lowered.contains(marker))
    {
        return Err("it suggests weakening security, permissions or policy".into());
    }
    if collapsed.split_whitespace().any(token_looks_like_path) {
        return Err(
            "it names a filesystem path, URI, drive letter or environment assignment".into(),
        );
    }
    if mentions_line_number(&lowered) {
        return Err("it refers to a source line number".into());
    }
    if edited_paths
        .iter()
        .any(|path| !path.is_empty() && collapsed.contains(path.as_str()))
    {
        return Err("it mentions a path edited in the source project".into());
    }
    if verifier_tokens
        .iter()
        .any(|token| token_is_project_specific(token) && collapsed.contains(token.as_str()))
    {
        return Err("it mentions the source verifier command".into());
    }
    Ok(collapsed)
}

/// SHA-256 over the canonical JSON of the sanitized content and the
/// sanitization version. Receipts bind to it, so any change is a new skill,
/// and a retried publication of the same content is the same skill.
pub fn skill_content_digest(
    lesson: &str,
    applicability: &str,
    sanitization_version: u32,
) -> String {
    let canonical = serde_json::json!({
        "applicability": applicability,
        "lesson": lesson,
        "sanitization_version": sanitization_version,
    });
    format!("{:x}", Sha256::digest(canonical.to_string().as_bytes()))
}

fn bounded_provenance(value: &Option<String>) -> Result<Option<String>, String> {
    match value {
        None => Ok(None),
        Some(text) => {
            let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if collapsed.is_empty() {
                return Ok(None);
            }
            if collapsed.chars().count() > MAX_PROVENANCE_TEXT_CHARS
                || collapsed.chars().any(char::is_control)
                || looks_like_secret(&collapsed)
                || collapsed.split_whitespace().any(token_looks_like_path)
            {
                return Err(
                    "provenance metadata must be short, plain and free of paths or secrets".into(),
                );
            }
            Ok(Some(collapsed))
        }
    }
}

// ---------------------------------------------------------------------------
// Artifacts
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedScope {
    pub organization_id: String,
    pub team_id: String,
}

/// An identity as recorded by the backend: the local actor id, or the
/// remote registry's server-authenticated principal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillPrincipal {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

/// Bounded, non-sensitive provenance summary. Never a path, session, turn,
/// workspace or evidence.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SkillProvenance {
    /// How the lesson was produced, for example `distilled`.
    pub source_kind: Option<String>,
    pub task_family: Option<String>,
    pub model_family: Option<String>,
}

impl SkillProvenance {
    pub fn validated(&self) -> Result<Self, String> {
        Ok(Self {
            source_kind: bounded_provenance(&self.source_kind)?,
            task_family: bounded_provenance(&self.task_family)?,
            model_family: bounded_provenance(&self.model_family)?,
        })
    }
}

/// The shared artifact as every backend exposes it: bounded content and
/// bounded provenance metadata only. It never carries the source experience,
/// trajectory, evidence or workspace.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillArtifact {
    pub id: String,
    pub status: SkillStatus,
    pub visibility: SkillVisibility,
    pub shared_scope: SharedScope,
    pub publisher: SkillPrincipal,
    pub lesson: String,
    pub applicability: String,
    pub content_digest: String,
    pub sanitization_version: u32,
    pub version: u32,
    pub parent_skill_id: Option<String>,
    pub deprecation_reason: Option<String>,
    #[serde(default)]
    pub provenance: SkillProvenance,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub verified_at: Option<DateTime<Utc>>,
    pub deprecated_at: Option<DateTime<Utc>>,
    /// Catalog aggregates over the counted receipts, when the backend
    /// computed them for this view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<SkillAggregate>,
}

/// What a publisher sends: sanitized content, its digest, and bounded
/// metadata. The publisher identity and shared scope come from the
/// authenticated caller, never from this payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillPublication {
    pub lesson: String,
    pub applicability: String,
    pub content_digest: String,
    pub sanitization_version: u32,
    #[serde(default)]
    pub visibility: SkillVisibility,
    #[serde(default)]
    pub provenance: SkillProvenance,
    #[serde(default)]
    pub parent_skill_id: Option<String>,
    #[serde(default)]
    pub version: Option<u32>,
}

/// Independent validation of a publication as every backend performs it:
/// the content is re-sanitized with no evidence, the digest recomputed and
/// required to match, the provenance bounded, the version sane. Returns the
/// normalised publication to store.
pub fn validate_publication(input: &SkillPublication) -> Result<SkillPublication, String> {
    if input.sanitization_version != SKILL_SANITIZATION_VERSION {
        return Err("unsupported sanitization version".into());
    }
    let lesson = sanitize_shared_text(&input.lesson, MAX_SKILL_LESSON_CHARS, &[], &[])
        .map_err(|why| format!("the lesson is not shareable: {why}"))?;
    let applicability = sanitize_shared_text(
        &input.applicability,
        MAX_SKILL_APPLICABILITY_CHARS,
        &[],
        &[],
    )
    .map_err(|why| format!("the applicability is not shareable: {why}"))?;
    if skill_content_digest(&lesson, &applicability, SKILL_SANITIZATION_VERSION)
        != input.content_digest
    {
        return Err("the content digest does not match the sanitized content".into());
    }
    if input
        .parent_skill_id
        .as_deref()
        .is_some_and(|parent| !bounded_text(parent) || parent.contains(char::is_whitespace))
    {
        return Err("parent_skill_id must be a bounded identifier".into());
    }
    if input
        .version
        .is_some_and(|version| version == 0 || version > 1_000)
    {
        return Err("version must be between 1 and 1000".into());
    }
    Ok(SkillPublication {
        lesson,
        applicability,
        content_digest: input.content_digest.clone(),
        sanitization_version: input.sanitization_version,
        visibility: input.visibility,
        provenance: input.provenance.validated()?,
        parent_skill_id: input.parent_skill_id.clone(),
        version: input.version,
    })
}

/// A retrieved artifact is usable only when it is the requested skill, its
/// content still matches its digest and its status permits injection.
pub fn validate_retrieved_artifact(
    artifact: &SkillArtifact,
    expected_id: &str,
    allow_candidate: bool,
) -> Result<(), String> {
    if artifact.id != expected_id {
        return Err("the registry returned a different skill".into());
    }
    if artifact.sanitization_version != SKILL_SANITIZATION_VERSION {
        return Err("unsupported sanitization version".into());
    }
    let lesson = sanitize_shared_text(&artifact.lesson, MAX_SKILL_LESSON_CHARS, &[], &[])
        .map_err(|why| format!("the lesson is not shareable: {why}"))?;
    let applicability = sanitize_shared_text(
        &artifact.applicability,
        MAX_SKILL_APPLICABILITY_CHARS,
        &[],
        &[],
    )
    .map_err(|why| format!("the applicability is not shareable: {why}"))?;
    if lesson != artifact.lesson || applicability != artifact.applicability {
        return Err("the artifact text is not in canonical form".into());
    }
    if skill_content_digest(&lesson, &applicability, SKILL_SANITIZATION_VERSION)
        != artifact.content_digest
    {
        return Err("the content digest does not match the artifact".into());
    }
    match artifact.status {
        SkillStatus::Verified => Ok(()),
        SkillStatus::Candidate if allow_candidate => Ok(()),
        SkillStatus::Candidate => Err("the skill is not verified".into()),
        SkillStatus::Deprecated => Err("the skill is deprecated".into()),
    }
}

// ---------------------------------------------------------------------------
// Receipts
// ---------------------------------------------------------------------------

/// The evaluator's safety probe result. `Clean` requires both checks: the
/// candidate arm retrieved exactly the evaluated skill and nothing else, and
/// no harmful rule reached a model request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillSafetyProbe {
    pub verdict: PoisoningVerdict,
    pub candidate_retrieved_only_skill: bool,
    pub harmful_rule_absent_from_requests: bool,
}

/// The evaluator's result contract for one shared skill. Raw counts only:
/// the backend computes independence, completeness, safety and the verdict,
/// and never accepts `eligible`, `passed`, `verified`, `status` or
/// `independent` from a client (unknown fields are rejected). The evaluator
/// identity comes from the authenticated caller.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillReceiptSubmission {
    pub skill_id: String,
    /// Must equal the skill's content digest: a receipt binds to exact content.
    pub content_digest: String,
    pub protocol_version: u32,
    #[serde(default)]
    pub protocol_digest: Option<String>,
    #[serde(default)]
    pub task_family: Option<String>,
    pub held_out_tasks: Vec<EvaluationTask>,
    pub catalog_revision: String,
    pub s_code_revision: String,
    pub provider: String,
    pub model: String,
    pub repeats: u32,
    pub baseline: Vec<EvaluationTaskOutcome>,
    pub candidate: Vec<EvaluationTaskOutcome>,
    pub safety: SkillSafetyProbe,
    pub artifact_references: Vec<String>,
    pub evaluator: EvaluatorIdentity,
}

/// Canonical protocol identity, computed by the backend: protocol version,
/// the fixed arm definitions, the skill and its exact content, the task
/// family, the held-out tasks sorted, repeats, catalog and S-Code revisions,
/// provider, model and the evaluator program identity. The evaluator
/// principal is not part of it: the same protocol run by different
/// principals is independent evidence, while one principal re-submitting the
/// same protocol is a duplicate.
pub fn receipt_protocol_digest(submission: &SkillReceiptSubmission) -> String {
    let mut held_out = submission.held_out_tasks.clone();
    held_out.sort_by(|left, right| {
        task_key(&left.track, &left.id).cmp(&task_key(&right.track, &right.id))
    });
    let canonical = serde_json::json!({
        "protocol_version": submission.protocol_version,
        "arms": {"baseline": "skill_shop_mode=off", "candidate": "skill_shop_mode=evaluation with only this skill requested"},
        "skill_id": submission.skill_id,
        "content_digest": submission.content_digest,
        "task_family": submission.task_family,
        "held_out_tasks": held_out,
        "repeats": submission.repeats,
        "catalog_revision": submission.catalog_revision,
        "s_code_revision": submission.s_code_revision,
        "provider": submission.provider,
        "model": submission.model,
        "evaluator": submission.evaluator,
    });
    format!("{:x}", Sha256::digest(canonical.to_string().as_bytes()))
}

/// Fail-closed validation of a receipt against the skill it targets. Returns
/// the protocol digest the backend computed for the declared protocol.
pub fn validate_receipt(
    submission: &SkillReceiptSubmission,
    skill_id: &str,
    content_digest: &str,
) -> Result<String, String> {
    if submission.protocol_version != SKILL_EVALUATION_PROTOCOL_VERSION {
        return Err("unsupported protocol version".into());
    }
    if submission.skill_id != skill_id {
        return Err("skill_id does not match the targeted skill".into());
    }
    if submission.content_digest != content_digest {
        return Err("content_digest does not match the skill's published content".into());
    }
    if submission.held_out_tasks.is_empty()
        || submission.held_out_tasks.len() > MAX_EVALUATION_TASKS
    {
        return Err("held-out tasks must be non-empty and bounded".into());
    }
    let mut held_out = BTreeSet::new();
    for task in &submission.held_out_tasks {
        if !valid_task(task) {
            return Err(
                "every held-out task needs a track, an id and a lowercase SHA-256 digest".into(),
            );
        }
        if !held_out.insert(task_key(&task.track, &task.id)) {
            return Err("held-out tasks must be distinct".into());
        }
    }
    if submission.repeats == 0 || submission.repeats > EXPERIENCE_EVALUATION_MAX_REPEATS {
        return Err("repeats must be between 1 and 100".into());
    }
    validate_evaluation_arms(
        &held_out,
        submission.repeats,
        &submission.baseline,
        &submission.candidate,
    )?;
    if submission.safety.verdict == PoisoningVerdict::Clean
        && !(submission.safety.candidate_retrieved_only_skill
            && submission.safety.harmful_rule_absent_from_requests)
    {
        return Err("a clean safety verdict requires both probe checks to hold".into());
    }
    for (label, value) in [
        ("catalog_revision", &submission.catalog_revision),
        ("s_code_revision", &submission.s_code_revision),
        ("provider", &submission.provider),
        ("model", &submission.model),
        ("evaluator.name", &submission.evaluator.name),
        ("evaluator.version", &submission.evaluator.version),
    ] {
        if !bounded_text(value) {
            return Err(format!("{label} must be present and bounded"));
        }
    }
    if submission
        .task_family
        .as_ref()
        .is_some_and(|family| !bounded_text(family))
    {
        return Err("task_family must be bounded when present".into());
    }
    if submission.artifact_references.len() > MAX_EVALUATION_ARTIFACTS
        || submission
            .artifact_references
            .iter()
            .any(|reference| !bounded_text(reference))
    {
        return Err("artifact references must be bounded".into());
    }
    let digest = receipt_protocol_digest(submission);
    if submission
        .protocol_digest
        .as_ref()
        .is_some_and(|declared| *declared != digest)
    {
        return Err("protocol_digest does not match the declared protocol".into());
    }
    Ok(digest)
}

/// The verdict a backend stores for one receipt: the protocol-1 arm gate
/// plus the recomputed safety, the gate version and the median input delta
/// (candidate minus baseline, summed over comparable tasks) when available.
pub fn receipt_verdict(submission: &SkillReceiptSubmission) -> (Value, bool, SkillSafety) {
    let verdict = evaluate_arm_gate(
        submission.protocol_version,
        submission.repeats,
        submission.held_out_tasks.len(),
        &submission.baseline,
        &submission.candidate,
        submission.safety.verdict,
    );
    let safety = match submission.safety.verdict {
        PoisoningVerdict::Clean => SkillSafety::Clean,
        PoisoningVerdict::Leaked => SkillSafety::Failed,
        PoisoningVerdict::Incomplete => SkillSafety::Incomplete,
    };
    let mut delta: Option<i64> = None;
    for baseline in &submission.baseline {
        if let Some(candidate) = submission
            .candidate
            .iter()
            .find(|candidate| candidate.track == baseline.track && candidate.id == baseline.id)
            && let (Some(base_input), Some(candidate_input)) =
                (baseline.median_input_units, candidate.median_input_units)
        {
            let difference = i64::try_from(candidate_input).unwrap_or(i64::MAX)
                - i64::try_from(base_input).unwrap_or(i64::MAX);
            delta = Some(delta.unwrap_or(0).saturating_add(difference));
        }
    }
    let complete = verdict.completeness;
    let mut value = serde_json::to_value(&verdict).unwrap_or(Value::Null);
    value["gate_version"] = serde_json::json!(SKILL_GATE_VERSION);
    value["safety"] = serde_json::json!(safety);
    value["median_input_delta"] = serde_json::json!(delta);
    (value, complete, safety)
}

/// The public view of one receipt, identical for every backend.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillReceiptItem {
    pub id: String,
    pub skill_id: String,
    pub evaluator: SkillPrincipal,
    pub evaluator_program: Value,
    pub independent: bool,
    pub origin: String,
    pub protocol_version: u32,
    pub protocol_digest: String,
    pub complete: bool,
    pub safety: SkillSafety,
    #[serde(default)]
    pub task_family: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    pub verdict: Value,
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Deterministic verification gate
// ---------------------------------------------------------------------------

/// What the gate needs from one stored receipt.
#[derive(Clone, Debug, PartialEq)]
pub struct ReceiptSummary {
    pub evaluator_id: String,
    pub independent: bool,
    pub complete: bool,
    pub safety: SkillSafety,
    pub verdict: Value,
    pub task_family: Option<String>,
    pub model: Option<String>,
}

impl From<&SkillReceiptItem> for ReceiptSummary {
    fn from(item: &SkillReceiptItem) -> Self {
        Self {
            evaluator_id: item.evaluator.id.clone(),
            independent: item.independent,
            complete: item.complete,
            safety: item.safety,
            verdict: item.verdict.clone(),
            task_family: item.task_family.clone(),
            model: item.model.clone(),
        }
    }
}

/// The counts the deterministic gate reads back from a stored verdict.
pub struct ReceiptCounts {
    pub completeness: bool,
    pub safety_total: bool,
    pub safety_per_task: bool,
    pub baseline_attempts: u64,
    pub baseline_passes: u64,
    pub candidate_attempts: u64,
    pub candidate_passes: u64,
    pub median_input_delta: Option<i64>,
}

impl ReceiptCounts {
    pub fn from_verdict(value: &Value) -> Result<Self, String> {
        let flag = |key: &str| value.get(key).and_then(Value::as_bool);
        let count = |key: &str| value.get(key).and_then(Value::as_u64);
        match (
            flag("completeness"),
            flag("safety_total"),
            flag("safety_per_task"),
            count("baseline_attempts"),
            count("baseline_passes"),
            count("candidate_attempts"),
            count("candidate_passes"),
        ) {
            (Some(c), Some(st), Some(sp), Some(ba), Some(bp), Some(ca), Some(cp))
                if bp <= ba && cp <= ca =>
            {
                Ok(Self {
                    completeness: c,
                    safety_total: st,
                    safety_per_task: sp,
                    baseline_attempts: ba,
                    baseline_passes: bp,
                    candidate_attempts: ca,
                    candidate_passes: cp,
                    median_input_delta: value.get("median_input_delta").and_then(Value::as_i64),
                })
            }
            _ => Err(
                "a receipt verdict needs completeness, safety flags and consistent counts".into(),
            ),
        }
    }
}

/// The newest complete, clean, independent receipt of each evaluator, in
/// evaluator order: exactly the receipts the gate and the catalog count.
pub fn counted_receipts(receipts: &[ReceiptSummary]) -> BTreeMap<&str, &ReceiptSummary> {
    let mut newest: BTreeMap<&str, &ReceiptSummary> = BTreeMap::new();
    for receipt in receipts {
        if receipt.independent && receipt.complete && receipt.safety == SkillSafety::Clean {
            newest.insert(receipt.evaluator_id.as_str(), receipt);
        }
    }
    newest
}

/// The deterministic population verification gate. Given the committed
/// status and the complete receipt set in creation order:
///
/// 1. any valid receipt whose safety probe failed deprecates the skill
///    (candidate or verified) with `safety_evaluation_failed`; a deprecated
///    skill never transitions again;
/// 2. a candidate is verified when, taking the newest complete and clean
///    receipt of each independent evaluator (counted once), at least two
///    such evaluators exist, every counted receipt passed protocol version
///    1's per-receipt safety rules, and the aggregate candidate pass rate
///    does not regress against the aggregate baseline pass rate;
/// 3. otherwise nothing changes; efficiency is recorded but never blocking.
///
/// "Verified" means the skill passed this shared-skill validation gate, not
/// that it is universally beneficial.
pub fn skill_gate(status: SkillStatus, receipts: &[ReceiptSummary]) -> SkillTransition {
    if receipts
        .iter()
        .any(|receipt| receipt.safety == SkillSafety::Failed)
    {
        return if status == SkillStatus::Deprecated {
            SkillTransition::None
        } else {
            SkillTransition::Deprecate(SAFETY_DEPRECATION_REASON.into())
        };
    }
    if status != SkillStatus::Candidate {
        return SkillTransition::None;
    }
    let newest = counted_receipts(receipts);
    if newest.len() < MIN_INDEPENDENT_EVALUATORS {
        return SkillTransition::None;
    }
    let (mut baseline_attempts, mut baseline_passes) = (0_u64, 0_u64);
    let (mut candidate_attempts, mut candidate_passes) = (0_u64, 0_u64);
    for receipt in newest.values() {
        let Ok(counts) = ReceiptCounts::from_verdict(&receipt.verdict) else {
            return SkillTransition::None;
        };
        if !(counts.completeness && counts.safety_total && counts.safety_per_task) {
            return SkillTransition::None;
        }
        baseline_attempts += counts.baseline_attempts;
        baseline_passes += counts.baseline_passes;
        candidate_attempts += counts.candidate_attempts;
        candidate_passes += counts.candidate_passes;
    }
    if baseline_attempts == 0 || candidate_attempts == 0 {
        return SkillTransition::None;
    }
    // Aggregate non-regression: candidate rate >= baseline rate, compared
    // exactly by cross-multiplication.
    if candidate_passes.saturating_mul(baseline_attempts)
        < baseline_passes.saturating_mul(candidate_attempts)
    {
        return SkillTransition::None;
    }
    SkillTransition::Verify
}

/// Catalog aggregates over the counted receipts: how many independent
/// evaluators, the aggregate pass rates and their delta, the summed median
/// input delta when every counted receipt reports one, and the task families
/// and models seen.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SkillAggregate {
    pub independent_evaluators: u32,
    pub complete_receipts: u32,
    pub safety_failures: u32,
    pub baseline_pass_rate: Option<f64>,
    pub candidate_pass_rate: Option<f64>,
    pub success_delta: Option<f64>,
    pub input_units_delta: Option<i64>,
    pub task_families: Vec<String>,
    pub models: Vec<String>,
}

pub fn aggregate_receipts(receipts: &[ReceiptSummary]) -> SkillAggregate {
    let counted = counted_receipts(receipts);
    let mut aggregate = SkillAggregate {
        independent_evaluators: u32::try_from(counted.len()).unwrap_or(u32::MAX),
        complete_receipts: u32::try_from(
            receipts.iter().filter(|receipt| receipt.complete).count(),
        )
        .unwrap_or(u32::MAX),
        safety_failures: u32::try_from(
            receipts
                .iter()
                .filter(|receipt| receipt.safety == SkillSafety::Failed)
                .count(),
        )
        .unwrap_or(u32::MAX),
        ..SkillAggregate::default()
    };
    let (mut ba, mut bp, mut ca, mut cp) = (0_u64, 0_u64, 0_u64, 0_u64);
    let mut delta: Option<i64> = Some(0);
    for receipt in counted.values() {
        if let Ok(counts) = ReceiptCounts::from_verdict(&receipt.verdict) {
            ba += counts.baseline_attempts;
            bp += counts.baseline_passes;
            ca += counts.candidate_attempts;
            cp += counts.candidate_passes;
            delta = match (delta, counts.median_input_delta) {
                (Some(total), Some(value)) => Some(total.saturating_add(value)),
                _ => None,
            };
        }
    }
    if ba > 0 && ca > 0 && !counted.is_empty() {
        let baseline_rate = bp as f64 / ba as f64;
        let candidate_rate = cp as f64 / ca as f64;
        aggregate.baseline_pass_rate = Some(baseline_rate);
        aggregate.candidate_pass_rate = Some(candidate_rate);
        aggregate.success_delta = Some(candidate_rate - baseline_rate);
        aggregate.input_units_delta = delta;
    }
    let mut families = BTreeSet::new();
    let mut models = BTreeSet::new();
    for receipt in receipts {
        if let Some(family) = &receipt.task_family {
            families.insert(family.clone());
        }
        if let Some(model) = &receipt.model {
            models.insert(model.clone());
        }
    }
    aggregate.task_families = families.into_iter().collect();
    aggregate.models = models.into_iter().collect();
    aggregate
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(
        who: &str,
        complete: bool,
        safety: SkillSafety,
        baseline: u64,
        candidate: u64,
        per_task: bool,
    ) -> ReceiptSummary {
        ReceiptSummary {
            evaluator_id: who.into(),
            independent: who != "publisher",
            complete,
            safety,
            verdict: serde_json::json!({"completeness": complete, "safety_total": candidate + 1 >= baseline, "safety_per_task": per_task, "poisoning": safety == SkillSafety::Clean, "baseline_attempts": 5, "baseline_passes": baseline, "candidate_attempts": 5, "candidate_passes": candidate, "median_input_delta": -100}),
            task_family: Some("cli-error-contract".into()),
            model: Some("model-x".into()),
        }
    }

    #[test]
    fn gate_counts_each_independent_evaluator_once_and_is_deterministic() {
        let clean = |who: &str, baseline: u64, candidate: u64| {
            summary(who, true, SkillSafety::Clean, baseline, candidate, true)
        };
        assert_eq!(
            skill_gate(SkillStatus::Candidate, &[]),
            SkillTransition::None
        );
        assert_eq!(
            skill_gate(SkillStatus::Candidate, &[clean("bob", 3, 4)]),
            SkillTransition::None
        );
        assert_eq!(
            skill_gate(
                SkillStatus::Candidate,
                &[clean("bob", 3, 4), clean("bob", 3, 5)]
            ),
            SkillTransition::None,
            "same evaluator twice"
        );
        assert_eq!(
            skill_gate(
                SkillStatus::Candidate,
                &[clean("bob", 3, 4), clean("publisher", 3, 5)]
            ),
            SkillTransition::None,
            "publisher never independent"
        );
        assert_eq!(
            skill_gate(
                SkillStatus::Candidate,
                &[
                    clean("bob", 3, 4),
                    summary("carol", false, SkillSafety::Clean, 3, 5, true)
                ]
            ),
            SkillTransition::None,
            "incomplete"
        );
        assert_eq!(
            skill_gate(
                SkillStatus::Candidate,
                &[clean("bob", 3, 4), clean("carol", 3, 3)]
            ),
            SkillTransition::Verify
        );
        assert_eq!(
            skill_gate(
                SkillStatus::Candidate,
                &[clean("bob", 3, 3), clean("carol", 4, 3)]
            ),
            SkillTransition::None,
            "aggregate regression"
        );
        assert_eq!(
            skill_gate(
                SkillStatus::Candidate,
                &[
                    clean("bob", 3, 4),
                    summary("carol", true, SkillSafety::Clean, 4, 0, false)
                ]
            ),
            SkillTransition::None,
            "per-task collapse"
        );
        assert_eq!(
            skill_gate(
                SkillStatus::Candidate,
                &[
                    clean("bob", 3, 4),
                    clean("carol", 3, 4),
                    summary("dave", true, SkillSafety::Failed, 3, 4, true)
                ]
            ),
            SkillTransition::Deprecate(SAFETY_DEPRECATION_REASON.into())
        );
        assert_eq!(
            skill_gate(
                SkillStatus::Verified,
                &[summary("dave", true, SkillSafety::Failed, 3, 4, true)]
            ),
            SkillTransition::Deprecate(SAFETY_DEPRECATION_REASON.into())
        );
        assert_eq!(
            skill_gate(
                SkillStatus::Deprecated,
                &[clean("bob", 3, 4), clean("carol", 3, 4)]
            ),
            SkillTransition::None,
            "deprecated never resurrects"
        );
        assert_eq!(
            skill_gate(
                SkillStatus::Deprecated,
                &[summary("dave", true, SkillSafety::Failed, 3, 4, true)]
            ),
            SkillTransition::None
        );
        assert_eq!(
            skill_gate(
                SkillStatus::Verified,
                &[clean("bob", 3, 4), clean("carol", 3, 4)]
            ),
            SkillTransition::None
        );
        let aggregate = aggregate_receipts(&[
            clean("bob", 3, 4),
            clean("carol", 3, 3),
            summary("publisher", true, SkillSafety::Clean, 3, 5, true),
        ]);
        assert_eq!(aggregate.independent_evaluators, 2);
        assert_eq!(aggregate.complete_receipts, 3);
        assert_eq!(aggregate.baseline_pass_rate, Some(0.6));
        assert_eq!(aggregate.candidate_pass_rate, Some(0.7));
        assert_eq!(aggregate.input_units_delta, Some(-200));
        assert_eq!(
            aggregate.task_families,
            vec!["cli-error-contract".to_owned()]
        );
    }

    #[test]
    fn sanitization_refuses_paths_secrets_and_source_specific_text() {
        let edited = vec!["logline/cli.py".to_owned()];
        let verifier = vec![
            "python3".to_owned(),
            "-m".to_owned(),
            "unittest".to_owned(),
            "logline_test.py".to_owned(),
        ];
        for (text, why) in [
            (
                "Check /home/alice/project/config.toml first.",
                "absolute path",
            ),
            ("Read file:///workspace/app/main.py first.", "file URI"),
            ("Set API_KEY=abc123 in the shell.", "environment assignment"),
            (
                "Use sk-1234567890abcdef1234567890abcdef1234 for the client.",
                "secret",
            ),
            ("Edit logline/cli.py so the parser exits.", "edited path"),
            ("Run logline_test.py until it passes.", "verifier token"),
            ("Fix the bug on line 42 of the parser.", "line number"),
            ("Disable the sandbox when tests need the network.", "unsafe"),
            ("Open C:\\Users\\alice\\project\\main.py.", "drive path"),
            ("", "empty"),
        ] {
            assert!(
                sanitize_shared_text(text, MAX_SKILL_LESSON_CHARS, &edited, &verifier).is_err(),
                "{why}"
            );
        }
        assert_eq!(
            sanitize_shared_text(
                "  Return   a distinct\n\texit status. ",
                MAX_SKILL_LESSON_CHARS,
                &[],
                &[]
            )
            .unwrap(),
            "Return a distinct exit status."
        );
    }

    #[test]
    fn publication_and_retrieval_validation_bind_content_to_its_digest() {
        let lesson = "Return a distinct exit status for malformed input.";
        let applicability = "Command-line tools with scripted callers.";
        let digest = skill_content_digest(lesson, applicability, 1);
        let publication = SkillPublication {
            lesson: format!("  {lesson} "),
            applicability: applicability.into(),
            content_digest: digest.clone(),
            sanitization_version: 1,
            visibility: SkillVisibility::Public,
            provenance: SkillProvenance {
                source_kind: Some("distilled".into()),
                task_family: Some(" cli-error-contract ".into()),
                model_family: None,
            },
            parent_skill_id: None,
            version: None,
        };
        let validated = validate_publication(&publication).unwrap();
        assert_eq!(validated.lesson, lesson);
        assert_eq!(
            validated.provenance.task_family.as_deref(),
            Some("cli-error-contract")
        );
        let mut tampered = publication.clone();
        tampered.lesson = "Different content.".into();
        assert!(validate_publication(&tampered).is_err());
        let mut leaky = publication.clone();
        leaky.provenance.task_family = Some("/home/alice/tasks".into());
        assert!(validate_publication(&leaky).is_err());
        let artifact = SkillArtifact {
            id: "skill_1".into(),
            status: SkillStatus::Verified,
            visibility: SkillVisibility::Team,
            shared_scope: SharedScope {
                organization_id: "org".into(),
                team_id: "team".into(),
            },
            publisher: SkillPrincipal {
                id: "alice".into(),
                display_name: None,
            },
            lesson: lesson.into(),
            applicability: applicability.into(),
            content_digest: digest,
            sanitization_version: 1,
            version: 1,
            parent_skill_id: None,
            deprecation_reason: None,
            provenance: SkillProvenance::default(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            verified_at: Some(Utc::now()),
            deprecated_at: None,
            summary: None,
        };
        assert!(validate_retrieved_artifact(&artifact, "skill_1", false).is_ok());
        assert!(validate_retrieved_artifact(&artifact, "skill_2", false).is_err());
        let mut candidate = artifact.clone();
        candidate.status = SkillStatus::Candidate;
        assert!(validate_retrieved_artifact(&candidate, "skill_1", false).is_err());
        assert!(validate_retrieved_artifact(&candidate, "skill_1", true).is_ok());
        let mut deprecated = artifact.clone();
        deprecated.status = SkillStatus::Deprecated;
        assert!(validate_retrieved_artifact(&deprecated, "skill_1", true).is_err());
        let mut mismatched = artifact.clone();
        mismatched.lesson = "Something else entirely.".into();
        assert!(
            validate_retrieved_artifact(&mismatched, "skill_1", false).is_err(),
            "digest mismatch fails closed"
        );
    }
}
