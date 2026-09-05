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
