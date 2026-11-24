use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuthUser {
    pub id: String,
    pub role: String,
}

#[derive(Serialize, Deserialize)]
struct Claims {
    sub: String,
    role: String,
    iss: String,
    exp: usize,
}

/// Salted SHA-256, `salt$digest`. Deliberately NOT argon2/bcrypt: every stored hash in
/// the live collection is in this format and switching would lock every existing user out.
pub fn hash_password(password: &str) -> String {
    let salt = Uuid::new_v4().to_string();
    let digest = sha256(&salt, password);
    format!("{salt}${digest}")
}

pub fn verify_password(password: &str, encoded: &str) -> bool {
    let Some((salt, hash)) = encoded.split_once('$') else {
        return false;
    };
    sha256(salt, password) == hash
}

fn sha256(salt: &str, password: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(salt.as_bytes());
    hasher.update(password.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn issue_token(
    secret: &str,
    issuer: &str,
    user_id: &str,
    role: &str,
) -> Result<String, jsonwebtoken::errors::Error> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as usize;
    let claims = Claims {
        sub: user_id.to_string(),
        role: role.to_string(),
        iss: issuer.to_string(),
        exp: now + 7 * 24 * 60 * 60,
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
}

pub fn decode_token(secret: &str, token: &str) -> Result<AuthUser, jsonwebtoken::errors::Error> {
    let data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )?;
    Ok(AuthUser {
        id: data.claims.sub,
        role: data.claims.role,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_password_roundtrips_and_is_salted() {
        let hash = hash_password("correct-horse-battery-staple");
        assert!(hash.contains('$'));
        assert!(verify_password("correct-horse-battery-staple", &hash));
        assert_ne!(hash_password("same"), hash_password("same"));
    }

    #[test]
    fn verify_password_rejects_wrong_or_malformed() {
        let hash = hash_password("right");
        assert!(!verify_password("wrong", &hash));
        assert!(!verify_password("x", "no-separator"));
        assert!(!verify_password("x", ""));
    }

    #[test]
    fn token_roundtrip_preserves_identity() {
        let token = issue_token("secret", "issuer", "user-123", "user").unwrap();
        let user = decode_token("secret", &token).unwrap();
        assert_eq!(user.id, "user-123");
        assert_eq!(user.role, "user");
        assert!(decode_token("other-secret", &token).is_err());
        assert!(decode_token("secret", "not.a.jwt").is_err());
    }
}
