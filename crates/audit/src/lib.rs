use aes_gcm::{
    Aes256Gcm, KeyInit,
    aead::{Aead, Payload},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use opencoding_protocol::Event;
use opencoding_protocol::{Id, Scope};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

pub const CENTRAL_AUDIT_SCHEMA_VERSION: u32 = 1;
pub const MAX_CENTRAL_AUDIT_RECORDS: usize = 1_000;
pub const MAX_CENTRAL_AUDIT_CONTENT_RECORD_BYTES: usize = 1024 * 1024;
pub const MAX_CENTRAL_AUDIT_CONTENT_BATCH_BYTES: usize = 8 * 1024 * 1024;
const CENTRAL_AUDIT_CONTENT_SCHEMA_VERSION: u32 = 1;
const AES_256_GCM: &str = "AES-256-GCM";

/// A content-free projection of one locally persisted daemon audit event. The
/// source chain head and payload digest preserve investigation evidence while
/// keeping source code, prompts, tool arguments and command output out of the
/// central control plane by default.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CentralAuditRecord {
    pub source_id: Id,
    pub source_sequence: u64,
    pub local_sequence: u64,
    pub occurred_at: DateTime<Utc>,
    pub scope: Scope,
    pub session_id: Option<Id>,
    pub turn_id: Option<Id>,
    pub kind: String,
    pub payload_sha256: String,
    pub source_chain_hash: String,
}

impl CentralAuditRecord {
    pub fn metadata_only(
        event: &Event,
        source_id: Id,
        source_sequence: u64,
        source_chain_hash: impl Into<String>,
    ) -> anyhow::Result<Self> {
        let payload = serde_json::to_vec(&event.payload)?;
        Ok(Self {
            source_id,
            source_sequence,
            local_sequence: event.sequence,
            occurred_at: event.timestamp,
            scope: event.scope.clone(),
            session_id: event.session_id.clone(),
            turn_id: event.turn_id.clone(),
            kind: event.kind.clone(),
            payload_sha256: hex(&Sha256::digest(payload)),
            source_chain_hash: source_chain_hash.into(),
        })
    }

    fn validate(&self, source_id: &Id, organization_id: &Id, team_id: &Id) -> anyhow::Result<()> {
        if self.source_sequence == 0
            || self.local_sequence == 0
            || self.source_id != *source_id
            || self.scope.organization_id != *organization_id
            || self.scope.team_id != *team_id
            || self.kind.is_empty()
            || self.kind.len() > 128
            || !valid_hash(&self.payload_sha256)
            || !valid_hash(&self.source_chain_hash)
        {
            anyhow::bail!("central audit record is invalid or outside its registered scope");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CentralAuditBatchPayload {
    pub schema_version: u32,
    pub batch_id: Id,
    pub source_id: Id,
    pub organization_id: Id,
    pub team_id: Id,
    pub previous_sequence: u64,
    pub previous_chain_hash: String,
    pub generated_at: DateTime<Utc>,
    pub records: Vec<CentralAuditRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted_content: Option<CentralAuditEncryptedContent>,
}

impl CentralAuditBatchPayload {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.schema_version != CENTRAL_AUDIT_SCHEMA_VERSION
            || self.batch_id.0.is_empty()
            || self.batch_id.0.len() > 128
            || self.source_id.0.is_empty()
            || self.source_id.0.len() > 128
            || self.records.is_empty()
            || self.records.len() > MAX_CENTRAL_AUDIT_RECORDS
            || !valid_hash(&self.previous_chain_hash)
            || (self.previous_sequence == 0 && self.previous_chain_hash != "0".repeat(64))
        {
            anyhow::bail!("central audit batch shape is invalid");
        }
        let mut expected = self
            .previous_sequence
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("central audit sequence overflow"))?;
        let mut previous_local_sequence = 0;
        for record in &self.records {
            record.validate(&self.source_id, &self.organization_id, &self.team_id)?;
            if record.source_sequence != expected
                || record.local_sequence <= previous_local_sequence
            {
                anyhow::bail!("central audit batch sequence is not contiguous");
            }
            previous_local_sequence = record.local_sequence;
            expected = expected
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("central audit sequence overflow"))?;
        }
        if let Some(content) = &self.encrypted_content {
            content.validate(self)?;
        }
        Ok(())
    }

    pub fn last_sequence(&self) -> u64 {
        self.records
            .last()
            .map_or(self.previous_sequence, |record| record.source_sequence)
    }

    pub fn chain_head(&self) -> &str {
        self.records
            .last()
            .map_or(self.previous_chain_hash.as_str(), |record| {
                record.source_chain_hash.as_str()
            })
    }

    pub fn last_local_sequence(&self) -> u64 {
        self.records
            .last()
            .map_or(0, |record| record.local_sequence)
    }

    pub fn sha256(&self) -> anyhow::Result<String> {
        Ok(hex(&Sha256::digest(serde_json::to_vec(self)?)))
    }
}

/// A KMS-wrapped data key. The daemon receives the plaintext data key only
/// from a scoped provider and never serializes it; the control plane stores
/// only this wrapped form until an authorized replay asks KMS to unwrap it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CentralAuditWrappedDataKey {
    pub key_id: String,
    pub algorithm: String,
    pub nonce: String,
    pub ciphertext: String,
}

impl CentralAuditWrappedDataKey {
    fn validate(&self) -> anyhow::Result<()> {
        let nonce = URL_SAFE_NO_PAD.decode(&self.nonce)?;
        let ciphertext = URL_SAFE_NO_PAD.decode(&self.ciphertext)?;
        if self.key_id.is_empty()
            || self.key_id.len() > 256
            || self.algorithm != AES_256_GCM
            || nonce.len() != 12
            || ciphertext.len() < 16
            || ciphertext.len() > 4096
        {
            anyhow::bail!("central audit wrapped data key is invalid");
        }
        Ok(())
    }
}

/// The plaintext half is deliberately private and lacks Serialize/Debug.
/// Providers should construct this from a KMS GenerateDataKey equivalent.
pub struct CentralAuditDataKeyMaterial {
    plaintext: [u8; 32],
    wrapped: CentralAuditWrappedDataKey,
}

impl Drop for CentralAuditDataKeyMaterial {
    fn drop(&mut self) {
        self.plaintext.zeroize();
    }
}

impl CentralAuditDataKeyMaterial {
    pub fn new(plaintext: &[u8], wrapped: CentralAuditWrappedDataKey) -> anyhow::Result<Self> {
        wrapped.validate()?;
        let plaintext: [u8; 32] = plaintext
            .try_into()
            .map_err(|_| anyhow::anyhow!("central audit data key must be exactly 32 bytes"))?;
        Ok(Self { plaintext, wrapped })
    }

    pub fn key_id(&self) -> &str {
        &self.wrapped.key_id
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CentralAuditEncryptedRecord {
    pub source_sequence: u64,
    pub local_sequence: u64,
    pub nonce: String,
    pub ciphertext: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CentralAuditEncryptedContent {
    pub schema_version: u32,
    pub wrapped_data_key: CentralAuditWrappedDataKey,
    pub records: Vec<CentralAuditEncryptedRecord>,
}

impl CentralAuditEncryptedContent {
    fn validate(&self, batch: &CentralAuditBatchPayload) -> anyhow::Result<()> {
        if self.schema_version != CENTRAL_AUDIT_CONTENT_SCHEMA_VERSION
            || self.records.len() != batch.records.len()
        {
            anyhow::bail!("central audit encrypted content does not cover the exact batch");
        }
        self.wrapped_data_key.validate()?;
        let mut total = 0_usize;
        for (encrypted, metadata) in self.records.iter().zip(&batch.records) {
            let nonce = URL_SAFE_NO_PAD.decode(&encrypted.nonce)?;
            let ciphertext = URL_SAFE_NO_PAD.decode(&encrypted.ciphertext)?;
            total = total
                .checked_add(ciphertext.len())
                .ok_or_else(|| anyhow::anyhow!("central audit encrypted content size overflow"))?;
            if encrypted.source_sequence != metadata.source_sequence
                || encrypted.local_sequence != metadata.local_sequence
                || nonce.len() != 12
                || ciphertext.len() < 16
                || ciphertext.len() > MAX_CENTRAL_AUDIT_CONTENT_RECORD_BYTES
                || total > MAX_CENTRAL_AUDIT_CONTENT_BATCH_BYTES
            {
                anyhow::bail!("central audit encrypted record is invalid or misbound");
            }
        }
        Ok(())
    }
}

fn central_audit_content_aad(
    batch: &CentralAuditBatchPayload,
    metadata: &CentralAuditRecord,
) -> anyhow::Result<Vec<u8>> {
    Ok(serde_json::to_vec(&(
        "opencoding.central-audit.content.v1",
        &batch.batch_id,
        &batch.source_id,
        &batch.organization_id,
        &batch.team_id,
        metadata.source_sequence,
        metadata.local_sequence,
        &metadata.payload_sha256,
        &metadata.source_chain_hash,
    ))?)
}

/// Encrypts the exact event payloads represented by `batch.records`. This is
/// opt-in: callers that do not have an explicit content policy leave
/// `encrypted_content` as `None`, preserving the default metadata-only path.
pub fn encrypt_central_audit_content(
    batch: &CentralAuditBatchPayload,
    events: &[Event],
    key: CentralAuditDataKeyMaterial,
) -> anyhow::Result<CentralAuditEncryptedContent> {
    if batch.encrypted_content.is_some() || events.len() != batch.records.len() {
        anyhow::bail!("central audit content must cover an unencrypted exact batch");
    }
    let cipher = Aes256Gcm::new_from_slice(&key.plaintext)
        .map_err(|_| anyhow::anyhow!("central audit data key is invalid"))?;
    let mut encrypted = Vec::with_capacity(events.len());
    for (event, metadata) in events.iter().zip(&batch.records) {
        if event.sequence != metadata.local_sequence
            || event.scope != metadata.scope
            || hex(&Sha256::digest(serde_json::to_vec(&event.payload)?)) != metadata.payload_sha256
        {
            anyhow::bail!("central audit plaintext event does not match its metadata projection");
        }
        if redact(event.payload.clone()) != event.payload {
            anyhow::bail!("central audit content contains a field that must be redacted locally");
        }
        let plaintext = serde_json::to_vec(&event.payload)?;
        if plaintext.len() + 16 > MAX_CENTRAL_AUDIT_CONTENT_RECORD_BYTES {
            anyhow::bail!("central audit content record exceeds the encrypted size limit");
        }
        let mut nonce = [0_u8; 12];
        getrandom::fill(&mut nonce)
            .map_err(|_| anyhow::anyhow!("central audit content nonce generation failed"))?;
        let ciphertext = cipher
            .encrypt(
                (&nonce).into(),
                Payload {
                    msg: &plaintext,
                    aad: &central_audit_content_aad(batch, metadata)?,
                },
            )
            .map_err(|_| anyhow::anyhow!("central audit content encryption failed"))?;
        encrypted.push(CentralAuditEncryptedRecord {
            source_sequence: metadata.source_sequence,
            local_sequence: metadata.local_sequence,
            nonce: URL_SAFE_NO_PAD.encode(nonce),
            ciphertext: URL_SAFE_NO_PAD.encode(ciphertext),
        });
    }
    let content = CentralAuditEncryptedContent {
        schema_version: CENTRAL_AUDIT_CONTENT_SCHEMA_VERSION,
        wrapped_data_key: key.wrapped.clone(),
        records: encrypted,
    };
    content.validate(batch)?;
    Ok(content)
}

/// Decrypts only after a caller has independently authorized replay and asked
/// the configured KMS provider to unwrap the included data key.
pub fn decrypt_central_audit_content(
    batch: &CentralAuditBatchPayload,
    plaintext_data_key: &[u8],
) -> anyhow::Result<Vec<serde_json::Value>> {
    batch.validate()?;
    let content = batch
        .encrypted_content
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("central audit batch is metadata-only"))?;
    let cipher = Aes256Gcm::new_from_slice(plaintext_data_key)
        .map_err(|_| anyhow::anyhow!("central audit data key must be exactly 32 bytes"))?;
    content
        .records
        .iter()
        .zip(&batch.records)
        .map(|(encrypted, metadata)| {
            let nonce = URL_SAFE_NO_PAD.decode(&encrypted.nonce)?;
            let ciphertext = URL_SAFE_NO_PAD.decode(&encrypted.ciphertext)?;
            let plaintext = cipher
                .decrypt(
                    nonce.as_slice().into(),
                    Payload {
                        msg: &ciphertext,
                        aad: &central_audit_content_aad(batch, metadata)?,
                    },
                )
                .map_err(|_| anyhow::anyhow!("central audit content authentication failed"))?;
            let value: serde_json::Value = serde_json::from_slice(&plaintext)?;
            if hex(&Sha256::digest(serde_json::to_vec(&value)?)) != metadata.payload_sha256 {
                anyhow::bail!("central audit decrypted content digest does not match metadata");
            }
            Ok(value)
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignedCentralAuditBatch {
    pub key_id: String,
    pub payload: CentralAuditBatchPayload,
    pub signature: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CentralAuditIngestReceipt {
    pub batch_id: Id,
    pub source_id: Id,
    pub accepted: bool,
    pub record_count: usize,
    pub last_sequence: u64,
    pub last_local_sequence: u64,
    pub chain_head: String,
    pub payload_sha256: String,
}

impl CentralAuditIngestReceipt {
    pub fn validate_for(&self, envelope: &SignedCentralAuditBatch) -> anyhow::Result<()> {
        let payload = &envelope.payload;
        if self.batch_id != payload.batch_id
            || self.source_id != payload.source_id
            || self.record_count != payload.records.len()
            || self.last_sequence != payload.last_sequence()
            || self.last_local_sequence != payload.last_local_sequence()
            || self.chain_head != payload.chain_head()
            || self.payload_sha256 != payload.sha256()?
        {
            anyhow::bail!("central audit ingestion receipt does not match the signed batch");
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct CentralAuditSigner {
    key_id: String,
    key: SigningKey,
}

impl CentralAuditSigner {
    pub fn from_base64(
        key_id: impl Into<String>,
        private_key_base64: &str,
    ) -> anyhow::Result<Self> {
        let bytes = URL_SAFE_NO_PAD.decode(private_key_base64)?;
        let key = SigningKey::from_bytes(
            bytes
                .as_slice()
                .try_into()
                .map_err(|_| anyhow::anyhow!("central audit signing key must be 32 bytes"))?,
        );
        let key_id = key_id.into();
        if key_id.is_empty() || key_id.len() > 128 {
            anyhow::bail!("central audit key id must be 1..128 characters");
        }
        Ok(Self { key_id, key })
    }

    pub fn public_key_base64(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.key.verifying_key().to_bytes())
    }

    pub fn sign(
        &self,
        payload: CentralAuditBatchPayload,
    ) -> anyhow::Result<SignedCentralAuditBatch> {
        payload.validate()?;
        let bytes = serde_json::to_vec(&payload)?;
        Ok(SignedCentralAuditBatch {
            key_id: self.key_id.clone(),
            payload,
            signature: URL_SAFE_NO_PAD.encode(self.key.sign(&bytes).to_bytes()),
        })
    }
}

pub fn verify_central_audit_batch(
    envelope: &SignedCentralAuditBatch,
    expected_key_id: &str,
    public_key_base64: &str,
) -> anyhow::Result<()> {
    if envelope.key_id != expected_key_id {
        anyhow::bail!("central audit signing key id is not trusted");
    }
    envelope.payload.validate()?;
    let key_bytes = URL_SAFE_NO_PAD.decode(public_key_base64)?;
    let key = VerifyingKey::from_bytes(
        key_bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("central audit public key must be 32 bytes"))?,
    )?;
    let signature_bytes = URL_SAFE_NO_PAD.decode(&envelope.signature)?;
    let signature = Signature::from_slice(&signature_bytes)?;
    key.verify(&serde_json::to_vec(&envelope.payload)?, &signature)?;
    Ok(())
}

pub fn validate_central_audit_public_key(public_key_base64: &str) -> anyhow::Result<()> {
    let key_bytes = URL_SAFE_NO_PAD.decode(public_key_base64)?;
    VerifyingKey::from_bytes(
        key_bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("central audit public key must be 32 bytes"))?,
    )?;
    Ok(())
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Default)]
pub struct HashChain {
    previous: [u8; 32],
}

impl HashChain {
    pub fn from_head(head: &str) -> anyhow::Result<Self> {
        if head.len() != 64 {
            anyhow::bail!("audit chain head must be 32-byte hex");
        }
        let mut previous = [0_u8; 32];
        for (index, byte) in previous.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&head[index * 2..index * 2 + 2], 16)?;
        }
        Ok(Self { previous })
    }

    pub fn append(&mut self, event: &Event) -> anyhow::Result<String> {
        let mut h = Sha256::new();
        h.update(self.previous);
        h.update(serde_json::to_vec(event)?);
        self.previous = h.finalize().into();
        Ok(hex(&self.previous))
    }
    pub fn head(&self) -> String {
        hex(&self.previous)
    }

    pub fn verify<'a>(
        records: impl IntoIterator<Item = (&'a Event, &'a str)>,
    ) -> anyhow::Result<Self> {
        let mut chain = Self::default();
        let mut expected_sequence = 1_u64;
        for (event, stored_hash) in records {
            if event.sequence != expected_sequence {
                anyhow::bail!("audit sequence gap at {}", event.sequence);
            }
            let computed = chain.append(event)?;
            if computed != stored_hash {
                anyhow::bail!("audit chain mismatch at sequence {}", event.sequence);
            }
            expected_sequence += 1;
        }
        Ok(chain)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sensitive_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    const EXACT_CREDENTIAL_KEYS: &[&str] = &[
        "authorization",
        "bearertoken",
        "credential",
        "credentials",
        "csrftoken",
        "idtoken",
        "password",
        "passphrase",
        "privatekey",
        "proxyauthorization",
        "secret",
        "token",
    ];
    const CREDENTIAL_SUFFIXES: &[&str] = &[
        "accesstoken",
        "apikey",
        "authtoken",
        "bearertoken",
        "clientsecret",
        "credential",
        "credentials",
        "csrftoken",
        "password",
        "passphrase",
        "privatekey",
        "refreshtoken",
        "secret",
        "signingkey",
    ];
    EXACT_CREDENTIAL_KEYS.contains(&normalized.as_str())
        || normalized.starts_with("authorization")
        || CREDENTIAL_SUFFIXES
            .iter()
            .any(|suffix| normalized.ends_with(suffix))
}

fn secret_token_end(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len()
        && (bytes[index].is_ascii_alphanumeric()
            || matches!(bytes[index], b'_' | b'-' | b'.' | b'/' | b'+' | b'='))
    {
        index += 1;
    }
    index
}

/// Removes high-confidence credential shapes from untrusted process output.
/// This deliberately avoids broad entropy guessing, which would corrupt normal
/// source code and hashes, and complements exact-value redaction at boundaries
/// that resolve an environment secret.
pub fn redact_text(value: &str) -> String {
    if value.contains("-----BEGIN") && value.contains("PRIVATE KEY-----") {
        return "[REDACTED PRIVATE KEY]".into();
    }
    let bytes = value.as_bytes();
    let lower = value.to_ascii_lowercase();
    let mut ranges = Vec::<(usize, usize)>::new();

    let mut line_start = 0;
    for line in value.split_inclusive('\n') {
        let content = line.trim_end_matches(['\r', '\n']);
        let trimmed = content.trim_start();
        let prefix = content.len() - trimmed.len();
        let assignment = trimmed.strip_prefix("export ").unwrap_or(trimmed);
        let export_offset = trimmed.len() - assignment.len();
        if let Some(separator) = assignment.find(['=', ':']) {
            let key = assignment[..separator].trim();
            if !key.is_empty()
                && key.len() <= 128
                && key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                && sensitive_key(key)
            {
                let start = line_start + prefix + export_offset + separator + 1;
                let end = line_start + content.len();
                if start < end {
                    ranges.push((start, end));
                }
            }
        }
        line_start += line.len();
    }

    for marker in ["bearer ", "basic "] {
        let mut offset = 0;
        while let Some(found) = lower[offset..].find(marker) {
            let start = offset + found + marker.len();
            let end = secret_token_end(bytes, start);
            if end > start {
                ranges.push((start, end));
            }
            offset = end.max(offset + found + marker.len());
        }
    }

    for marker in [
        "sk-",
        "github_pat_",
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "ghr_",
        "glpat-",
        "hf_",
        "xoxb-",
        "xoxp-",
        "xoxa-",
        "xoxr-",
        "akia",
        "aiza",
    ] {
        let mut offset = 0;
        while let Some(found) = lower[offset..].find(marker) {
            let start = offset + found;
            let end = secret_token_end(bytes, start);
            if end.saturating_sub(start) >= 16 {
                ranges.push((start, end));
            }
            offset = end.max(start + marker.len());
        }
    }

    if ranges.is_empty() {
        return value.into();
    }
    ranges.sort_unstable();
    let mut merged = Vec::<(usize, usize)>::new();
    for (start, end) in ranges {
        if let Some(last) = merged.last_mut()
            && start <= last.1
        {
            last.1 = last.1.max(end);
        } else {
            merged.push((start, end));
        }
    }
    let mut result = String::with_capacity(value.len());
    let mut cursor = 0;
    for (start, end) in merged {
        result.push_str(&value[cursor..start]);
        result.push_str("[REDACTED]");
        cursor = end;
    }
    result.push_str(&value[cursor..]);
    result
}

pub fn redact(mut value: serde_json::Value) -> serde_json::Value {
    match &mut value {
        serde_json::Value::Object(map) => {
            for (key, item) in map.iter_mut() {
                if sensitive_key(key) {
                    *item = serde_json::Value::String("[REDACTED]".into())
                } else {
                    *item = redact(item.take())
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                *item = redact(item.take())
            }
        }
        serde_json::Value::String(text) => *text = redact_text(text),
        _ => {}
    }
    value
}

pub fn redact_with_secrets(
    value: serde_json::Value,
    secrets: impl IntoIterator<Item = impl AsRef<str>>,
) -> serde_json::Value {
    let secrets = secrets
        .into_iter()
        .map(|secret| secret.as_ref().to_owned())
        .filter(|secret| !secret.is_empty())
        .collect::<Vec<_>>();
    fn exact(mut value: serde_json::Value, secrets: &[String]) -> serde_json::Value {
        match &mut value {
            serde_json::Value::Object(map) => {
                for item in map.values_mut() {
                    *item = exact(item.take(), secrets);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    *item = exact(item.take(), secrets);
                }
            }
            serde_json::Value::String(text) => {
                for secret in secrets {
                    *text = text.replace(secret, "[REDACTED]");
                }
            }
            _ => {}
        }
        value
    }
    exact(redact(value), &secrets)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_secrets_are_removed() {
        let v = redact(serde_json::json!({
            "headers":{
                "Authorization":"Bearer x",
                "access_token":"secret",
                "accessToken":"secret",
                "refreshToken":"secret",
                "apiKey":"secret",
                "x-api-key":"secret",
                "authorizationHeader":"secret",
            },
            "clientSecret":"secret",
            "privateKey":"secret",
            "usage":{"input_tokens":12,"output_tokens":4,"max_tokens":8192},
            "ok":"safe"
        }));
        assert_eq!(v["headers"]["Authorization"], "[REDACTED]");
        assert_eq!(v["headers"]["access_token"], "[REDACTED]");
        assert_eq!(v["headers"]["accessToken"], "[REDACTED]");
        assert_eq!(v["headers"]["refreshToken"], "[REDACTED]");
        assert_eq!(v["headers"]["apiKey"], "[REDACTED]");
        assert_eq!(v["headers"]["x-api-key"], "[REDACTED]");
        assert_eq!(v["headers"]["authorizationHeader"], "[REDACTED]");
        assert_eq!(v["clientSecret"], "[REDACTED]");
        assert_eq!(v["privateKey"], "[REDACTED]");
        assert_eq!(v["usage"]["input_tokens"], 12);
        assert_eq!(v["usage"]["output_tokens"], 4);
        assert_eq!(v["usage"]["max_tokens"], 8192);
        assert_eq!(v["ok"], "safe");
    }

    #[test]
    fn token_metrics_and_protocol_metadata_are_not_credentials() {
        let value = redact(serde_json::json!({
            "completion_tokens": 17,
            "prompt_tokens": 23,
            "estimated_tokens": 31,
            "tokens_used": 40,
            "token_budget": 100,
            "token_type": "Bearer",
            "token_endpoint": "https://identity.example/token",
            "progressToken": "request-7",
            "access_token": "private-access",
            "refreshToken": "private-refresh",
            "bearer_token": "private-bearer",
            "client_secret": "private-client"
        }));
        assert_eq!(value["completion_tokens"], 17);
        assert_eq!(value["prompt_tokens"], 23);
        assert_eq!(value["estimated_tokens"], 31);
        assert_eq!(value["tokens_used"], 40);
        assert_eq!(value["token_budget"], 100);
        assert_eq!(value["token_type"], "Bearer");
        assert_eq!(value["token_endpoint"], "https://identity.example/token");
        assert_eq!(value["progressToken"], "request-7");
        assert_eq!(value["access_token"], "[REDACTED]");
        assert_eq!(value["refreshToken"], "[REDACTED]");
        assert_eq!(value["bearer_token"], "[REDACTED]");
        assert_eq!(value["client_secret"], "[REDACTED]");
    }

    #[test]
    fn process_output_redacts_assignments_headers_and_known_token_shapes() {
        let value = redact(serde_json::json!({
            "stdout": "OPENAI_API_KEY=sk-project-1234567890\nAuthorization: Bearer header-secret\nnormal=visible\n",
            "stderr": "github ghp_1234567890abcdef"
        }));
        let output = serde_json::to_string(&value).unwrap();
        assert!(!output.contains("project-1234567890"));
        assert!(!output.contains("header-secret"));
        assert!(!output.contains("ghp_1234567890abcdef"));
        assert!(output.contains("normal=visible"));
    }

    #[test]
    fn exact_secret_redaction_covers_unstructured_extension_output() {
        let value = redact_with_secrets(
            serde_json::json!({"stdout":"arbitrary-value-from-handle"}),
            ["arbitrary-value-from-handle"],
        );
        assert_eq!(value["stdout"], "[REDACTED]");
    }

    #[test]
    fn persisted_head_resumes_the_same_chain() {
        let event = Event {
            id: opencoding_protocol::Id("event".into()),
            sequence: 1,
            timestamp: chrono::Utc::now(),
            scope: opencoding_protocol::Scope {
                organization_id: opencoding_protocol::Id("org".into()),
                team_id: opencoding_protocol::Id("team".into()),
                actor_id: opencoding_protocol::Id("actor".into()),
                goal_id: None,
                task_id: None,
            },
            session_id: None,
            turn_id: None,
            kind: "test".into(),
            payload: serde_json::json!({}),
        };
        let mut original = HashChain::default();
        let head = original.append(&event).unwrap();
        let resumed = HashChain::from_head(&head).unwrap();
        assert_eq!(resumed.head(), original.head());
        assert!(HashChain::from_head("invalid").is_err());
        assert!(HashChain::verify([(&event, "00")]).is_err());
    }

    #[test]
    fn signed_metadata_batch_verifies_without_persisting_event_content() {
        let event = Event {
            id: Id("event".into()),
            sequence: 1,
            timestamp: Utc::now(),
            scope: Scope {
                organization_id: Id("org".into()),
                team_id: Id("team".into()),
                actor_id: Id("actor".into()),
                goal_id: None,
                task_id: None,
            },
            session_id: Some(Id("session".into())),
            turn_id: Some(Id("turn".into())),
            kind: "tool.completed".into(),
            payload: serde_json::json!({"command_output":"private source"}),
        };
        let mut chain = HashChain::default();
        let head = chain.append(&event).unwrap();
        let signer =
            CentralAuditSigner::from_base64("source-key", &URL_SAFE_NO_PAD.encode([7_u8; 32]))
                .unwrap();
        let envelope = signer
            .sign(CentralAuditBatchPayload {
                schema_version: CENTRAL_AUDIT_SCHEMA_VERSION,
                batch_id: Id("batch".into()),
                source_id: Id("source".into()),
                organization_id: Id("org".into()),
                team_id: Id("team".into()),
                previous_sequence: 0,
                previous_chain_hash: "0".repeat(64),
                generated_at: Utc::now(),
                records: vec![
                    CentralAuditRecord::metadata_only(&event, Id("source".into()), 1, head)
                        .unwrap(),
                ],
                encrypted_content: None,
            })
            .unwrap();
        verify_central_audit_batch(&envelope, "source-key", &signer.public_key_base64()).unwrap();
        let serialized = serde_json::to_string(&envelope).unwrap();
        assert!(!serialized.contains("private source"));
        let mut tampered = envelope;
        tampered.payload.records[0].kind = "tool.failed".into();
        assert!(
            verify_central_audit_batch(&tampered, "source-key", &signer.public_key_base64(),)
                .is_err()
        );
    }

    #[test]
    fn encrypted_content_is_opt_in_bound_and_authenticated() {
        let event = Event {
            id: Id("content-event".into()),
            sequence: 1,
            timestamp: Utc::now(),
            scope: Scope {
                organization_id: Id("org".into()),
                team_id: Id("team".into()),
                actor_id: Id("actor".into()),
                goal_id: None,
                task_id: Some(Id("task".into())),
            },
            session_id: Some(Id("session".into())),
            turn_id: Some(Id("turn".into())),
            kind: "tool.completed".into(),
            payload: serde_json::json!({"command_output":"private source content"}),
        };
        let mut chain = HashChain::default();
        let head = chain.append(&event).unwrap();
        let mut batch = CentralAuditBatchPayload {
            schema_version: CENTRAL_AUDIT_SCHEMA_VERSION,
            batch_id: Id("content-batch".into()),
            source_id: Id("source".into()),
            organization_id: Id("org".into()),
            team_id: Id("team".into()),
            previous_sequence: 0,
            previous_chain_hash: "0".repeat(64),
            generated_at: event.timestamp,
            records: vec![
                CentralAuditRecord::metadata_only(&event, Id("source".into()), 1, head).unwrap(),
            ],
            encrypted_content: None,
        };
        let data_key = [19_u8; 32];
        let material = CentralAuditDataKeyMaterial::new(
            &data_key,
            CentralAuditWrappedDataKey {
                key_id: "kms/team-a/audit-content".into(),
                algorithm: AES_256_GCM.into(),
                nonce: URL_SAFE_NO_PAD.encode([3_u8; 12]),
                ciphertext: URL_SAFE_NO_PAD.encode([4_u8; 48]),
            },
        )
        .unwrap();
        batch.encrypted_content = Some(
            encrypt_central_audit_content(&batch, std::slice::from_ref(&event), material).unwrap(),
        );
        let signer =
            CentralAuditSigner::from_base64("source-key", &URL_SAFE_NO_PAD.encode([7_u8; 32]))
                .unwrap();
        let envelope = signer.sign(batch).unwrap();
        verify_central_audit_batch(&envelope, "source-key", &signer.public_key_base64()).unwrap();
        let serialized = serde_json::to_string(&envelope).unwrap();
        assert!(!serialized.contains("private source content"));
        assert_eq!(
            decrypt_central_audit_content(&envelope.payload, &data_key).unwrap(),
            vec![event.payload.clone()]
        );
        assert!(decrypt_central_audit_content(&envelope.payload, &[20_u8; 32]).is_err());

        let mut wrong_batch = envelope.payload.clone();
        wrong_batch.batch_id = Id("different-batch".into());
        assert!(decrypt_central_audit_content(&wrong_batch, &data_key).is_err());
        let mut misbound = envelope.payload;
        misbound.encrypted_content.as_mut().unwrap().records[0].local_sequence = 2;
        assert!(misbound.validate().is_err());
    }
}
