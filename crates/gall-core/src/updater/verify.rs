//! Minisign verification helpers for update archives and binaries.

use base64::Engine;
use minisign_verify::{PublicKey, Signature};
use std::io::Read;
use std::path::Path;

/// Decodes a Minisign public key from either:
/// - a base64-encoded `minisign.pub` text block (as stored in `GALL_UPDATER_PUBLIC_KEY` and `.key.pub`),
/// - raw two-line `minisign.pub` text, or
/// - a single-line base64 public key (`RW...`).
pub fn decode_pubkey(raw: &str) -> Result<PublicKey, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("No update signing public key is configured.".into());
    }
    if let Ok(decoded_bytes) = base64::engine::general_purpose::STANDARD.decode(trimmed) {
        if let Ok(text) = std::str::from_utf8(&decoded_bytes) {
            if let Ok(pk) = PublicKey::decode(text) {
                return Ok(pk);
            }
        }
    }
    PublicKey::decode(trimmed)
        .or_else(|_| PublicKey::from_base64(trimmed))
        .map_err(|e| format!("Invalid update public key: {e}"))
}

/// Decodes a Minisign signature from either a base64-encoded `.sig` file (Tauri CLI output /
/// `latest.json` format) or raw four-line Minisign text.
pub fn decode_signature(raw: &str) -> Result<Signature, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Update signature is empty.".into());
    }
    if let Ok(decoded_bytes) = base64::engine::general_purpose::STANDARD.decode(trimmed) {
        if let Ok(text) = std::str::from_utf8(&decoded_bytes) {
            if let Ok(sig) = Signature::decode(text) {
                return Ok(sig);
            }
        }
    }
    Signature::decode(trimmed).map_err(|e| format!("Invalid update signature: {e}"))
}

/// Extracts `version:<ver>` from a verified Minisign trusted comment.
pub fn signed_version(trusted_comment: &str) -> Option<&str> {
    trusted_comment
        .split('\t')
        .find_map(|field| field.strip_prefix("version:"))
}

fn verify_signed_version(
    signature: &Signature,
    expected_version: Option<&str>,
    require_signed_version: bool,
) -> Result<(), String> {
    let Some(expected) = expected_version else {
        return Ok(());
    };
    let Some(signed) = signed_version(signature.trusted_comment()) else {
        return if require_signed_version {
            Err("Update signature does not contain a signed version.".into())
        } else {
            Ok(())
        };
    };
    let matches = match (
        semver::Version::parse(signed.trim_start_matches('v')),
        semver::Version::parse(expected.trim_start_matches('v')),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => signed == expected,
    };
    if matches {
        Ok(())
    } else {
        Err(format!(
            "Signed update version mismatch: expected {expected}, signature is for {signed}."
        ))
    }
}

/// Verifies an in-memory payload against a Minisign public key and signature.
pub fn verify_bytes(
    pubkey_raw: &str,
    sig_raw: &str,
    data: &[u8],
    expected_version: Option<&str>,
    require_signed_version: bool,
) -> Result<(), String> {
    let pk = decode_pubkey(pubkey_raw)?;
    let sig = decode_signature(sig_raw)?;
    pk.verify(data, &sig, true)
        .map_err(|e| format!("Signature verification failed: {e}"))?;
    verify_signed_version(&sig, expected_version, require_signed_version)
}

/// Streams a file from disk and verifies its Minisign signature.
pub fn verify_file(
    pubkey_raw: &str,
    sig_path: &Path,
    file_path: &Path,
    expected_version: Option<&str>,
    require_signed_version: bool,
) -> Result<(), String> {
    let sig_text = std::fs::read_to_string(sig_path)
        .map_err(|e| format!("Cannot read signature {}: {e}", sig_path.display()))?;
    let pk = decode_pubkey(pubkey_raw)?;
    let sig = decode_signature(&sig_text)?;
    match pk.verify_stream(&sig) {
        Ok(mut verifier) => {
            let mut file = std::fs::File::open(file_path)
                .map_err(|e| format!("Cannot open {}: {e}", file_path.display()))?;
            let mut buf = [0u8; 64 * 1024];
            loop {
                let n = file
                    .read(&mut buf)
                    .map_err(|e| format!("Cannot read {}: {e}", file_path.display()))?;
                if n == 0 {
                    break;
                }
                verifier.update(&buf[..n]);
            }
            verifier
                .finalize()
                .map_err(|e| format!("Signature verification failed: {e}"))?;
        }
        Err(minisign_verify::Error::UnsupportedLegacyMode) => {
            let data = std::fs::read(file_path)
                .map_err(|e| format!("Cannot read {}: {e}", file_path.display()))?;
            pk.verify(&data, &sig, true)
                .map_err(|e| format!("Signature verification failed: {e}"))?;
        }
        Err(e) => return Err(format!("Signature verification failed: {e}")),
    }
    verify_signed_version(&sig, expected_version, require_signed_version)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Throwaway test keypair and signatures generated via `tauri signer`.
    const TEST_PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEZGMDQwOTBBNTZGQjMyOUEKUldTYU12dFdDZ2tFLzRnVFQ5Nk05YVNaMlVOWWoxNm9tdXE1U002MG1PblZ5M1JsUmlrNk03ZXQK";
    const TEST_PAYLOAD: &[u8] = b"#!/bin/sh\necho \"GALL_PROBE_OK 9.9.9\"\n";
    const TEST_SIG: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVTYU12dFdDZ2tFLy9KMnptWGFQcUJRTmhIMU5PZkxZdUJISmRsdEFkbmRla1ZqUVZZa1VYTzY3b0FrbXAzcTlzMTk1MnR0TGZ2a3lLMWNHRWd5YjlvTWJ5UExzUXpxZWdrPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk1MTczNDc4CWZpbGU6Z2FsbAl2ZXJzaW9uOjkuOS45CmxVRjlPRnViQzJXOXppMzZNT2JoUmNTYXJ4Njc5RjJ3N3dnRzJJTzlOYVV2OTlDZ09KZ2cwSmcyYlgvYnVsTEhMb2VJOUlKdVBvZEhaUU1jNnF6QkR3PT0K";

    #[test]
    fn verifies_tauri_signer_bytes_and_file_and_enforces_version() {
        verify_bytes(TEST_PUBKEY, TEST_SIG, TEST_PAYLOAD, Some("9.9.9"), true).unwrap();
        assert!(verify_bytes(TEST_PUBKEY, TEST_SIG, b"tampered", Some("9.9.9"), true).is_err());
        assert!(verify_bytes(TEST_PUBKEY, TEST_SIG, TEST_PAYLOAD, Some("1.0.0"), true).is_err());
        assert!(verify_bytes("bad-key", TEST_SIG, TEST_PAYLOAD, Some("9.9.9"), true).is_err());

        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("gall");
        let sp = t.path().join("gall.sig");
        std::fs::write(&p, TEST_PAYLOAD).unwrap();
        std::fs::write(&sp, TEST_SIG).unwrap();
        verify_file(TEST_PUBKEY, &sp, &p, Some("9.9.9"), true).unwrap();

        std::fs::write(&p, b"tampered").unwrap();
        assert!(verify_file(TEST_PUBKEY, &sp, &p, Some("9.9.9"), true).is_err());
    }
}
