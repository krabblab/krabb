use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use ring::aead::{self, Aad, LessSafeKey, Nonce, UnboundKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(super) type Secrets = BTreeMap<String, String>;
type Error = Box<dyn std::error::Error + Send + Sync>;
const FORMAT: &str = "krabb.credentials.v1";
const LIMIT: usize = 1024 * 1024;
const FILE_LIMIT: u64 = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Backend {
    Keyring,
    File,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    format: String,
    backend: Backend,
    key_id: String,
    nonce: String,
    ciphertext: String,
}

type Loaded = (Secrets, Option<(Envelope, Vec<u8>)>);

trait Keyring {
    fn available(&self) -> bool;
    fn get(&self, id: &str) -> Result<Vec<u8>, Error>;
    fn set(&self, id: &str, key: &[u8]) -> Result<(), Error>;
}

struct Native;

#[cfg(not(test))]
impl Keyring for Native {
    fn available(&self) -> bool {
        keyring::Entry::store_status().is_ok()
    }

    fn get(&self, id: &str) -> Result<Vec<u8>, Error> {
        keyring::Entry::new("it.airscript.krabb", &format!("credentials:{id}"))
            .and_then(|entry| entry.get_secret())
            .map_err(|_| "Credential keyring is locked, missing, or unavailable.".into())
    }

    fn set(&self, id: &str, key: &[u8]) -> Result<(), Error> {
        keyring::Entry::new("it.airscript.krabb", &format!("credentials:{id}"))
            .and_then(|entry| entry.set_secret(key))
            .map_err(|_| {
                "Could not save the credential key in the keyring; unlock it and retry.".into()
            })
    }
}

// Unit tests never access a developer's OS keyring. Backend behavior uses injected fixtures.
#[cfg(test)]
impl Keyring for Native {
    fn available(&self) -> bool {
        false
    }

    fn get(&self, _: &str) -> Result<Vec<u8>, Error> {
        Err("Test keyring unavailable.".into())
    }

    fn set(&self, _: &str, _: &[u8]) -> Result<(), Error> {
        Err("Test keyring unavailable.".into())
    }
}

pub(super) fn path(root: &Path) -> PathBuf {
    std::env::var_os("KRABB_CREDENTIALS")
        .filter(|value| !value.is_empty())
        .map_or_else(|| root.join("credentials.json"), PathBuf::from)
}

pub(super) fn key_path(root: &Path) -> PathBuf {
    std::env::var_os("KRABB_CREDENTIALS_KEY")
        .filter(|value| !value.is_empty())
        .map_or_else(|| root.join("credentials.key"), PathBuf::from)
}

pub(super) fn preference() -> Result<Option<bool>, Error> {
    preference_from(std::env::var("KRABB_KEYRING").ok().as_deref())
}

pub(super) fn preference_from(value: Option<&str>) -> Result<Option<bool>, Error> {
    match value {
        None | Some("auto") => Ok(None),
        Some("0") => Ok(Some(false)),
        Some("1") => Ok(Some(true)),
        _ => Err("KRABB_KEYRING must be auto, 1 (required), or 0 (encrypted file key).".into()),
    }
}

pub(super) fn read(path: &Path, key_path: &Path) -> Result<Secrets, Error> {
    read_with(path, key_path, &Native).map(|(values, _)| values)
}

fn read_with(path: &Path, key_path: &Path, keyring: &impl Keyring) -> Result<Loaded, Error> {
    let Some(bytes) = krabb_file::load(path, FILE_LIMIT)? else {
        return Ok((Secrets::new(), None));
    };

    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| "Credential file must contain a JSON object of string values.")?;

    if value.get("format").is_none() {
        if bytes.len() > LIMIT {
            return Err("Credential file exceeds the size limit.".into());
        }

        let values = serde_json::from_value(value)
            .map_err(|_| "Credential file must contain a JSON object of string values.")?;

        return Ok((values, None));
    }

    let envelope: Envelope =
        serde_json::from_value(value).map_err(|_| "Invalid encrypted credential file.")?;

    if envelope.format != FORMAT
        || envelope.key_id.len() != 64
        || !envelope.key_id.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("Unsupported encrypted credential format.".into());
    }

    let key = match envelope.backend {
        Backend::Keyring => keyring.get(&envelope.key_id)?,
        Backend::File => load_key(key_path)?,
    };

    let values = decrypt(&envelope, &key)?;

    Ok((values, Some((envelope, key))))
}

fn load_key(path: &Path) -> Result<Vec<u8>, Error> {
    let key = krabb_file::load(path, 32)?
        .ok_or("Credential encryption key is missing; restore it from backup.")?;

    validate_key(key)
}

fn validate_key(key: Vec<u8>) -> Result<Vec<u8>, Error> {
    if key.len() != 32 {
        return Err("Credential encryption key must contain exactly 32 bytes.".into());
    }

    Ok(key)
}

fn random<const N: usize>() -> Result<[u8; N], Error> {
    let mut value = [0; N];

    getrandom::fill(&mut value).map_err(|_| "Secure random generation failed.")?;
    Ok(value)
}

fn id(key: &[u8]) -> String {
    super::hex_digest(Sha256::digest(key))
}

fn cipher(key: &[u8]) -> Result<LessSafeKey, Error> {
    UnboundKey::new(&aead::AES_256_GCM, key)
        .map(LessSafeKey::new)
        .map_err(|_| "Invalid credential encryption key.".into())
}

fn aad(envelope: &Envelope) -> Vec<u8> {
    let backend = match envelope.backend {
        Backend::Keyring => "keyring",
        Backend::File => "file",
    };

    format!("{}:{backend}:{}", envelope.format, envelope.key_id).into_bytes()
}

fn encrypt(values: &Secrets, key: &[u8], backend: Backend) -> Result<Envelope, Error> {
    let mut bytes = serde_json::to_vec(values)?;

    if bytes.len() > LIMIT {
        return Err("Credential file exceeds the size limit.".into());
    }

    let nonce = random::<12>()?;

    let mut envelope = Envelope {
        format: FORMAT.into(),
        backend,
        key_id: id(key),
        nonce: STANDARD.encode(nonce),
        ciphertext: String::new(),
    };

    cipher(key)?
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(aad(&envelope)),
            &mut bytes,
        )
        .map_err(|_| "Credential encryption failed.")?;

    envelope.ciphertext = STANDARD.encode(bytes);
    Ok(envelope)
}

fn decrypt(envelope: &Envelope, key: &[u8]) -> Result<Secrets, Error> {
    if id(key) != envelope.key_id {
        return Err("Credential encryption key does not match this file.".into());
    }

    let nonce: [u8; 12] = STANDARD
        .decode(&envelope.nonce)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or("Invalid credential nonce.")?;

    let mut bytes =
        STANDARD.decode(&envelope.ciphertext).map_err(|_| "Invalid encrypted credentials.")?;

    if bytes.len() > LIMIT + aead::AES_256_GCM.tag_len() {
        return Err("Encrypted credentials exceed the size limit.".into());
    }

    let plaintext = cipher(key)?
        .open_in_place(Nonce::assume_unique_for_key(nonce), Aad::from(aad(envelope)), &mut bytes)
        .map_err(
            |_| "Credential authentication failed; the file is damaged or the key is wrong.",
        )?;

    serde_json::from_slice(plaintext).map_err(|_| "Invalid credential contents.".into())
}

pub(super) fn save(
    path: &Path,
    key_path: &Path,
    values: Secrets,
    preference: Option<bool>,
) -> Result<&'static str, Error> {
    save_with(path, key_path, values, preference, &Native)
}

fn file_key(path: &Path, key_path: &Path) -> Result<Vec<u8>, Error> {
    let parent = parent(key_path);
    std::fs::create_dir_all(parent)?;
    let name = key_path.file_name().ok_or("A key filename is required.")?.to_string_lossy();

    if parent.canonicalize()?.join(key_path.file_name().unwrap())
        == self::parent(path)
            .canonicalize()?
            .join(path.file_name().ok_or("A credential filename is required.")?)
    {
        return Err("Credentials and their encryption key must use different files.".into());
    }

    let _lock = super::lock_at_timeout(parent, &format!(".{name}.lock"), Duration::from_secs(5))?;

    if let Some(key) = krabb_file::load(key_path, 32)? {
        return validate_key(key);
    }

    let key = random::<32>()?;
    super::save_file(key_path, key)?;
    Ok(key.to_vec())
}

fn parent(path: &Path) -> &Path {
    path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."))
}

fn save_with(
    path: &Path,
    key_path: &Path,
    values: Secrets,
    preference: Option<bool>,
    keyring: &impl Keyring,
) -> Result<&'static str, Error> {
    let name = path.file_name().ok_or("A credential filename is required.")?.to_string_lossy();
    let _lock =
        super::lock_at_timeout(parent(path), &format!(".{name}.lock"), Duration::from_secs(5))?;
    let (mut existing, previous) = read_with(path, key_path, keyring)?;
    existing.extend(values);

    let backend = match (preference, &previous) {
        (Some(true), _) => Backend::Keyring,
        (Some(false), _) => Backend::File,
        (None, Some((envelope, _))) => envelope.backend,
        (None, None) if keyring.available() => Backend::Keyring,
        _ => Backend::File,
    };

    let key = if let Some((_, key)) = previous.filter(|(envelope, _)| envelope.backend == backend) {
        key
    } else if backend == Backend::Keyring {
        if !keyring.available() {
            return Err("Keyring was requested but is unavailable.".into());
        }

        let key = random::<32>()?;
        keyring.set(&id(&key), &key)?;
        // Verify the persisted key before replacing the credential file.
        if keyring.get(&id(&key))? != key {
            return Err("Keyring did not retain the credential key.".into());
        }

        key.to_vec()
    } else {
        file_key(path, key_path)?
    };

    let envelope = encrypt(&existing, &key, backend)?;
    super::save_file(path, serde_json::to_vec_pretty(&envelope)?)?;
    Ok(if backend == Backend::Keyring { "keyring" } else { "file" })
}

pub(super) fn resolve(names: &[String], root: &Path) -> Result<Secrets, Error> {
    resolve_with(names, |name| std::env::var(name).ok(), || read(&path(root), &key_path(root)))
}

fn resolve_with(
    names: &[String],
    mut environment: impl FnMut(&str) -> Option<String>,
    load: impl FnOnce() -> Result<Secrets, Error>,
) -> Result<Secrets, Error> {
    let mut values = Secrets::new();
    let mut missing = Vec::new();

    for name in names.iter().filter(|name| name.starts_with("KRABB_")) {
        if let Some(value) = environment(name).filter(|value| !value.trim().is_empty()) {
            values.insert(name.clone(), value);
        } else {
            missing.push(name);
        }
    }

    if !missing.is_empty() {
        // One authenticated snapshot prevents mixing credentials from concurrent rotations.
        let mut stored = load()?;

        for name in missing {
            if let Some(value) = stored.remove(name).filter(|value| !value.trim().is_empty()) {
                values.insert(name.clone(), value);
            }
        }
    }

    Ok(values)
}

#[cfg(test)]
mod tests;
