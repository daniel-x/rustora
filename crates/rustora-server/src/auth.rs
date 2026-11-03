// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Player authentication: a client signs the server's token with its forum key, the server checks the signature
//! with the public key of the player database (`Server.ValidateClient`, `CryptoUtil.VerifySignature`).

use base64::Engine;
use rsa::pkcs8::DecodePublicKey;
use rsa::{Pkcs1v15Sign, RsaPublicKey};
use rustora_core::fieldloader::load;
use rustora_core::miniyaml::MiniYaml;
use rustora_derive::FieldLoad;
use sha1::{Digest, Sha1};

/// `PlayerProfile`.
#[derive(Clone, Debug, Default, FieldLoad)]
pub struct PlayerProfile {
    pub fingerprint: Option<String>,
    pub public_key: Option<String>,
    pub key_revoked: bool,
    #[field(name = "ProfileID")]
    pub profile_id: i32,
    pub profile_name: Option<String>,
    pub profile_rank: Option<String>,
}

/// `CryptoUtil.VerifySignature` of a PEM key given in base64.
pub fn verify_signature(public_key: &str, data: &str, signature: &str) -> Result<bool, String> {
    let engine = base64::engine::general_purpose::STANDARD;
    let pem = engine.decode(public_key.trim()).map_err(|e| e.to_string())?;
    let pem = String::from_utf8(pem).map_err(|e| e.to_string())?;
    let key = RsaPublicKey::from_public_key_pem(pem.trim()).map_err(|e| e.to_string())?;
    let signature = engine.decode(signature.trim()).map_err(|e| e.to_string())?;
    let hash = Sha1::digest(data.as_bytes());
    Ok(key.verify(Pkcs1v15Sign::new::<Sha1>(), &hash, &signature).is_ok())
}

/// The profile of a client whose signature of `token` is valid under the key of the database answer `yaml`.
pub fn check_profile(yaml: &str, token: &str, signature: &str) -> Result<PlayerProfile, String> {
    let nodes = MiniYaml::from_str(yaml, "player profile", true).map_err(|e| e.to_string())?;
    let first = nodes.first().ok_or("empty profile")?;
    if first.key != "Player" {
        return Err(format!("invalid server response: `{}` is not `Player`", first.key));
    }
    let profile: PlayerProfile = load(&first.value).map_err(|e| e.to_string())?;
    if profile.key_revoked {
        return Err("key revoked".to_string());
    }
    let key = profile.public_key.as_deref().ok_or("profile has no public key")?;
    if verify_signature(key, token, signature)? {
        Ok(profile)
    } else {
        Err("signature verification failed".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::RsaPrivateKey;
    use rsa::pkcs8::{EncodePublicKey, LineEnding};

    /// A database answer for a new key, and the key.
    fn profile_yaml(revoked: bool) -> (String, RsaPrivateKey) {
        let key = RsaPrivateKey::new(&mut rand::thread_rng(), 1024).unwrap();
        let pem = key.to_public_key().to_public_key_pem(LineEnding::LF).unwrap();
        let encoded = base64::engine::general_purpose::STANDARD.encode(pem);
        let yaml = format!(
            "Player:\n\tFingerprint: abc\n\tPublicKey: {encoded}\n\tKeyRevoked: {revoked}\n\tProfileID: 42\n\tProfileName: Tester\n"
        );
        (yaml, key)
    }

    fn sign(key: &RsaPrivateKey, data: &str) -> String {
        let hash = Sha1::digest(data.as_bytes());
        let signature = key.sign(Pkcs1v15Sign::new::<Sha1>(), &hash).unwrap();
        base64::engine::general_purpose::STANDARD.encode(signature)
    }

    #[test]
    fn valid_signature_is_accepted() {
        let (yaml, key) = profile_yaml(false);
        let profile = check_profile(&yaml, "token", &sign(&key, "token")).unwrap();
        assert_eq!(profile.profile_id, 42);
    }

    #[test]
    fn signature_of_other_data_is_rejected() {
        let (yaml, key) = profile_yaml(false);
        assert!(check_profile(&yaml, "token", &sign(&key, "other")).is_err());
    }

    #[test]
    fn revoked_key_is_rejected() {
        let (yaml, key) = profile_yaml(true);
        assert!(check_profile(&yaml, "token", &sign(&key, "token")).is_err());
    }

    #[test]
    fn other_answers_are_rejected() {
        assert!(check_profile("Error:\n\tMessage: x\n", "t", "s").is_err());
    }
}
