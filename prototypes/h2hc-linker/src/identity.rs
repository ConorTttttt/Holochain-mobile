//! Persistent ed25519 keypair management for the linker.
//!
//! The linker's stable identity is a plain ed25519 keypair (not a Holochain
//! agent key). It is used for signing kitsune2 report entries and for
//! joining-service registration heartbeats.
//!
//! Load priority:
//! 1. `H2HC_LINKER_PRIVATE_KEY` env var (base64-encoded 32-byte seed)
//! 2. `H2HC_LINKER_KEY_FILE` file path (raw 32-byte seed)
//! 3. Generate new keypair and write to key file path

use base64::Engine;
use ed25519_dalek::SigningKey;
use std::path::{Path, PathBuf};

/// Configuration for loading the linker identity.
#[derive(Debug, Clone)]
pub struct IdentityConfig {
    /// Path to the key file (default: `./linker-key.ed25519`).
    pub key_file: PathBuf,
    /// Base64-encoded 32-byte seed from `H2HC_LINKER_PRIVATE_KEY`.
    pub private_key_base64: Option<String>,
}

impl Default for IdentityConfig {
    fn default() -> Self {
        Self {
            key_file: PathBuf::from("./linker-key.ed25519"),
            private_key_base64: None,
        }
    }
}

/// A persistent ed25519 identity for the linker.
pub struct LinkerIdentity {
    signing_key: SigningKey,
}

impl std::fmt::Debug for LinkerIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkerIdentity")
            .field("pubkey", &self.public_key_base64())
            .finish()
    }
}

impl LinkerIdentity {
    /// Load or generate the linker identity.
    ///
    /// Priority: env var > key file > generate new.
    pub fn load(config: &IdentityConfig) -> anyhow::Result<Self> {
        // 1. Try base64-encoded env var
        if let Some(ref b64) = config.private_key_base64 {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| anyhow::anyhow!("H2HC_LINKER_PRIVATE_KEY is not valid base64: {e}"))?;
            let seed: [u8; 32] = bytes.try_into().map_err(|v: Vec<u8>| {
                anyhow::anyhow!(
                    "H2HC_LINKER_PRIVATE_KEY must decode to 32 bytes, got {}",
                    v.len()
                )
            })?;
            let signing_key = SigningKey::from_bytes(&seed);
            tracing::info!(
                pubkey = %Self::encode_pubkey(&signing_key),
                "Loaded linker identity from H2HC_LINKER_PRIVATE_KEY"
            );
            return Ok(Self { signing_key });
        }

        // 2. Try key file
        if config.key_file.exists() {
            let signing_key = Self::load_from_file(&config.key_file)?;
            tracing::info!(
                pubkey = %Self::encode_pubkey(&signing_key),
                path = %config.key_file.display(),
                "Loaded linker identity from key file"
            );
            return Ok(Self { signing_key });
        }

        // 3. Generate new and persist
        let secret: [u8; 32] = rand::random();
        let signing_key = SigningKey::from_bytes(&secret);
        Self::write_to_file(&config.key_file, &signing_key)?;
        tracing::info!(
            pubkey = %Self::encode_pubkey(&signing_key),
            path = %config.key_file.display(),
            "Generated new linker identity and saved to key file"
        );
        Ok(Self { signing_key })
    }

    /// Get a reference to the signing key.
    pub fn signing_key(&self) -> &SigningKey {
        &self.signing_key
    }

    /// The public key encoded as standard base64 (for joining-service protocol).
    pub fn public_key_base64(&self) -> String {
        Self::encode_pubkey(&self.signing_key)
    }

    fn encode_pubkey(key: &SigningKey) -> String {
        base64::engine::general_purpose::STANDARD.encode(key.verifying_key().as_bytes())
    }

    fn load_from_file(path: &Path) -> anyhow::Result<SigningKey> {
        let bytes = std::fs::read(path)
            .map_err(|e| anyhow::anyhow!("Failed to read key file {}: {e}", path.display()))?;
        let seed: [u8; 32] = bytes.try_into().map_err(|v: Vec<u8>| {
            anyhow::anyhow!(
                "Key file {} must contain exactly 32 bytes, got {}",
                path.display(),
                v.len()
            )
        })?;
        Ok(SigningKey::from_bytes(&seed))
    }

    fn write_to_file(path: &Path, key: &SigningKey) -> anyhow::Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    anyhow::anyhow!(
                        "Failed to create directory for key file {}: {e}",
                        path.display()
                    )
                })?;
            }
        }

        std::fs::write(path, key.to_bytes())
            .map_err(|e| anyhow::anyhow!("Failed to write key file {}: {e}", path.display()))?;

        // Set file permissions to 0600 (owner read/write only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(
                |e| anyhow::anyhow!("Failed to set permissions on {}: {e}", path.display()),
            )?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_and_persist_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("test-key.ed25519");

        // Generate
        let config = IdentityConfig {
            key_file: key_path.clone(),
            private_key_base64: None,
        };
        let identity1 = LinkerIdentity::load(&config).unwrap();
        assert!(key_path.exists());

        // Verify file is 32 bytes
        let bytes = std::fs::read(&key_path).unwrap();
        assert_eq!(bytes.len(), 32);

        // Reload from same path
        let identity2 = LinkerIdentity::load(&config).unwrap();
        assert_eq!(identity1.public_key_base64(), identity2.public_key_base64());
    }

    #[test]
    fn test_load_from_base64_env() {
        let secret: [u8; 32] = rand::random();
        let b64 = base64::engine::general_purpose::STANDARD.encode(secret);

        let dir = tempfile::tempdir().unwrap();
        let config = IdentityConfig {
            key_file: dir.path().join("should-not-be-created.ed25519"),
            private_key_base64: Some(b64),
        };
        let identity = LinkerIdentity::load(&config).unwrap();

        // Verify it loaded the right key
        let expected = SigningKey::from_bytes(&secret);
        assert_eq!(
            identity.public_key_base64(),
            LinkerIdentity::encode_pubkey(&expected)
        );

        // Key file should NOT have been created
        assert!(!config.key_file.exists());
    }

    #[test]
    fn test_env_var_takes_precedence_over_file() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("existing-key.ed25519");

        // Write a key to file
        let file_secret: [u8; 32] = rand::random();
        let file_key = SigningKey::from_bytes(&file_secret);
        LinkerIdentity::write_to_file(&key_path, &file_key).unwrap();

        // Load with a different key via base64
        let env_secret: [u8; 32] = rand::random();
        let env_b64 = base64::engine::general_purpose::STANDARD.encode(env_secret);

        let config = IdentityConfig {
            key_file: key_path,
            private_key_base64: Some(env_b64),
        };
        let identity = LinkerIdentity::load(&config).unwrap();

        // Should use the env var key, not the file key
        let expected = SigningKey::from_bytes(&env_secret);
        assert_eq!(
            identity.public_key_base64(),
            LinkerIdentity::encode_pubkey(&expected)
        );
    }

    #[test]
    fn test_invalid_base64_env() {
        let config = IdentityConfig {
            key_file: PathBuf::from("/nonexistent"),
            private_key_base64: Some("not-valid-base64!!!".to_string()),
        };
        let err = LinkerIdentity::load(&config).unwrap_err();
        assert!(
            err.to_string().contains("not valid base64"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_wrong_length_env() {
        let b64 = base64::engine::general_purpose::STANDARD.encode([0u8; 16]); // 16 bytes, not 32
        let config = IdentityConfig {
            key_file: PathBuf::from("/nonexistent"),
            private_key_base64: Some(b64),
        };
        let err = LinkerIdentity::load(&config).unwrap_err();
        assert!(
            err.to_string().contains("32 bytes"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_invalid_key_file() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("bad-key.ed25519");
        std::fs::write(&key_path, b"too short").unwrap();

        let config = IdentityConfig {
            key_file: key_path,
            private_key_base64: None,
        };
        let err = LinkerIdentity::load(&config).unwrap_err();
        assert!(
            err.to_string().contains("32 bytes"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_public_key_base64_is_standard_base64() {
        let secret: [u8; 32] = rand::random();
        let key = SigningKey::from_bytes(&secret);
        let b64 = LinkerIdentity::encode_pubkey(&key);

        // Standard base64 uses +/ (not -_) and may have = padding
        // Verify it round-trips with STANDARD engine
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&b64)
            .unwrap();
        assert_eq!(decoded.len(), 32);
        assert_eq!(decoded, key.verifying_key().as_bytes());
    }

    #[cfg(unix)]
    #[test]
    fn test_key_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("perm-test.ed25519");

        let config = IdentityConfig {
            key_file: key_path.clone(),
            private_key_base64: None,
        };
        let _identity = LinkerIdentity::load(&config).unwrap();

        let perms = std::fs::metadata(&key_path).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o600);
    }
}
