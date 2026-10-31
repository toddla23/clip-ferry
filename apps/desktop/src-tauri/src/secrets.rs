//! 플랫폼 보안 저장소(`keyring`)를 쓰는 `SecretStore`.
//! Windows는 Credential Manager, macOS는 Keychain에 저장한다.

use clip_ferry_core::store::{SecretStore, StoreError};

const SERVICE: &str = "clip-ferry";

pub struct KeyringStore {
    service: String,
}

impl KeyringStore {
    pub fn new() -> Self {
        Self::with_service(SERVICE)
    }

    pub fn with_service(service: &str) -> Self {
        Self {
            service: service.to_string(),
        }
    }

    fn entry(&self, key: &str) -> Result<keyring::Entry, StoreError> {
        keyring::Entry::new(&self.service, key).map_err(backend)
    }
}

impl Default for KeyringStore {
    fn default() -> Self {
        Self::new()
    }
}

/// 오류 문장에 값이 섞이지 않도록 종류만 남긴다.
fn backend(e: keyring::Error) -> StoreError {
    let kind = match e {
        keyring::Error::NoEntry => "no entry",
        keyring::Error::NoStorageAccess(_) => "no storage access",
        keyring::Error::PlatformFailure(_) => "platform failure",
        keyring::Error::TooLong(..) => "too long",
        keyring::Error::BadEncoding(_) => "bad encoding",
        _ => "unknown",
    };
    StoreError::Backend(kind.to_string())
}

impl SecretStore for KeyringStore {
    fn get(&self, key: &str) -> Option<Vec<u8>> {
        self.entry(key).ok()?.get_secret().ok()
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), StoreError> {
        self.entry(key)?.set_secret(value).map_err(backend)
    }

    fn delete(&self, key: &str) -> Result<(), StoreError> {
        match self.entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(backend(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 실제 OS 보안 저장소를 쓰므로 기본으로는 돌리지 않는다.
    /// 로컬에서: `cargo test -p clip-ferry-desktop -- --ignored`
    #[test]
    #[ignore = "OS 보안 저장소를 사용"]
    fn round_trip_in_os_keystore() {
        let store = KeyringStore::with_service("clip-ferry-test");
        let key = "group_key/1";
        let value = [7u8; 32];

        store.set(key, &value).unwrap();
        assert_eq!(store.get(key).as_deref(), Some(&value[..]));

        store.set(key, &[9u8; 64]).unwrap();
        assert_eq!(store.get(key).as_deref(), Some(&[9u8; 64][..]));

        store.delete(key).unwrap();
        assert_eq!(store.get(key), None);
        // 없는 항목을 지워도 오류가 아니다.
        store.delete(key).unwrap();
    }
}
