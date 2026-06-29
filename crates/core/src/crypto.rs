//! Optional **encryption at rest** for the on-disk store.
//!
//! Data is encrypted with **XChaCha20-Poly1305**, an AEAD symmetric cipher. The
//! key is derived from a passphrase with **Argon2id**, a memory-hard KDF, so a
//! stolen database file is useless without the passphrase. The 192-bit XChaCha
//! nonce is chosen at random *per value*, so nonce reuse is not a concern even
//! across billions of writes (unlike AES-GCM's 96-bit nonce).
//!
//! ## Why this is already "post-quantum"
//!
//! Encryption at rest is **symmetric**, and symmetric crypto is quantum-resistant:
//! Grover's algorithm only square-roots the search space, so a 256-bit key keeps
//! a ~128-bit security level against a quantum attacker — comfortably safe. There
//! is no such thing as (nor any need for) a "post-quantum block cipher" here.
//!
//! Post-quantum *key encapsulation* (ML-KEM / Kyber) would only add value if the
//! data key were wrapped under an **asymmetric** public key (key escrow, multiple
//! recipients, "harvest-now-decrypt-later" on the wrapped key). For a database
//! protected by a local passphrase, this Argon2id + XChaCha20 path is already
//! post-quantum-safe, so we deliberately keep the asymmetric layer out for now.
//!
//! Both crates are pure Rust (no C toolchain), so the self-contained Windows DLL
//! keeps building unchanged.

use std::path::Path;

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;
use zeroize::Zeroizing;

type HmacSha256 = Hmac<Sha256>;

use crate::error::NucleusError;
use crate::Result;

/// XChaCha20 nonce length (192-bit → safe to pick at random per message).
pub const NONCE_LEN: usize = 24;
/// Argon2id salt length.
pub const SALT_LEN: usize = 16;

/// Argon2id cost parameters. Persisted next to the salt so a database stays
/// openable even if the built-in defaults change in a later release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KdfParams {
    /// Memory cost in KiB.
    pub m_cost: u32,
    /// Iteration (time) cost.
    pub t_cost: u32,
    /// Parallelism (lanes).
    pub p_cost: u32,
}

impl KdfParams {
    /// OWASP-recommended interactive defaults for Argon2id: 19 MiB, t=2, p=1.
    pub const DEFAULT: KdfParams = KdfParams {
        m_cost: 19 * 1024,
        t_cost: 2,
        p_cost: 1,
    };

    /// Serialise to a fixed 12-byte little-endian layout for the crypto header.
    pub fn to_bytes(self) -> [u8; 12] {
        let mut b = [0u8; 12];
        b[0..4].copy_from_slice(&self.m_cost.to_le_bytes());
        b[4..8].copy_from_slice(&self.t_cost.to_le_bytes());
        b[8..12].copy_from_slice(&self.p_cost.to_le_bytes());
        b
    }

    /// Parse the 12-byte layout written by [`to_bytes`](KdfParams::to_bytes).
    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        if b.len() != 12 {
            return Err(NucleusError::crypto("malformed KDF parameters"));
        }
        let r = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        Ok(Self {
            m_cost: r(0),
            t_cost: r(4),
            p_cost: r(8),
        })
    }
}

/// Fill `n` bytes from the OS-seeded cryptographic RNG. `ThreadRng` is a CSPRNG
/// (`CryptoRng`) reseeded from OS entropy — fine for salts and nonces.
pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    rand::rng().fill_bytes(&mut b);
    b
}

/// Derive a 32-byte key from a passphrase and salt with Argon2id. The returned
/// key is wrapped in [`Zeroizing`] so it is scrubbed from memory on drop.
pub fn derive_key(passphrase: &[u8], salt: &[u8], p: KdfParams) -> Result<Zeroizing<[u8; 32]>> {
    let params = Params::new(p.m_cost, p.t_cost, p.p_cost, Some(32))
        .map_err(|e| NucleusError::crypto(format!("argon2 params: {e}")))?;
    let a2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = Zeroizing::new([0u8; 32]);
    a2.hash_password_into(passphrase, salt, key.as_mut_slice())
        .map_err(|e| NucleusError::crypto(format!("argon2 derive: {e}")))?;
    Ok(key)
}

/// Derive a 32-byte subkey from `key` for a given purpose `label`, keeping the
/// AEAD and MAC keys cryptographically independent (`HMAC(key, label)`).
fn derive_subkey(key: &[u8; 32], label: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(label);
    let bytes = mac.finalize().into_bytes();
    let mut out = Zeroizing::new([0u8; 32]);
    out.copy_from_slice(&bytes);
    out
}

/// AEAD cipher over stored values. Each [`seal`](Cipher::seal) prepends a fresh
/// random nonce, so identical plaintexts encrypt to different ciphertexts.
///
/// Also holds a separate **index-MAC** subkey (HKDF-style domain-separated from
/// the master key) used by [`index_token`](Cipher::index_token) to make sensitive
/// lookup keys opaque on disk. Reusing the AEAD key directly for the MAC would
/// violate key separation, hence the derived subkey.
pub struct Cipher {
    aead: XChaCha20Poly1305,
    index_key: Zeroizing<[u8; 32]>,
}

impl Cipher {
    /// Build a cipher directly from a 32-byte key.
    pub fn new(key: &[u8; 32]) -> Self {
        Self {
            aead: XChaCha20Poly1305::new(Key::from_slice(key)),
            index_key: derive_subkey(key, b"nucleus/index-mac/v1"),
        }
    }

    /// Deterministic, keyed token for a lookup key, as lowercase hex of
    /// `HMAC-SHA256(index_key, data)`. Equal inputs map to equal tokens (so exact
    /// lookups still work), but without the key the token is opaque and a guessed
    /// value cannot be confirmed offline.
    pub fn index_token(&self, data: &str) -> String {
        let mut mac = <HmacSha256 as Mac>::new_from_slice(self.index_key.as_slice())
            .expect("HMAC accepts any key length");
        mac.update(data.as_bytes());
        let bytes = mac.finalize().into_bytes();
        let mut s = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            use std::fmt::Write as _;
            let _ = write!(s, "{b:02x}");
        }
        s
    }

    /// Derive a key from a passphrase/salt (Argon2id) and build a cipher.
    pub fn from_passphrase(passphrase: &str, salt: &[u8], p: KdfParams) -> Result<Self> {
        let key = derive_key(passphrase.as_bytes(), salt, p)?;
        Ok(Self::new(&key))
    }

    /// Encrypt `plaintext`. The output is `nonce (24 B) || ciphertext+tag`, so it
    /// is self-describing and can be stored as an opaque blob.
    pub fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let nonce_bytes = random_bytes(NONCE_LEN);
        let nonce = XNonce::from_slice(&nonce_bytes);
        let ct = self
            .aead
            .encrypt(nonce, plaintext)
            .map_err(|e| NucleusError::crypto(format!("encrypt: {e}")))?;
        let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ct);
        Ok(out)
    }

    /// Decrypt a blob produced by [`seal`](Cipher::seal). A wrong key or tampered
    /// ciphertext fails the Poly1305 authentication check and returns an error.
    pub fn open(&self, blob: &[u8]) -> Result<Vec<u8>> {
        if blob.len() < NONCE_LEN {
            return Err(NucleusError::crypto("ciphertext too short"));
        }
        let (nonce_bytes, ct) = blob.split_at(NONCE_LEN);
        let nonce = XNonce::from_slice(nonce_bytes);
        self.aead.decrypt(nonce, ct).map_err(|_| {
            NucleusError::crypto("decryption failed (wrong passphrase or corrupt data)")
        })
    }
}

/// Load (creating on first use) the 32-byte **machine key** stored at `keyfile`.
///
/// The file contents are protected by the OS so a stolen disk does not also hand
/// over the key:
/// - **Windows:** DPAPI (`CryptProtectData`, current-user scope) — the key file is
///   useless on another machine/user account.
/// - **Unix:** the file is written `0600` (owner-only).
///
/// **Losing this file makes a machine-key database unrecoverable** — for portable
/// or backed-up protection, use a passphrase instead.
///
/// Creation is **atomic** (`create_new`, i.e. `O_EXCL`): if two processes race to
/// create the same key file, exactly one wins and the other reads the winner's
/// key, so they never end up with mismatched keys.
pub fn load_or_create_machine_key(keyfile: &Path) -> Result<Zeroizing<[u8; 32]>> {
    if let Some(k) = try_read_key(keyfile)? {
        return Ok(k);
    }
    if let Some(parent) = keyfile.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut key = Zeroizing::new([0u8; 32]);
    rand::rng().fill_bytes(key.as_mut_slice());
    let wrapped = machine::wrap(key.as_slice())?;
    match machine::write_new(keyfile, &wrapped) {
        Ok(()) => Ok(key),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // Lost the creation race: read the key the winner just wrote. It may be
            // mid-write for a moment, so retry briefly.
            for _ in 0..50 {
                if let Some(k) = try_read_key(keyfile)? {
                    return Ok(k);
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(NucleusError::crypto(
                "machine key file was created concurrently but could not be read",
            ))
        }
        Err(e) => Err(e.into()),
    }
}

/// Read and unwrap the machine key, returning `None` if the file is absent or
/// momentarily empty (a concurrent creation still flushing).
fn try_read_key(keyfile: &Path) -> Result<Option<Zeroizing<[u8; 32]>>> {
    let wrapped = match std::fs::read(keyfile) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if wrapped.is_empty() {
        return Ok(None);
    }
    let raw = Zeroizing::new(machine::unwrap(&wrapped)?);
    let arr: [u8; 32] = raw
        .as_slice()
        .try_into()
        .map_err(|_| NucleusError::crypto("machine key file is corrupt"))?;
    Ok(Some(Zeroizing::new(arr)))
}

/// OS-specific protection for the machine key file.
#[cfg(windows)]
mod machine {
    use std::path::Path;
    use std::ptr;

    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB,
    };

    use crate::error::NucleusError;
    use crate::Result;

    pub fn wrap(data: &[u8]) -> Result<Vec<u8>> {
        dpapi(data, true)
    }

    pub fn unwrap(data: &[u8]) -> Result<Vec<u8>> {
        dpapi(data, false)
    }

    pub fn write_new(path: &Path, data: &[u8]) -> std::io::Result<()> {
        use std::io::Write;
        // `create_new` fails if the file exists (atomic claim). The bytes are
        // already DPAPI-encrypted, so a plain write is enough.
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        f.write_all(data)
    }

    fn dpapi(data: &[u8], protect: bool) -> Result<Vec<u8>> {
        // SAFETY: the in-blob points at `data` for the duration of the call; the
        // out-blob is allocated by the OS and freed with `LocalFree` after copying.
        unsafe {
            let in_blob = CRYPT_INTEGER_BLOB {
                cbData: data.len() as u32,
                pbData: data.as_ptr() as *mut u8,
            };
            let mut out = CRYPT_INTEGER_BLOB {
                cbData: 0,
                pbData: ptr::null_mut(),
            };
            let ok = if protect {
                CryptProtectData(
                    &in_blob,
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    0,
                    &mut out,
                )
            } else {
                CryptUnprotectData(
                    &in_blob,
                    ptr::null_mut(),
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    0,
                    &mut out,
                )
            };
            if ok == 0 {
                return Err(NucleusError::crypto(if protect {
                    "DPAPI CryptProtectData failed"
                } else {
                    "DPAPI CryptUnprotectData failed (key bound to another user or machine?)"
                }));
            }
            let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
            LocalFree(out.pbData as *mut _);
            Ok(bytes)
        }
    }
}

/// OS-specific protection for the machine key file (non-Windows).
#[cfg(not(windows))]
mod machine {
    use std::path::Path;

    use crate::Result;

    pub fn wrap(data: &[u8]) -> Result<Vec<u8>> {
        Ok(data.to_vec())
    }

    pub fn unwrap(data: &[u8]) -> Result<Vec<u8>> {
        Ok(data.to_vec())
    }

    pub fn write_new(path: &Path, data: &[u8]) -> std::io::Result<()> {
        use std::io::Write;
        // `create_new` fails if the file exists (atomic claim).
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)?;
            f.write_all(data)
        }
        #[cfg(not(unix))]
        {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?;
            f.write_all(data)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let salt = random_bytes(SALT_LEN);
        let c = Cipher::from_passphrase("correct horse", &salt, KdfParams::DEFAULT).unwrap();
        let msg = b"el arrendador podra rescindir el contrato";
        let blob = c.seal(msg).unwrap();
        assert_ne!(&blob[NONCE_LEN..], msg); // actually encrypted
        assert_eq!(c.open(&blob).unwrap(), msg);
    }

    #[test]
    fn distinct_nonces_per_seal() {
        let c = Cipher::new(&[7u8; 32]);
        let a = c.seal(b"same").unwrap();
        let b = c.seal(b"same").unwrap();
        assert_ne!(a, b); // random nonce => different ciphertext
    }

    #[test]
    fn wrong_passphrase_fails() {
        let salt = random_bytes(SALT_LEN);
        let good = Cipher::from_passphrase("right", &salt, KdfParams::DEFAULT).unwrap();
        let blob = good.seal(b"secret").unwrap();
        let bad = Cipher::from_passphrase("wrong", &salt, KdfParams::DEFAULT).unwrap();
        assert!(bad.open(&blob).is_err());
    }

    #[test]
    fn tamper_detected() {
        let c = Cipher::new(&[3u8; 32]);
        let mut blob = c.seal(b"secret").unwrap();
        let last = blob.len() - 1;
        blob[last] ^= 0x01;
        assert!(c.open(&blob).is_err());
    }

    #[test]
    fn kdf_params_roundtrip() {
        let p = KdfParams {
            m_cost: 12345,
            t_cost: 3,
            p_cost: 2,
        };
        assert_eq!(KdfParams::from_bytes(&p.to_bytes()).unwrap(), p);
    }
}
