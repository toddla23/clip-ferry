use std::path::PathBuf;

use clip_ferry_core::store::{SecretStore, StoreError};

/// 비밀을 평문 파일로 저장하는 **개발 전용** 저장소.
/// 실제 앱은 플랫폼 보안 저장소를 쓴다.
pub struct FileSecretStore {
    dir: PathBuf,
}

impl FileSecretStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn path(&self, key: &str) -> PathBuf {
        // 키에 들어 있는 '/'는 파일 이름에 쓸 수 없다.
        self.dir.join(format!("{}.bin", key.replace('/', "__")))
    }
}

fn backend(e: std::io::Error) -> StoreError {
    StoreError::Backend(e.kind().to_string())
}

impl SecretStore for FileSecretStore {
    fn get(&self, key: &str) -> Option<Vec<u8>> {
        std::fs::read(self.path(key)).ok()
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), StoreError> {
        std::fs::create_dir_all(&self.dir).map_err(backend)?;
        let path = self.path(key);
        std::fs::write(&path, value).map_err(backend)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .map_err(backend)?;
        }
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), StoreError> {
        match std::fs::remove_file(self.path(key)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(backend(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_delete() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileSecretStore::new(dir.path().join("secrets"));
        assert_eq!(store.get("group_key/1"), None);
        store.set("group_key/1", b"abc").unwrap();
        assert_eq!(store.get("group_key/1").unwrap(), b"abc");
        store.delete("group_key/1").unwrap();
        assert_eq!(store.get("group_key/1"), None);
        store.delete("group_key/1").unwrap();
    }
}
