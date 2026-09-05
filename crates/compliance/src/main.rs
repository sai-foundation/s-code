use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::VerifyingKey;
use s_code_compliance::SignedEvidenceBundle;
use std::collections::BTreeMap;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut evidence_path = None;
    let mut organization_id = None;
    let mut last_sequence = None;
    let mut trusted_keys = BTreeMap::new();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--external-evidence" => {
                evidence_path = Some(args.next().ok_or("--external-evidence requires a path")?);
            }
            "--trust-key" => {
                let value = args.next().ok_or("--trust-key requires KEY_ID=BASE64")?;
                let (key_id, encoded) = value
                    .split_once('=')
                    .ok_or("--trust-key requires KEY_ID=BASE64")?;
                let bytes = STANDARD.decode(encoded)?;
                let bytes: [u8; 32] = bytes
                    .try_into()
                    .map_err(|_| "Ed25519 public key must be 32 bytes")?;
                trusted_keys.insert(key_id.to_owned(), VerifyingKey::from_bytes(&bytes)?);
            }
            "--organization" => {
                organization_id = Some(args.next().ok_or("--organization requires an ID")?);
            }
            "--after-sequence" => {
                last_sequence = Some(
                    args.next()
                        .ok_or("--after-sequence requires an integer")?
                        .parse::<u64>()?,
                );
            }
            "--help" | "-h" => {
                println!(
                    "s-code-compliance [--external-evidence PATH --organization ORG_ID --after-sequence N --trust-key KEY_ID=BASE64 ...]"
                );
                return Ok(());
            }
            _ => return Err(format!("unknown argument: {argument}").into()),
        }
    }
    let baseline = s_code_compliance::load_baseline()?;
    let report = if let Some(path) = evidence_path {
        if trusted_keys.is_empty() {
            return Err("external evidence requires at least one trusted public key".into());
        }
        let organization_id = organization_id
            .as_deref()
            .ok_or("external evidence requires --organization")?;
        let last_sequence = last_sequence.ok_or("external evidence requires --after-sequence")?;
        let encoded = std::fs::read(path)?;
        let evidence: SignedEvidenceBundle = serde_json::from_slice(&encoded)?;
        s_code_compliance::validate_with_external_evidence(
            &baseline,
            &evidence,
            &trusted_keys,
            organization_id,
            last_sequence,
            chrono::Utc::now(),
        )?
    } else {
        if !trusted_keys.is_empty() || organization_id.is_some() || last_sequence.is_some() {
            return Err("evidence verification arguments require --external-evidence".into());
        }
        s_code_compliance::validate(&baseline)?
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
