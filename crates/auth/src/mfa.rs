// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Multi-factor authentication primitives for Phase 4 M-E3.
//!
//! Three concerns live here:
//!
//! 1. **At-rest encryption of the TOTP secret.** `EncryptedString`
//!    wraps an AES-256-GCM ciphertext blob. The plaintext (20-byte
//!    Base32 TOTP secret) is decrypted only when the server needs to
//!    verify a code — never read directly off the User row. The
//!    encryption key comes from `MFA_ENCRYPTION_KEY`, a 32-byte value
//!    supplied via env (base64-url-no-pad). Rotating the key is a
//!    forced-re-enroll event: there's no envelope to wrap old
//!    ciphertexts under a new key (KMS DEK pattern is a v2 carry-
//!    forward).
//!
//! 2. **TOTP generation / verification** via `totp-rs` (RFC 6238).
//!    6-digit codes, 30-second window, SHA-1 hash (the algorithm every
//!    authenticator app supports). A `±1` step tolerance covers minor
//!    clock skew between the user's phone and the server.
//!
//! 3. **Recovery-code generation + verification.** Ten random
//!    base32-encoded codes minted at enroll-time. Each is bcrypt-
//!    hashed at rest (cost 10) and presented to the user exactly
//!    once. A successful redemption deletes the row, so re-use is
//!    structurally impossible.

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng as AesOsRng};
use aes_gcm::Aes256Gcm;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

pub use ogrenotes_storage::models::user::EncryptedString;

/// Errors surfaced by the MFA primitives. The public API NEVER leaks
/// detail about what failed during decrypt/verify — the variants here
/// are for server logging only. Routes that call into this module
/// collapse all of them to a single `ApiError::Unauthorized` so an
/// attacker cannot distinguish "bad code" from "missing key" or
/// "ciphertext corrupted."
#[derive(Debug, thiserror::Error)]
pub enum MfaError {
    #[error("MFA encryption key is missing or malformed")]
    KeyConfig,
    #[error("ciphertext too short or corrupt")]
    BadCiphertext,
    #[error("decrypt failed")]
    Decrypt,
    #[error("encrypt failed")]
    Encrypt,
    #[error("invalid TOTP code")]
    InvalidCode,
    #[error("TOTP secret malformed: {0}")]
    BadSecret(String),
    #[error("recovery code mismatch")]
    BadRecoveryCode,
    #[error("bcrypt: {0}")]
    Bcrypt(String),
}

// `EncryptedString` lives in the storage crate so the User row can
// name the type without a circular dep (auth → storage exists, the
// reverse would close a cycle). We re-export it here so callers
// reach for the type via `auth::mfa::EncryptedString` next to the
// encrypt/decrypt functions that operate on it — a single discovery
// path even though the definition lives one crate down.

/// Read the 32-byte AES key from `MFA_ENCRYPTION_KEY`. Format is
/// base64-url-no-pad. Surfaces `KeyConfig` if missing/short — the
/// caller decides whether to fail the request or fail server startup.
pub fn load_key() -> Result<[u8; 32], MfaError> {
    let raw = std::env::var("MFA_ENCRYPTION_KEY").map_err(|_| MfaError::KeyConfig)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(raw.trim())
        .map_err(|_| MfaError::KeyConfig)?;
    if bytes.len() != 32 {
        return Err(MfaError::KeyConfig);
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// Encrypt a plaintext under the supplied 32-byte key. Fresh random
/// nonce each call. Returns the self-describing blob ready to write
/// into a User row.
pub fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> Result<EncryptedString, MfaError> {
    let cipher = Aes256Gcm::new(key.into());
    let nonce = Aes256Gcm::generate_nonce(&mut AesOsRng);
    let ct = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|_| MfaError::Encrypt)?;
    Ok(EncryptedString {
        nonce: URL_SAFE_NO_PAD.encode(nonce),
        ct: URL_SAFE_NO_PAD.encode(ct),
    })
}

/// Inverse of `encrypt`. Returns `BadCiphertext` if the blob fields
/// fail base64 / length checks, `Decrypt` for any authentication
/// failure — the caller MUST collapse both into a generic 401 so an
/// attacker can't fingerprint key state.
pub fn decrypt(key: &[u8; 32], blob: &EncryptedString) -> Result<Vec<u8>, MfaError> {
    let nonce = URL_SAFE_NO_PAD
        .decode(blob.nonce.as_bytes())
        .map_err(|_| MfaError::BadCiphertext)?;
    if nonce.len() != 12 {
        return Err(MfaError::BadCiphertext);
    }
    let ct = URL_SAFE_NO_PAD
        .decode(blob.ct.as_bytes())
        .map_err(|_| MfaError::BadCiphertext)?;
    let cipher = Aes256Gcm::new(key.into());
    cipher
        .decrypt(nonce.as_slice().into(), ct.as_slice())
        .map_err(|_| MfaError::Decrypt)
}

// ─── TOTP ────────────────────────────────────────────────────

/// Build a `totp_rs::TOTP` from a Base32 secret. RFC 6238 defaults:
/// SHA-1, 6 digits, 30-second step. SHA-1 is the algorithm every
/// authenticator app supports — Google Authenticator, 1Password, etc.
/// don't all speak SHA-256 / SHA-512.
pub fn totp_for(secret_b32: &str, issuer: &str, account: &str) -> Result<totp_rs::TOTP, MfaError> {
    let secret = totp_rs::Secret::Encoded(secret_b32.to_string())
        .to_bytes()
        .map_err(|e| MfaError::BadSecret(format!("{e:?}")))?;
    totp_rs::TOTP::new(
        totp_rs::Algorithm::SHA1,
        6,
        1,  // step tolerance: accept current ±1 step (30s either side)
        30,
        secret,
        Some(issuer.to_string()),
        account.to_string(),
    )
    .map_err(|e| MfaError::BadSecret(format!("{e}")))
}

/// Generate a fresh 20-byte (160-bit) random secret encoded as Base32
/// — the format every authenticator app accepts in the `otpauth://`
/// URI. Used at enrollment time.
pub fn new_totp_secret() -> String {
    totp_rs::Secret::generate_secret().to_encoded().to_string()
}

/// Verify a 6-digit TOTP code against the user's secret. Returns
/// `true` on match within the configured step tolerance.
pub fn verify_totp(secret_b32: &str, code: &str, issuer: &str, account: &str) -> bool {
    match totp_for(secret_b32, issuer, account) {
        Ok(t) => t.check_current(code).unwrap_or(false),
        Err(_) => false,
    }
}

// ─── Recovery codes ──────────────────────────────────────────

/// Number of recovery codes minted at enrollment. RFC has no opinion;
/// 10 is the de-facto industry default (GitHub, Google) — high enough
/// to survive a phone reset without re-enrollment, low enough that a
/// dump doesn't yield a useful key for brute-force.
pub const RECOVERY_CODE_COUNT: usize = 10;

/// Format: `xxxxx-xxxxx` where each x is base32 (A-Z2-7). 50 bits of
/// entropy total — comfortably above the bcrypt-online-attack
/// threshold and human-typeable.
pub fn generate_recovery_codes() -> Vec<String> {
    (0..RECOVERY_CODE_COUNT).map(|_| one_recovery_code()).collect()
}

fn one_recovery_code() -> String {
    // 10 random bytes → 10 base32 alphabet chars (one per byte, low
    // 5 bits). `rand::random::<[u8; 10]>()` pulls from the same
    // ChaCha-seeded-from-OS source as the rest of the codebase
    // without us depending on a specific RngCore version.
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let bytes: [u8; 10] = rand::random();
    let chars: String = bytes.iter().map(|b| ALPHABET[(*b as usize) % 32] as char).collect();
    format!("{}-{}", &chars[..5], &chars[5..])
}

/// Hash a recovery code for at-rest storage. Cost 10 strikes the same
/// balance the bcrypt crate's defaults pick — ~100 ms verify on a
/// modern server, slow enough to thwart offline attack on the small
/// 50-bit search space if the DDB row leaks.
pub fn hash_recovery_code(code: &str) -> Result<String, MfaError> {
    bcrypt::hash(code, 10).map_err(|e| MfaError::Bcrypt(e.to_string()))
}

/// Verify a presented code against its stored bcrypt hash. Returns
/// `false` for any failure path so the caller can map cleanly to a
/// single 401.
pub fn verify_recovery_code(code: &str, hash: &str) -> bool {
    bcrypt::verify(code, hash).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_key() -> [u8; 32] {
        // Deterministic so encrypt/decrypt roundtrip tests don't pull
        // env vars. Real config-side path is exercised separately.
        let mut k = [0u8; 32];
        for (i, slot) in k.iter_mut().enumerate() {
            *slot = i as u8;
        }
        k
    }

    #[test]
    fn aes_roundtrip_recovers_plaintext() {
        let key = fixed_key();
        let plaintext = b"OPDPP3XJZX5JV6KH";  // looks like a TOTP secret
        let blob = encrypt(&key, plaintext).expect("encrypt");
        let back = decrypt(&key, &blob).expect("decrypt");
        assert_eq!(back, plaintext);
    }

    #[test]
    fn aes_fresh_nonce_per_encrypt() {
        // Critical GCM invariant — same key, same plaintext, two
        // encrypt() calls must produce DIFFERENT ciphertexts because
        // the nonce is fresh. Reusing a nonce destroys GCM's
        // confidentiality.
        let key = fixed_key();
        let pt = b"identical";
        let a = encrypt(&key, pt).unwrap();
        let b = encrypt(&key, pt).unwrap();
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.ct, b.ct);
    }

    #[test]
    fn aes_decrypt_rejects_tampered_ciphertext() {
        // Flip one bit of the ciphertext; GCM's auth tag MUST notice.
        let key = fixed_key();
        let mut blob = encrypt(&key, b"hello").unwrap();
        // Decode → flip → re-encode the first ciphertext byte.
        let mut bytes = URL_SAFE_NO_PAD.decode(blob.ct.as_bytes()).unwrap();
        bytes[0] ^= 0xff;
        blob.ct = URL_SAFE_NO_PAD.encode(&bytes);
        let res = decrypt(&key, &blob);
        assert!(matches!(res, Err(MfaError::Decrypt)));
    }

    #[test]
    fn totp_generated_code_self_verifies() {
        let secret = new_totp_secret();
        let totp = totp_for(&secret, "OgreNotes", "alice@example.com").unwrap();
        let code = totp.generate_current().unwrap();
        assert!(verify_totp(&secret, &code, "OgreNotes", "alice@example.com"));
    }

    #[test]
    fn totp_rejects_wrong_code() {
        let secret = new_totp_secret();
        assert!(!verify_totp(&secret, "000000", "OgreNotes", "alice@example.com"));
    }

    #[test]
    fn totp_rejects_wrong_secret() {
        let secret_a = new_totp_secret();
        let secret_b = new_totp_secret();
        let totp_a = totp_for(&secret_a, "OgreNotes", "alice@example.com").unwrap();
        let code = totp_a.generate_current().unwrap();
        // Same code, different secret → reject.
        assert!(!verify_totp(&secret_b, &code, "OgreNotes", "alice@example.com"));
    }

    #[test]
    fn recovery_codes_unique_per_call() {
        let a = generate_recovery_codes();
        let b = generate_recovery_codes();
        assert_eq!(a.len(), RECOVERY_CODE_COUNT);
        // Any collision between the two batches is astronomically
        // unlikely (10 × 50 bits each), so a hit is a deterministic-
        // RNG regression.
        let mut all = a.clone();
        all.extend(b);
        all.sort();
        let dedup_len = {
            let mut x = all.clone();
            x.dedup();
            x.len()
        };
        assert_eq!(dedup_len, all.len(), "recovery codes must not collide");
    }

    #[test]
    fn recovery_code_format_is_xxxxx_xxxxx() {
        let code = one_recovery_code();
        assert_eq!(code.len(), 11);
        assert_eq!(&code[5..6], "-");
        assert!(code.chars().take(5).all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()));
    }

    #[test]
    fn recovery_code_bcrypt_roundtrip() {
        let code = "ABCDE-FGHIJ";
        let hash = hash_recovery_code(code).unwrap();
        assert!(verify_recovery_code(code, &hash));
        assert!(!verify_recovery_code("WRONG-CODE!", &hash));
    }

    // ─── decrypt: BadCiphertext + wrong-key paths ────────────────
    // The roundtrip / tamper tests above only exercise the `Decrypt`
    // variant. The `BadCiphertext` variant (malformed blob fields,
    // checked before any AEAD work) had no coverage; these pin its
    // three entry points plus the wrong-key `Decrypt` path. All four
    // matter because routes collapse every MfaError to one opaque 401,
    // so a misclassification here is invisible from the outside.

    #[test]
    fn decrypt_rejects_non_base64_nonce() {
        let key = fixed_key();
        let mut blob = encrypt(&key, b"secret").unwrap();
        // '!' is outside the URL_SAFE_NO_PAD alphabet → decode fails
        // before the length check.
        blob.nonce = "!!!not-base64!!!".to_string();
        assert!(matches!(decrypt(&key, &blob), Err(MfaError::BadCiphertext)));
    }

    #[test]
    fn decrypt_rejects_wrong_length_nonce() {
        let key = fixed_key();
        let mut blob = encrypt(&key, b"secret").unwrap();
        // Valid base64 that decodes to 8 bytes, not the required 12 —
        // exercises the explicit `nonce.len() != 12` guard rather than
        // the base64 decode error.
        blob.nonce = URL_SAFE_NO_PAD.encode([0u8; 8]);
        assert!(matches!(decrypt(&key, &blob), Err(MfaError::BadCiphertext)));
    }

    #[test]
    fn decrypt_rejects_non_base64_ciphertext() {
        let key = fixed_key();
        let mut blob = encrypt(&key, b"secret").unwrap();
        // Nonce stays valid so we reach (and fail) the ct decode.
        blob.ct = "!!!not-base64!!!".to_string();
        assert!(matches!(decrypt(&key, &blob), Err(MfaError::BadCiphertext)));
    }

    #[test]
    fn decrypt_with_wrong_key_fails_authentication() {
        let key_a = fixed_key();
        let mut key_b = fixed_key();
        key_b[0] ^= 0xff; // a different key
        let blob = encrypt(&key_a, b"top secret totp seed").unwrap();
        // Well-formed blob, wrong key → GCM auth tag fails → Decrypt,
        // NOT BadCiphertext.
        assert!(matches!(decrypt(&key_b, &blob), Err(MfaError::Decrypt)));
    }

    // ─── load_key: env-config validation ─────────────────────────
    // Self-contained and serialized within this one test fn — no other
    // test in the crate touches MFA_ENCRYPTION_KEY, so there's no
    // cross-test race on the process-global env. Covers each rejection
    // branch plus the happy path.

    #[test]
    fn load_key_validates_env_format() {
        // SAFETY: single test owns this var; set/remove are paired and
        // no concurrent reader of MFA_ENCRYPTION_KEY exists.
        unsafe {
            // Missing → KeyConfig.
            std::env::remove_var("MFA_ENCRYPTION_KEY");
            assert!(matches!(load_key(), Err(MfaError::KeyConfig)));

            // Malformed base64 → KeyConfig.
            std::env::set_var("MFA_ENCRYPTION_KEY", "!!! not base64 !!!");
            assert!(matches!(load_key(), Err(MfaError::KeyConfig)));

            // Valid base64 but only 16 bytes → KeyConfig (length guard).
            std::env::set_var("MFA_ENCRYPTION_KEY", URL_SAFE_NO_PAD.encode([7u8; 16]));
            assert!(matches!(load_key(), Err(MfaError::KeyConfig)));

            // Exactly 32 bytes → Ok and round-trips the bytes.
            let raw = [0u8, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
                       16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31];
            std::env::set_var("MFA_ENCRYPTION_KEY", URL_SAFE_NO_PAD.encode(raw));
            assert_eq!(load_key().unwrap(), raw);

            std::env::remove_var("MFA_ENCRYPTION_KEY");
        }
    }

    // ─── TOTP: malformed secret + skew tolerance ─────────────────

    #[test]
    fn totp_for_rejects_malformed_secret() {
        // '8' is not in the RFC 4648 base32 alphabet (A-Z, 2-7), so the
        // secret fails to decode.
        let bad = "AAAAAAA8";
        assert!(matches!(
            totp_for(bad, "OgreNotes", "alice@example.com"),
            Err(MfaError::BadSecret(_))
        ));
        // verify_totp must swallow the error and report a plain false
        // rather than panicking — it's on the login hot path.
        assert!(!verify_totp(bad, "123456", "OgreNotes", "alice@example.com"));
    }

    #[test]
    fn totp_accepts_one_step_skew_rejects_two() {
        // Pins the documented "±1 step (30s either side)" clock-skew
        // tolerance. Using fixed timestamps makes this deterministic
        // regardless of wall-clock at test time. t0 sits 20s into its
        // 30s window, so ±30s lands cleanly on the adjacent steps.
        let secret = new_totp_secret();
        let totp = totp_for(&secret, "OgreNotes", "alice@example.com").unwrap();
        let t0 = 1_700_000_020u64;
        let token = totp.generate(t0);

        assert!(totp.check(&token, t0), "same step must verify");
        assert!(totp.check(&token, t0 + 30), "one step ahead is within tolerance");
        assert!(totp.check(&token, t0 - 30), "one step behind is within tolerance");
        assert!(!totp.check(&token, t0 + 60), "two steps ahead must be rejected");
        assert!(!totp.check(&token, t0 - 60), "two steps behind must be rejected");
    }

    #[test]
    fn verify_recovery_code_rejects_garbage_hash() {
        // A stored value that isn't a valid bcrypt hash must yield false,
        // not a panic — bcrypt::verify returns Err and we unwrap_or(false).
        assert!(!verify_recovery_code("ABCDE-FGHIJ", "not-a-bcrypt-hash"));
        assert!(!verify_recovery_code("ABCDE-FGHIJ", ""));
    }
}
