//! Registry tokens: random, shown once at creation, stored only as a SHA-256
//! digest, compared in constant time, never logged and never accepted from a
//! query string.
use base64::Engine;
use sha2::{Digest, Sha256};

pub const TOKEN_PREFIX: &str = "skr_";

/// A fresh high-entropy token. The caller shows it exactly once.
pub fn generate_token() -> Result<String, String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| error.to_string())?;
    Ok(format!(
        "{TOKEN_PREFIX}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    ))
}

pub fn token_hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.trim().as_bytes()))
}

/// The bearer token of an `Authorization` header, if the header is present
/// and well formed. Tokens in query strings are never accepted.
pub fn bearer_token(headers: &axum::http::HeaderMap) -> Option<Result<String, ()>> {
    let value = headers.get(axum::http::header::AUTHORIZATION)?;
    let Ok(text) = value.to_str() else {
        return Some(Err(()));
    };
    let Some(token) = text.strip_prefix("Bearer ") else {
        return Some(Err(()));
    };
    let token = token.trim();
    if token.is_empty() || token.len() > 512 || !token.starts_with(TOKEN_PREFIX) {
        return Some(Err(()));
    }
    Some(Ok(token.to_owned()))
}

pub fn constant_time_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_prefixed_random_and_hashed() {
        let one = generate_token().unwrap();
        let two = generate_token().unwrap();
        assert!(one.starts_with(TOKEN_PREFIX) && two.starts_with(TOKEN_PREFIX));
        assert_ne!(one, two);
        assert_eq!(token_hash(&one).len(), 64);
        assert_ne!(token_hash(&one), token_hash(&two));
        assert!(constant_time_eq(&token_hash(&one), &token_hash(&one)));
        assert!(!constant_time_eq("a", "ab"));
        let mut headers = axum::http::HeaderMap::new();
        assert!(bearer_token(&headers).is_none());
        headers.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {one}").parse().unwrap(),
        );
        assert_eq!(bearer_token(&headers), Some(Ok(one.clone())));
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Basic abc".parse().unwrap(),
        );
        assert_eq!(bearer_token(&headers), Some(Err(())));
    }
}
