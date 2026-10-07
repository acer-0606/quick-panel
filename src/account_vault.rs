//! Bounded authenticated account storage. Blocking crypto runs on `VaultWorker`.
use crate::account_fill::{Binding, FieldSelector};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{AeadInPlace, KeyInit},
};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, io::Write};
use zeroize::{Zeroize, Zeroizing};

pub const MAX_ENVELOPE_BYTES: usize = 4 * 1024 * 1024;
const HEADER: usize = 80;
const TAG: usize = 16;
const INVALID: &str = "保险箱密码错误或数据损坏";
const INVALID_DOCUMENT: &str = "账号或关联内容无效或超过上限";
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);
impl Secret {
    pub fn new(value: String) -> Self {
        Self(value)
    }
    /// Borrow only for the immediate authorized operation; never log or persist.
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}
impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountSummary {
    pub id: String,
    pub name: String,
    pub username: String,
}
impl Drop for AccountSummary {
    fn drop(&mut self) {
        self.id.zeroize();
        self.name.zeroize();
        self.username.zeroize();
    }
}
#[derive(Clone)]
pub struct AccountDraft {
    pub id: String,
    pub name: String,
    pub username: String,
    pub password: Secret,
}
impl Drop for AccountDraft {
    fn drop(&mut self) {
        self.id.zeroize();
        self.name.zeroize();
        self.username.zeroize();
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Account {
    pub id: String,
    pub name: String,
    pub username: String,
    pub password: Secret,
}
impl Drop for Account {
    fn drop(&mut self) {
        self.id.zeroize();
        self.name.zeroize();
        self.username.zeroize();
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VaultDocument {
    pub accounts: Vec<Account>,
    pub bindings: Vec<Binding>,
}
fn wipe_selector(selector: &mut FieldSelector) {
    if let Some(v) = &mut selector.automation_id {
        v.zeroize();
    }
    if let Some(v) = &mut selector.class_name {
        v.zeroize();
    }
    for a in &mut selector.anchors {
        if let Some(v) = &mut a.automation_id {
            v.zeroize();
        }
        if let Some(v) = &mut a.class_name {
            v.zeroize();
        }
        if let Some(v) = &mut a.label {
            v.zeroize();
        }
    }
}
fn wipe_binding(binding: &mut Binding) {
    binding.binding_id.zeroize();
    binding.account_id.zeroize();
    binding.app.canonical_path.zeroize();
    binding.app.file_fingerprint.zeroize();
    binding.scene_key.zeroize();
    wipe_selector(&mut binding.password);
    if let Some(v) = &mut binding.username {
        wipe_selector(v);
    }
    if let Some(v) = &mut binding.target {
        wipe_selector(&mut v.selector);
        v.expected.zeroize();
    }
}
impl Drop for VaultDocument {
    fn drop(&mut self) {
        for b in &mut self.bindings {
            wipe_binding(b);
        }
    }
}
fn text_valid(s: &str, min: usize, max: usize, bytes: usize) -> bool {
    s.len() <= bytes && (min..=max).contains(&s.chars().count()) && !s.contains(['\0', '\r', '\n'])
}
impl VaultDocument {
    pub fn validate(&self) -> Result<(), String> {
        if self.accounts.len() > 512 || self.bindings.len() > 2048 {
            return Err(INVALID_DOCUMENT.into());
        }
        let mut ids = HashSet::new();
        for a in &self.accounts {
            if !text_valid(&a.id, 1, 96, 96)
                || !ids.insert(a.id.as_str())
                || !text_valid(&a.name, 1, 96, 384)
                || !text_valid(&a.username, 0, 256, 1024)
                || !text_valid(a.password.expose(), 1, 1024, 4096)
            {
                return Err(INVALID_DOCUMENT.into());
            }
        }
        let mut bindings = HashSet::new();
        for b in &self.bindings {
            if b.validate().is_err()
                || !ids.contains(b.account_id.as_str())
                || !bindings.insert(b.binding_id.as_str())
            {
                return Err(INVALID_DOCUMENT.into());
            }
        }
        Ok(())
    }
    pub fn upsert_account(&mut self, draft: AccountDraft) -> Result<(), String> {
        let mut candidate = self.clone();
        let previous = candidate.accounts.iter().position(|a| a.id == draft.id);
        let password = if draft.password.expose().is_empty() {
            previous
                .map(|i| candidate.accounts[i].password.clone())
                .ok_or(INVALID_DOCUMENT)?
        } else {
            draft.password.clone()
        };
        let account = Account {
            id: draft.id.clone(),
            name: draft.name.clone(),
            username: draft.username.clone(),
            password,
        };
        if let Some(i) = previous {
            candidate.accounts[i] = account;
        } else {
            candidate.accounts.push(account);
        }
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }
    pub fn upsert_binding(&mut self, binding: Binding) -> Result<(), String> {
        let mut candidate = self.clone();
        if let Some(i) = candidate
            .bindings
            .iter()
            .position(|b| b.binding_id == binding.binding_id)
        {
            wipe_binding(&mut candidate.bindings[i]);
            candidate.bindings[i] = binding;
        } else {
            candidate.bindings.push(binding);
        }
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }
    pub fn delete_account(&mut self, id: &str) {
        self.accounts.retain(|a| a.id != id);
        self.bindings.retain_mut(|b| {
            if b.account_id == id {
                wipe_binding(b);
                false
            } else {
                true
            }
        });
    }
    pub fn delete_binding(&mut self, id: &str) {
        self.bindings.retain_mut(|b| {
            if b.binding_id == id {
                wipe_binding(b);
                false
            } else {
                true
            }
        });
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VaultSnapshot {
    pub revision: u64,
    pub envelope: Vec<u8>,
}
#[derive(Clone)]
pub(crate) struct VaultData {
    pub(crate) document: VaultDocument,
    key: Zeroizing<[u8; 32]>,
    salt: [u8; 16],
    pub(crate) revision: u64,
}
#[derive(Clone, Default)]
pub struct VaultSession {
    pub(crate) data: Option<VaultData>,
    epoch: u64,
}
impl VaultSession {
    /// Blocking: use only on the crypto worker. Creation is not a durable save.
    pub fn create(
        password: &Secret,
        confirmation: &Secret,
        document: VaultDocument,
    ) -> Result<Self, String> {
        document.validate()?;
        drop(serialize(&document)?);
        validate_master(password)?;
        if password.expose() != confirmation.expose() {
            return Err("两次主密码不一致".into());
        }
        let mut salt = [0; 16];
        random(&mut salt)?;
        let key = derive(password, &salt)?;
        Ok(Self {
            data: Some(VaultData {
                document,
                key,
                salt,
                revision: 0,
            }),
            epoch: 0,
        })
    }
    /// Blocking: envelope format/size/parameters are checked before any KDF.
    pub fn unlock(password: &Secret, snapshot: &VaultSnapshot) -> Result<Self, String> {
        validate_envelope(&snapshot.envelope)?;
        validate_master(password)?;
        let bytes = &snapshot.envelope;
        let salt = bytes[36..52].try_into().map_err(|_| INVALID)?;
        let key = derive(password, &salt)?;
        let mut plain = Zeroizing::new(bytes[HEADER..].to_vec());
        XChaCha20Poly1305::new((&*key).into())
            .decrypt_in_place(
                XNonce::from_slice(&bytes[52..76]),
                &bytes[..HEADER],
                &mut *plain,
            )
            .map_err(|_| INVALID)?;
        let document: VaultDocument = serde_json::from_slice(&plain).map_err(|_| INVALID)?;
        document.validate().map_err(|_| INVALID)?;
        Ok(Self {
            data: Some(VaultData {
                document,
                key,
                salt,
                revision: snapshot.revision,
            }),
            epoch: 0,
        })
    }
    pub fn encrypt(&self) -> Result<Vec<u8>, String> {
        self.encrypt_with(random)
    }
    pub(crate) fn encrypt_with(
        &self,
        mut rng: impl FnMut(&mut [u8]) -> Result<(), String>,
    ) -> Result<Vec<u8>, String> {
        let data = self.data.as_ref().ok_or("保险箱已锁定")?;
        data.document.validate()?;
        let mut plain = PlainWriter(serialize(&data.document)?);
        let mut envelope = b"QPVAULT\0".to_vec();
        for v in [1u32, 2, 19, 65536, 3, 1, 32] {
            envelope.extend(v.to_le_bytes());
        }
        envelope.extend(data.salt);
        let mut nonce = [0; 24];
        rng(&mut nonce)?;
        envelope.extend(nonce);
        envelope.extend(((plain.0.len() + TAG) as u32).to_le_bytes());
        XChaCha20Poly1305::new((&*data.key).into())
            .encrypt_in_place(XNonce::from_slice(&nonce), &envelope, &mut *plain.0)
            .map_err(|_| "保险箱加密失败")?;
        envelope.extend_from_slice(&plain.0);
        Ok(envelope)
    }
    pub fn change_master(&self, password: &Secret, confirmation: &Secret) -> Result<Self, String> {
        let old = self.data.as_ref().ok_or("保险箱已锁定")?;
        let mut next = Self::create(password, confirmation, old.document.clone())?;
        next.data.as_mut().unwrap().revision = old.revision;
        Ok(next)
    }
    pub fn lock(&mut self) {
        self.epoch = self.epoch.checked_add(1).expect("vault epoch exhausted");
        self.data = None;
    }
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn is_unlocked(&self) -> bool {
        self.data.is_some()
    }
    pub fn revision(&self) -> Option<u64> {
        self.data.as_ref().map(|d| d.revision)
    }
    pub fn document(&self) -> Result<&VaultDocument, String> {
        self.data
            .as_ref()
            .map(|d| &d.document)
            .ok_or_else(|| "保险箱已锁定".into())
    }
    pub fn summaries(&self) -> Vec<AccountSummary> {
        self.data
            .as_ref()
            .map(|d| {
                d.document
                    .accounts
                    .iter()
                    .map(|a| AccountSummary {
                        id: a.id.clone(),
                        name: a.name.clone(),
                        username: a.username.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
    pub fn credentials(&self, id: &str) -> Result<(Secret, Secret), String> {
        let a = self
            .document()?
            .accounts
            .iter()
            .find(|a| a.id == id)
            .ok_or("账号不存在")?;
        Ok((Secret::new(a.username.clone()), a.password.clone()))
    }
    pub(crate) fn candidate(&self, document: VaultDocument) -> Result<Self, String> {
        document.validate()?;
        let mut next = self.clone();
        next.data.as_mut().ok_or("保险箱已锁定")?.document = document;
        Ok(next)
    }
    pub(crate) fn install(&mut self, mut candidate: Self, revision: u64) {
        if let Some(data) = candidate.data.as_mut() {
            data.revision = revision;
        }
        self.data = candidate.data.take();
    }
}
fn serialize(document: &VaultDocument) -> Result<Zeroizing<Vec<u8>>, String> {
    let mut plain = PlainWriter(Zeroizing::new(Vec::with_capacity(
        MAX_ENVELOPE_BYTES - HEADER,
    )));
    serde_json::to_writer(&mut plain, document).map_err(|_| INVALID_DOCUMENT)?;
    Ok(plain.0)
}
struct PlainWriter(Zeroizing<Vec<u8>>);
impl Write for PlainWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > MAX_ENVELOPE_BYTES - HEADER - TAG {
            return Err(std::io::Error::other(INVALID_DOCUMENT));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn validate_master(password: &Secret) -> Result<(), String> {
    if !(12..=256).contains(&password.expose().chars().count()) || password.expose().len() > 1024 {
        Err("主密码需为12至256个字符且不超过1024字节".into())
    } else {
        Ok(())
    }
}
pub(crate) fn random(bytes: &mut [u8]) -> Result<(), String> {
    getrandom::fill(bytes).map_err(|_| "无法取得安全随机数".into())
}
fn derive(password: &Secret, salt: &[u8; 16]) -> Result<Zeroizing<[u8; 32]>, String> {
    let mut key = Zeroizing::new([0; 32]);
    let mut memory = Zeroizing::new(vec![argon2::Block::default(); 65536]);
    Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        Params::new(65536, 3, 1, Some(32)).map_err(|_| INVALID)?,
    )
    .hash_password_into_with_memory(password.expose().as_bytes(), salt, &mut *key, &mut *memory)
    .map_err(|_| INVALID)?;
    Ok(key)
}
pub fn validate_envelope(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() < HEADER + TAG || bytes.len() > MAX_ENVELOPE_BYTES || &bytes[..8] != b"QPVAULT\0"
    {
        return Err(INVALID.into());
    }
    for (i, value) in [1u32, 2, 19, 65536, 3, 1, 32].iter().enumerate() {
        if bytes[8 + i * 4..12 + i * 4] != value.to_le_bytes() {
            return Err(INVALID.into());
        }
    }
    let length = u32::from_le_bytes(bytes[76..80].try_into().map_err(|_| INVALID)?) as usize;
    if length != bytes.len() - HEADER {
        return Err(INVALID.into());
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn random_failure_never_returns_envelope() {
        let p = Secret::new("Synthetic master password".into());
        let s = VaultSession::create(&p, &p, VaultDocument::default()).unwrap();
        assert!(s.encrypt_with(|_| Err("随机源不可用".into())).is_err());
    }
}
