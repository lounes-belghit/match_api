use argon2::password_hash::{
    rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
};
use argon2::Argon2;
use rand::RngCore;
use sha2::{Digest, Sha256};

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|err| anyhow::anyhow!("password hashing failed: {err}"))?;
    Ok(hash.to_string())
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

pub fn sha256_hex(value: impl AsRef<[u8]>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_ref());
    hex::encode(hasher.finalize())
}

pub fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

pub struct GeneratedKey {
    pub raw: String,
    pub prefix: String,
    pub hash: String,
}

pub fn generate_api_key() -> GeneratedKey {
    let secret = random_token(24);
    let prefix = random_token(5);
    let raw = format!("m3_{}_{}", prefix, secret);
    let hash = sha256_hex(&raw);
    GeneratedKey { raw, prefix, hash }
}
