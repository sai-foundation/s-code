use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const BASELINE: &str = include_str!("../../../compliance/baseline.json");

#[derive(Clone, Debug, Deserialize)]
pub struct ComplianceBaseline {
    pub schema_version: u32,
    pub baseline_version: String,
    pub scope: Vec<String>,
    pub controls: Vec<Control>,
    pub assets: Vec<Asset>,
    pub risks: Vec<Risk>,
    pub data_flows: Vec<DataFlow>,
    pub vendors: Vec<Vendor>,
    pub exercises: Vec<Exercise>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Control {
    pub id: String,
    pub domain: String,
    pub title: String,
    pub accountable_team: String,
    pub status: String,
    pub frequency: String,
    pub evidence_source: String,
    pub evidence_artifact: String,
    pub automated_test: String,
    pub exception_workflow: String,
    pub soc2: Vec<String>,
    pub iso27001: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Asset {
    pub id: String,
    pub kind: String,
    pub owner_team: String,
    pub classification: String,
    pub system_of_record: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Risk {
    pub id: String,
    pub owner_team: String,
    pub likelihood: u8,
    pub impact: u8,
    pub treatment: String,
    pub due_date: String,
    pub status: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DataFlow {
    pub id: String,
    pub source: String,
    pub destination: String,
    pub data_classes: Vec<String>,
    pub purpose: String,
    pub retention_policy: String,
    pub subprocessor: Option<String>,
    pub cross_boundary: bool,
    pub approval_control: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Vendor {
    pub id: String,
    pub category: String,
    pub owner_team: String,
    pub data_classes: Vec<String>,
    pub review_status: String,
    pub review_due: String,
    pub dpa_required: bool,
    pub subprocessor: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Exercise {
    pub id: String,
    pub kind: String,
    pub owner_team: String,
    pub status: String,
    pub executed_at: Option<DateTime<Utc>>,
    pub evidence: String,
    pub findings: Vec<String>,
    pub remediation: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EvidenceReport {
    pub schema_version: u32,
    pub baseline_version: String,
    pub baseline_sha256: String,
    pub counts: BTreeMap<String, usize>,
    pub domains: Vec<String>,
    pub external_evidence: Option<VerifiedEvidenceSummary>,
    pub required_external_controls: Vec<String>,
    pub required_external_vendors: Vec<String>,
    pub pending_exercises: Vec<String>,
    pub certification_ready: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SignedEvidenceBundle {
    pub payload: ExternalEvidencePayload,
    pub signature: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalEvidencePayload {
    pub schema_version: u32,
    pub baseline_version: String,
    pub baseline_sha256: String,
    pub organization_id: String,
    pub sequence: u64,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub key_id: String,
    pub observations: Vec<EvidenceObservation>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceTargetType {
    Control,
    Vendor,
    Exercise,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceObservation {
    pub target_type: EvidenceTargetType,
    pub target_id: String,
    pub owner_team: String,
    pub observed_at: DateTime<Utc>,
    pub source_system: String,
    pub immutable_ref: String,
    pub artifact_sha256: String,
    pub result: String,
    #[serde(default)]
    pub claims: BTreeMap<String, String>,
    #[serde(default)]
    pub findings: Vec<String>,
    #[serde(default)]
    pub remediation: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct VerifiedEvidenceSummary {
    pub organization_id: String,
    pub sequence: u64,
    pub key_id: String,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub satisfied_controls: Vec<String>,
    pub satisfied_vendors: Vec<String>,
    pub satisfied_exercises: Vec<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ComplianceError {
    #[error("baseline JSON is invalid: {0}")]
    Json(String),
    #[error("baseline invariant failed: {0}")]
    Invariant(String),
    #[error("external evidence is invalid: {0}")]
    Evidence(String),
}

pub fn load_baseline() -> Result<ComplianceBaseline, ComplianceError> {
    serde_json::from_str(BASELINE).map_err(|error| ComplianceError::Json(error.to_string()))
}

pub fn validate(baseline: &ComplianceBaseline) -> Result<EvidenceReport, ComplianceError> {
    validate_at(baseline, None, &BTreeMap::new(), Utc::now())
}

pub fn validate_with_external_evidence(
    baseline: &ComplianceBaseline,
    evidence: &SignedEvidenceBundle,
    trusted_keys: &BTreeMap<String, VerifyingKey>,
    expected_organization_id: &str,
    last_accepted_sequence: u64,
    now: DateTime<Utc>,
) -> Result<EvidenceReport, ComplianceError> {
    validate_at(
        baseline,
        Some((evidence, expected_organization_id, last_accepted_sequence)),
        trusted_keys,
        now,
    )
}

fn validate_at(
    baseline: &ComplianceBaseline,
    evidence: Option<(&SignedEvidenceBundle, &str, u64)>,
    trusted_keys: &BTreeMap<String, VerifyingKey>,
    now: DateTime<Utc>,
) -> Result<EvidenceReport, ComplianceError> {
    const DOMAINS: &[&str] = &[
        "governance",
        "risk_management",
        "identity_access",
        "secure_sdlc",
        "change_management",
        "vulnerability_management",
        "logging_monitoring",
        "data_protection",
        "incident_response",
        "availability",
        "vendor_management",
        "hr_security",
        "customer_assurance",
    ];
    if baseline.schema_version != 1 || baseline.baseline_version.trim().is_empty() {
        return invariant("schema and baseline versions are required");
    }
    if baseline.scope.len() < 5 {
        return invariant(
            "Community scope must cover product, engineering, supply chain, governance and external boundaries",
        );
    }
    unique(
        baseline.controls.iter().map(|control| &control.id),
        "control",
    )?;
    unique(baseline.assets.iter().map(|asset| &asset.id), "asset")?;
    unique(baseline.risks.iter().map(|risk| &risk.id), "risk")?;
    unique(baseline.data_flows.iter().map(|flow| &flow.id), "data flow")?;
    unique(baseline.vendors.iter().map(|vendor| &vendor.id), "vendor")?;
    let domains = baseline
        .controls
        .iter()
        .map(|control| control.domain.as_str())
        .collect::<BTreeSet<_>>();
    for domain in DOMAINS {
        if !domains.contains(domain) {
            return invariant(&format!("missing control domain {domain}"));
        }
    }
    for control in &baseline.controls {
        if control.title.trim().is_empty()
            || !control.accountable_team.starts_with("team_")
            || !matches!(control.status.as_str(), "implemented" | "required_external")
            || control.frequency.trim().is_empty()
            || control.evidence_source.trim().is_empty()
            || control.evidence_artifact.trim().is_empty()
            || control.automated_test.trim().is_empty()
            || control.exception_workflow != "policy_exception_v1"
            || control.soc2.is_empty()
            || control.iso27001.is_empty()
        {
            return invariant(&format!("control {} is incomplete", control.id));
        }
    }
    for asset in &baseline.assets {
        if !asset.owner_team.starts_with("team_")
            || !matches!(
                asset.classification.as_str(),
                "public" | "internal" | "confidential" | "restricted"
            )
            || asset.system_of_record.trim().is_empty()
            || asset.kind.trim().is_empty()
        {
            return invariant(&format!("asset {} is incomplete", asset.id));
        }
    }
    for risk in &baseline.risks {
        if !risk.owner_team.starts_with("team_")
            || !(1..=5).contains(&risk.likelihood)
            || !(1..=5).contains(&risk.impact)
            || risk.treatment.trim().is_empty()
            || risk.due_date.trim().is_empty()
            || !matches!(risk.status.as_str(), "open" | "accepted" | "mitigated")
        {
            return invariant(&format!("risk {} is incomplete", risk.id));
        }
    }
    for flow in &baseline.data_flows {
        if flow.source.trim().is_empty()
            || flow.destination.trim().is_empty()
            || flow.data_classes.is_empty()
            || flow.purpose.trim().is_empty()
            || flow.retention_policy.trim().is_empty()
            || flow.approval_control.trim().is_empty()
            || (flow.cross_boundary && flow.subprocessor.is_none())
        {
            return invariant(&format!("data flow {} is incomplete", flow.id));
        }
    }
    for vendor in &baseline.vendors {
        if !vendor.owner_team.starts_with("team_")
            || vendor.data_classes.is_empty()
            || !matches!(
                vendor.review_status.as_str(),
                "approved" | "required_external"
            )
            || vendor.review_due.trim().is_empty()
        {
            return invariant(&format!("vendor {} is incomplete", vendor.id));
        }
    }
    for exercise in &baseline.exercises {
        if !exercise.owner_team.starts_with("team_")
            || !matches!(exercise.status.as_str(), "passed" | "required_external")
            || exercise.evidence.trim().is_empty()
            || exercise.findings.len() != exercise.remediation.len()
            || (exercise.status == "passed" && exercise.executed_at.is_none())
        {
            return invariant(&format!("exercise {} is incomplete", exercise.id));
        }
    }
    for required in ["model-provider", "local-audit"] {
        if !baseline.data_flows.iter().any(|flow| flow.id == required) {
            return invariant(&format!("missing required data flow {required}"));
        }
    }
    let verified = evidence
        .map(|(bundle, organization_id, last_sequence)| {
            verify_external_evidence(
                baseline,
                bundle,
                trusted_keys,
                organization_id,
                last_sequence,
                now,
            )
        })
        .transpose()?;
    let satisfied_controls: BTreeSet<String> = verified
        .as_ref()
        .map(|summary| summary.satisfied_controls.iter().cloned().collect())
        .unwrap_or_default();
    let satisfied_vendors: BTreeSet<String> = verified
        .as_ref()
        .map(|summary| summary.satisfied_vendors.iter().cloned().collect())
        .unwrap_or_default();
    let satisfied_exercises: BTreeSet<String> = verified
        .as_ref()
        .map(|summary| summary.satisfied_exercises.iter().cloned().collect())
        .unwrap_or_default();
    let required_external_controls = baseline
        .controls
        .iter()
        .filter(|control| {
            control.status == "required_external" && !satisfied_controls.contains(&control.id)
        })
        .map(|control| control.id.clone())
        .collect::<Vec<_>>();
    let required_external_vendors = baseline
        .vendors
        .iter()
        .filter(|vendor| {
            vendor.review_status == "required_external" && !satisfied_vendors.contains(&vendor.id)
        })
        .map(|vendor| vendor.id.clone())
        .collect::<Vec<_>>();
    let pending_exercises = baseline
        .exercises
        .iter()
        .filter(|exercise| {
            exercise.status == "required_external" && !satisfied_exercises.contains(&exercise.id)
        })
        .map(|exercise| exercise.id.clone())
        .collect::<Vec<_>>();
    let mut counts = BTreeMap::new();
    counts.insert("controls".into(), baseline.controls.len());
    counts.insert("assets".into(), baseline.assets.len());
    counts.insert("risks".into(), baseline.risks.len());
    counts.insert("data_flows".into(), baseline.data_flows.len());
    counts.insert("vendors".into(), baseline.vendors.len());
    counts.insert("exercises".into(), baseline.exercises.len());
    Ok(EvidenceReport {
        schema_version: baseline.schema_version,
        baseline_version: baseline.baseline_version.clone(),
        baseline_sha256: hex_sha256(BASELINE.as_bytes()),
        counts,
        domains: domains.into_iter().map(str::to_owned).collect(),
        certification_ready: required_external_controls.is_empty()
            && required_external_vendors.is_empty()
            && pending_exercises.is_empty(),
        external_evidence: verified,
        required_external_controls,
        required_external_vendors,
        pending_exercises,
    })
}

fn verify_external_evidence(
    baseline: &ComplianceBaseline,
    bundle: &SignedEvidenceBundle,
    trusted_keys: &BTreeMap<String, VerifyingKey>,
    expected_organization_id: &str,
    last_accepted_sequence: u64,
    now: DateTime<Utc>,
) -> Result<VerifiedEvidenceSummary, ComplianceError> {
    let payload = &bundle.payload;
    let expected_hash = hex_sha256(BASELINE.as_bytes());
    if payload.schema_version != 1
        || payload.baseline_version != baseline.baseline_version
        || payload.baseline_sha256 != expected_hash
    {
        return evidence_error("bundle is not bound to this baseline");
    }
    if payload.observations.len() > 256 {
        return evidence_error("bundle has too many observations");
    }
    if !valid_identifier(expected_organization_id, "org_")
        || payload.organization_id != expected_organization_id
        || !valid_identifier(&payload.key_id, "key_")
        || payload.sequence <= last_accepted_sequence
    {
        return evidence_error("organization binding, key or monotonic sequence is invalid");
    }
    if payload.issued_at > now + chrono::Duration::minutes(5)
        || payload.expires_at <= now
        || payload.expires_at > payload.issued_at + chrono::Duration::days(366)
    {
        return evidence_error("bundle is not currently valid or exceeds one year");
    }
    let key = trusted_keys
        .get(&payload.key_id)
        .ok_or_else(|| ComplianceError::Evidence("untrusted key ID".into()))?;
    let encoded = serde_json::to_vec(payload)
        .map_err(|error| ComplianceError::Evidence(error.to_string()))?;
    let signature_bytes = STANDARD
        .decode(&bundle.signature)
        .map_err(|_| ComplianceError::Evidence("malformed signature".into()))?;
    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|_| ComplianceError::Evidence("malformed signature".into()))?;
    key.verify(&encoded, &signature)
        .map_err(|_| ComplianceError::Evidence("signature verification failed".into()))?;

    let controls = baseline
        .controls
        .iter()
        .map(|value| (value.id.as_str(), value.accountable_team.as_str()))
        .collect::<BTreeMap<_, _>>();
    let vendors = baseline
        .vendors
        .iter()
        .map(|value| (value.id.as_str(), value.owner_team.as_str()))
        .collect::<BTreeMap<_, _>>();
    let exercises = baseline
        .exercises
        .iter()
        .map(|value| (value.id.as_str(), value.owner_team.as_str()))
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    let mut satisfied_controls = BTreeSet::new();
    let mut satisfied_vendors = BTreeSet::new();
    let mut satisfied_exercises = BTreeSet::new();
    for observation in &payload.observations {
        if !seen.insert((
            observation.target_type.clone(),
            observation.target_id.clone(),
        )) {
            return evidence_error("duplicate evidence target");
        }
        if observation.result != "passed"
            || !valid_label(&observation.source_system)
            || !valid_evidence_ref(&observation.immutable_ref)
            || !valid_sha256(&observation.artifact_sha256)
            || observation.observed_at > payload.issued_at + chrono::Duration::minutes(5)
            || observation.observed_at < payload.issued_at - chrono::Duration::days(366)
            || observation.findings.len() != observation.remediation.len()
            || observation.findings.len() > 64
            || observation.claims.len() > 32
            || observation
                .claims
                .iter()
                .any(|(key, value)| !valid_label(key) || !valid_claim(value))
            || observation
                .findings
                .iter()
                .chain(&observation.remediation)
                .any(|value| !valid_label(value))
        {
            return evidence_error("observation fields, time or remediation are invalid");
        }
        let expected_owner = match observation.target_type {
            EvidenceTargetType::Control => controls.get(observation.target_id.as_str()),
            EvidenceTargetType::Vendor => vendors.get(observation.target_id.as_str()),
            EvidenceTargetType::Exercise => exercises.get(observation.target_id.as_str()),
        }
        .ok_or_else(|| ComplianceError::Evidence("unknown evidence target".into()))?;
        if observation.owner_team != *expected_owner {
            return evidence_error("evidence owner does not match the baseline");
        }
        if observation.target_type == EvidenceTargetType::Exercise
            && observation.target_id == "exercise-incident-tabletop"
        {
            for claim in [
                "severity_model_tested",
                "attendee_count",
                "timeline_ref",
                "notification_decision",
                "postmortem_ref",
            ] {
                if observation
                    .claims
                    .get(claim)
                    .is_none_or(|value| value.trim().is_empty())
                {
                    return evidence_error("incident tabletop is missing a required claim");
                }
            }
            let attendee_count = observation.claims["attendee_count"]
                .parse::<u16>()
                .map_err(|_| ComplianceError::Evidence("invalid tabletop attendee count".into()))?;
            if attendee_count < 2 || observation.findings.is_empty() {
                return evidence_error("tabletop needs separation of duties and remediation");
            }
        }
        match observation.target_type {
            EvidenceTargetType::Control => {
                satisfied_controls.insert(observation.target_id.clone());
            }
            EvidenceTargetType::Vendor => {
                satisfied_vendors.insert(observation.target_id.clone());
            }
            EvidenceTargetType::Exercise => {
                satisfied_exercises.insert(observation.target_id.clone());
            }
        }
    }
    Ok(VerifiedEvidenceSummary {
        organization_id: payload.organization_id.clone(),
        sequence: payload.sequence,
        key_id: payload.key_id.clone(),
        issued_at: payload.issued_at,
        expires_at: payload.expires_at,
        satisfied_controls: satisfied_controls.into_iter().collect(),
        satisfied_vendors: satisfied_vendors.into_iter().collect(),
        satisfied_exercises: satisfied_exercises.into_iter().collect(),
    })
}

fn valid_identifier(value: &str, prefix: &str) -> bool {
    value.len() <= 128
        && value.starts_with(prefix)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn valid_evidence_ref(value: &str) -> bool {
    value.len() <= 2048
        && !value.contains(['?', '#'])
        && !value.chars().any(char::is_whitespace)
        && (value.starts_with("https://") || value.starts_with("urn:"))
}

fn valid_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
}

fn valid_claim(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn evidence_error<T>(message: &str) -> Result<T, ComplianceError> {
    Err(ComplianceError::Evidence(message.into()))
}

fn unique<'a>(values: impl Iterator<Item = &'a String>, kind: &str) -> Result<(), ComplianceError> {
    let mut seen = BTreeSet::new();
    for value in values {
        if value.trim().is_empty() || !seen.insert(value) {
            return invariant(&format!("{kind} IDs must be non-empty and unique"));
        }
    }
    Ok(())
}

fn invariant<T>(message: &str) -> Result<T, ComplianceError> {
    Err(ComplianceError::Invariant(message.into()))
}

fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn observation(
        target_type: EvidenceTargetType,
        target_id: &str,
        owner_team: &str,
        now: DateTime<Utc>,
    ) -> EvidenceObservation {
        EvidenceObservation {
            target_type,
            target_id: target_id.into(),
            owner_team: owner_team.into(),
            observed_at: now,
            source_system: "fixture-system".into(),
            immutable_ref: format!("urn:opencoding:test:{target_id}"),
            artifact_sha256: "a".repeat(64),
            result: "passed".into(),
            claims: BTreeMap::new(),
            findings: vec![],
            remediation: vec![],
        }
    }

    fn signed_bundle(
        baseline: &ComplianceBaseline,
        observations: Vec<EvidenceObservation>,
        now: DateTime<Utc>,
    ) -> (SignedEvidenceBundle, BTreeMap<String, VerifyingKey>) {
        let signing_key = SigningKey::from_bytes(&[7; 32]);
        let payload = ExternalEvidencePayload {
            schema_version: 1,
            baseline_version: baseline.baseline_version.clone(),
            baseline_sha256: hex_sha256(BASELINE.as_bytes()),
            organization_id: "org_fixture".into(),
            sequence: 1,
            issued_at: now,
            expires_at: now + chrono::Duration::days(30),
            key_id: "key_fixture".into(),
            observations,
        };
        let signature = signing_key.sign(&serde_json::to_vec(&payload).unwrap());
        let bundle = SignedEvidenceBundle {
            payload,
            signature: STANDARD.encode(signature.to_bytes()),
        };
        let keys = BTreeMap::from([("key_fixture".into(), signing_key.verifying_key())]);
        (bundle, keys)
    }

    #[test]
    fn checked_in_baseline_is_complete_and_reports_external_gaps() {
        let baseline = load_baseline().unwrap();
        let report = validate(&baseline).unwrap();
        assert_eq!(report.domains.len(), 13);
        assert!(!report.certification_ready);
        assert!(report.required_external_controls.contains(&"IAM-01".into()));
        assert_eq!(report.required_external_vendors.len(), 2);
        assert!(report.external_evidence.is_none());
    }

    #[test]
    fn missing_evidence_and_personal_ownership_fail_validation() {
        let mut baseline = load_baseline().unwrap();
        baseline.controls[0].evidence_artifact.clear();
        assert!(matches!(
            validate(&baseline),
            Err(ComplianceError::Invariant(_))
        ));
        let mut baseline = load_baseline().unwrap();
        baseline.assets[0].owner_team = "alice".into();
        assert!(matches!(
            validate(&baseline),
            Err(ComplianceError::Invariant(_))
        ));
    }

    #[test]
    fn signed_external_evidence_satisfies_only_its_exact_target() {
        let baseline = load_baseline().unwrap();
        let now = "2026-07-21T12:00:00Z".parse().unwrap();
        let item = observation(
            EvidenceTargetType::Control,
            "IAM-01",
            "team_maintainers",
            now,
        );
        let (bundle, keys) = signed_bundle(&baseline, vec![item], now);
        let report =
            validate_with_external_evidence(&baseline, &bundle, &keys, "org_fixture", 0, now)
                .unwrap();
        assert!(!report.required_external_controls.contains(&"IAM-01".into()));
        assert!(
            report
                .required_external_controls
                .contains(&"SDLC-01".into())
        );
        assert_eq!(
            report.external_evidence.unwrap().satisfied_controls,
            vec!["IAM-01"]
        );
        assert!(!report.certification_ready);
        assert!(matches!(
            validate_with_external_evidence(
                &baseline,
                &bundle,
                &keys,
                "org_other",
                0,
                now
            ),
            Err(ComplianceError::Evidence(message)) if message.contains("organization binding")
        ));
        assert!(matches!(
            validate_with_external_evidence(
                &baseline,
                &bundle,
                &keys,
                "org_fixture",
                1,
                now
            ),
            Err(ComplianceError::Evidence(message)) if message.contains("monotonic sequence")
        ));
    }

    #[test]
    fn external_evidence_rejects_tampering_expiry_and_wrong_owner() {
        let baseline = load_baseline().unwrap();
        let now = "2026-07-21T12:00:00Z".parse().unwrap();
        let item = observation(
            EvidenceTargetType::Control,
            "IAM-01",
            "team_maintainers",
            now,
        );
        let (mut bundle, keys) = signed_bundle(&baseline, vec![item], now);
        bundle.payload.observations[0].result = "failed".into();
        assert!(matches!(
            validate_with_external_evidence(
                &baseline,
                &bundle,
                &keys,
                "org_fixture",
                0,
                now
            ),
            Err(ComplianceError::Evidence(message)) if message == "signature verification failed"
        ));

        let item = observation(EvidenceTargetType::Control, "IAM-01", "team_identity", now);
        let (bundle, keys) = signed_bundle(&baseline, vec![item], now);
        assert!(matches!(
            validate_with_external_evidence(
                &baseline,
                &bundle,
                &keys,
                "org_fixture",
                0,
                now
            ),
            Err(ComplianceError::Evidence(message)) if message.contains("owner")
        ));

        let item = observation(
            EvidenceTargetType::Control,
            "IAM-01",
            "team_maintainers",
            now,
        );
        let (bundle, keys) = signed_bundle(&baseline, vec![item], now);
        assert!(matches!(
            validate_with_external_evidence(
                &baseline,
                &bundle,
                &keys,
                "org_fixture",
                0,
                now + chrono::Duration::days(31)
            ),
            Err(ComplianceError::Evidence(message)) if message.contains("currently valid")
        ));
    }

    #[test]
    fn incident_tabletop_requires_people_timeline_notification_and_remediation() {
        let baseline = load_baseline().unwrap();
        let now = "2026-07-21T12:00:00Z".parse().unwrap();
        let item = observation(
            EvidenceTargetType::Exercise,
            "exercise-incident-tabletop",
            "team_security",
            now,
        );
        let (bundle, keys) = signed_bundle(&baseline, vec![item], now);
        assert!(matches!(
            validate_with_external_evidence(
                &baseline,
                &bundle,
                &keys,
                "org_fixture",
                0,
                now
            ),
            Err(ComplianceError::Evidence(message)) if message.contains("required claim")
        ));

        let mut item = observation(
            EvidenceTargetType::Exercise,
            "exercise-incident-tabletop",
            "team_security",
            now,
        );
        item.claims = BTreeMap::from([
            ("severity_model_tested".into(), "SEV-1".into()),
            ("attendee_count".into(), "4".into()),
            ("timeline_ref".into(), "urn:incident:timeline:123".into()),
            ("notification_decision".into(), "notify".into()),
            (
                "postmortem_ref".into(),
                "urn:incident:postmortem:123".into(),
            ),
        ]);
        item.findings = vec!["finding-123".into()];
        item.remediation = vec!["task-456".into()];
        let (bundle, keys) = signed_bundle(&baseline, vec![item], now);
        let report =
            validate_with_external_evidence(&baseline, &bundle, &keys, "org_fixture", 0, now)
                .unwrap();
        assert_eq!(
            report.pending_exercises,
            vec!["exercise-local-backup-restore"]
        );
        assert_eq!(
            report.external_evidence.unwrap().satisfied_exercises,
            vec!["exercise-incident-tabletop"]
        );
    }

    #[test]
    fn readiness_requires_every_external_control_vendor_and_exercise() {
        let baseline = load_baseline().unwrap();
        let now = "2026-07-21T12:00:00Z".parse().unwrap();
        let mut items = baseline
            .controls
            .iter()
            .filter(|control| control.status == "required_external")
            .map(|control| {
                observation(
                    EvidenceTargetType::Control,
                    &control.id,
                    &control.accountable_team,
                    now,
                )
            })
            .collect::<Vec<_>>();
        items.extend(
            baseline
                .vendors
                .iter()
                .filter(|vendor| vendor.review_status == "required_external")
                .map(|vendor| {
                    observation(
                        EvidenceTargetType::Vendor,
                        &vendor.id,
                        &vendor.owner_team,
                        now,
                    )
                }),
        );
        for exercise in baseline
            .exercises
            .iter()
            .filter(|exercise| exercise.status == "required_external")
        {
            let mut item = observation(
                EvidenceTargetType::Exercise,
                &exercise.id,
                &exercise.owner_team,
                now,
            );
            if exercise.id == "exercise-incident-tabletop" {
                item.claims = BTreeMap::from([
                    ("severity_model_tested".into(), "SEV-1".into()),
                    ("attendee_count".into(), "4".into()),
                    ("timeline_ref".into(), "urn:incident:timeline:123".into()),
                    ("notification_decision".into(), "notify".into()),
                    (
                        "postmortem_ref".into(),
                        "urn:incident:postmortem:123".into(),
                    ),
                ]);
                item.findings = vec!["finding-123".into()];
                item.remediation = vec!["task-456".into()];
            }
            items.push(item);
        }

        let (bundle, keys) = signed_bundle(&baseline, items, now);
        let report =
            validate_with_external_evidence(&baseline, &bundle, &keys, "org_fixture", 0, now)
                .unwrap();
        assert!(report.required_external_controls.is_empty());
        assert!(report.required_external_vendors.is_empty());
        assert!(report.pending_exercises.is_empty());
        assert!(report.certification_ready);
    }
}
