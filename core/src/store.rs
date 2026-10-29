//! 로컬 저장: 비밀 저장소 트레이트와 암호화된 상태 파일(`state.bin`).

use std::collections::HashMap;
use std::io::Write as _;
use std::path::Path;
use std::sync::{Arc, Mutex};

use prost::Message;

use crate::crypto::{StateKey, NONCE_LEN};
use crate::pb;

/// 상태 파일 형식 버전.
const FORMAT_VERSION: u8 = 1;
const MAGIC: &[u8; 4] = b"CFST";
const HEADER_LEN: usize = MAGIC.len() + 1;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("비밀 저장소 오류: {0}")]
    Backend(String),
}

/// 기기 개인키·그룹 키 같은 비밀을 두는 곳. 셸이 플랫폼 보안 저장소로 구현한다.
pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Option<Vec<u8>>;
    fn set(&self, key: &str, value: &[u8]) -> Result<(), StoreError>;
    fn delete(&self, key: &str) -> Result<(), StoreError>;
}

/// 메모리에만 두는 저장소. 테스트용이며 `clone`하면 같은 내용을 공유한다.
#[derive(Clone, Default)]
pub struct MemorySecretStore {
    map: Arc<Mutex<HashMap<String, Vec<u8>>>>,
}

impl SecretStore for MemorySecretStore {
    fn get(&self, key: &str) -> Option<Vec<u8>> {
        self.map.lock().unwrap().get(key).cloned()
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), StoreError> {
        self.map
            .lock()
            .unwrap()
            .insert(key.to_string(), value.to_vec());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), StoreError> {
        self.map.lock().unwrap().remove(key);
        Ok(())
    }
}

/// 상태 파일을 읽을 수 없는 이유. 내용은 담지 않는다.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StateFileError {
    Io,
    /// 형식이 맞지 않거나, 복호화·인증에 실패했거나, 지원하지 않는 버전.
    Corrupt,
}

fn aad() -> [u8; HEADER_LEN] {
    let mut a = [0u8; HEADER_LEN];
    a[..MAGIC.len()].copy_from_slice(MAGIC);
    a[MAGIC.len()] = FORMAT_VERSION;
    a
}

/// 파일이 없으면 `Ok(None)`.
pub(crate) fn read_state(
    path: &Path,
    key: &StateKey,
) -> Result<Option<pb::LocalState>, StateFileError> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(StateFileError::Io),
    };
    if bytes.len() < HEADER_LEN + NONCE_LEN || bytes[..HEADER_LEN] != aad() {
        return Err(StateFileError::Corrupt);
    }
    let nonce: [u8; NONCE_LEN] = bytes[HEADER_LEN..HEADER_LEN + NONCE_LEN]
        .try_into()
        .map_err(|_| StateFileError::Corrupt)?;
    let plain = key
        .open(&nonce, &bytes[HEADER_LEN + NONCE_LEN..], &aad())
        .map_err(|_| StateFileError::Corrupt)?;
    let state = pb::LocalState::decode(plain.as_slice()).map_err(|_| StateFileError::Corrupt)?;
    if state.version != u32::from(FORMAT_VERSION) {
        return Err(StateFileError::Corrupt);
    }
    Ok(Some(state))
}

/// 임시 파일에 쓴 뒤 이름을 바꿔 한 번에 교체한다.
pub(crate) fn write_state(
    path: &Path,
    key: &StateKey,
    state: &pb::LocalState,
) -> Result<(), StateFileError> {
    let sealed = key
        .seal(&state.encode_to_vec(), &aad())
        .map_err(|_| StateFileError::Corrupt)?;
    let mut out = Vec::with_capacity(HEADER_LEN + NONCE_LEN + sealed.ciphertext.len());
    out.extend_from_slice(&aad());
    out.extend_from_slice(&sealed.nonce);
    out.extend_from_slice(&sealed.ciphertext);

    let io = |_| StateFileError::Io;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let tmp = path.with_extension("bin.tmp");
    let mut file = std::fs::File::create(&tmp).map_err(io)?;
    file.write_all(&out).map_err(io)?;
    file.sync_all().map_err(io)?;
    drop(file);
    std::fs::rename(&tmp, path).map_err(io)?;
    Ok(())
}
