use super::*;
use crate::store::MemorySecretStore;

fn open(dir: &std::path::Path, secrets: &MemorySecretStore) -> Result<ClipFerry, Error> {
    ClipFerry::open(
        Config {
            data_dir: dir.to_path_buf(),
        },
        Box::new(secrets.clone()),
    )
}

fn setup() -> (tempfile::TempDir, MemorySecretStore) {
    (tempfile::tempdir().unwrap(), MemorySecretStore::default())
}

fn prepared() -> (tempfile::TempDir, MemorySecretStore) {
    let (dir, secrets) = setup();
    let app = open(dir.path(), &secrets).unwrap();
    app.init_device("laptop-secret-name").unwrap();
    app.create_group().unwrap();
    (dir, secrets)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn fresh_profile_is_uninitialized() {
    let (dir, secrets) = setup();
    let app = open(dir.path(), &secrets).unwrap();
    assert!(!app.is_initialized());
    assert!(matches!(app.status(), Err(Error::NotInitialized)));
    assert!(app.connection().is_none());
    assert!(app.devices().is_empty());
}

#[test]
fn init_then_create_group() {
    let (dir, secrets) = setup();
    let app = open(dir.path(), &secrets).unwrap();
    app.init_device("laptop").unwrap();
    let st = app.status().unwrap();
    assert_eq!(st.device_name, "laptop");
    assert_eq!(st.epoch, None);
    assert_eq!(st.member_count, 0);

    app.create_group().unwrap();
    let st = app.status().unwrap();
    assert_eq!(st.epoch, Some(1));
    assert_eq!(st.member_count, 1);

    let devices = app.devices();
    assert_eq!(devices.len(), 1);
    assert!(devices[0].is_self);
    assert_eq!(devices[0].name, "laptop");
    assert_eq!(devices[0].device_id, st.device_id);
}

#[test]
fn reopen_keeps_device_id_and_channel() {
    let (dir, secrets) = setup();
    let first = open(dir.path(), &secrets).unwrap();
    first.init_device("laptop").unwrap();
    first.create_group().unwrap();
    let status = first.status().unwrap();
    let conn = first.connection().unwrap();
    drop(first);

    let second = open(dir.path(), &secrets).unwrap();
    assert_eq!(second.status().unwrap(), status);
    let again = second.connection().unwrap();
    assert_eq!(again.epoch, 1);
    assert_eq!(again.channel_id, conn.channel_id);
    assert_eq!(again.device_tag, conn.device_tag);
}

#[test]
fn reopen_before_group_keeps_device_id() {
    let (dir, secrets) = setup();
    let first = open(dir.path(), &secrets).unwrap();
    first.init_device("laptop").unwrap();
    let id = first.status().unwrap().device_id;
    drop(first);
    let second = open(dir.path(), &secrets).unwrap();
    assert_eq!(second.status().unwrap().device_id, id);
    assert!(second.connection().is_none());
}

#[test]
fn repeated_calls_are_rejected() {
    let (dir, secrets) = setup();
    let app = open(dir.path(), &secrets).unwrap();
    assert!(matches!(app.create_group(), Err(Error::NotInitialized)));
    app.init_device("a").unwrap();
    assert!(matches!(
        app.init_device("b"),
        Err(Error::AlreadyInitialized)
    ));
    app.create_group().unwrap();
    assert!(matches!(app.create_group(), Err(Error::GroupExists)));
}

#[test]
fn state_file_is_encrypted() {
    let (dir, _secrets) = prepared();
    let bytes = std::fs::read(dir.path().join("state.bin")).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("laptop-secret-name"));
}

#[test]
fn tampered_state_file_fails_to_open() {
    let (dir, secrets) = prepared();
    let path = dir.path().join("state.bin");
    let original = std::fs::read(&path).unwrap();

    // 모든 바이트를 하나씩 바꿔 본다: 머리말, nonce, 암호문, 태그 어디든 거부해야 한다.
    for i in 0..original.len() {
        let mut bad = original.clone();
        bad[i] ^= 0x01;
        std::fs::write(&path, &bad).unwrap();
        match open(dir.path(), &secrets) {
            Err(Error::StateCorrupt) => {}
            Err(other) => panic!("byte {i}: 다른 오류 {other}"),
            Ok(_) => panic!("byte {i}: 변조된 파일이 열림"),
        }
    }
}

#[test]
fn truncated_or_empty_state_file_fails_to_open() {
    let (dir, secrets) = prepared();
    let path = dir.path().join("state.bin");
    let original = std::fs::read(&path).unwrap();
    for len in [0, 1, 4, 5, 20, original.len() - 1] {
        std::fs::write(&path, &original[..len]).unwrap();
        assert!(
            matches!(open(dir.path(), &secrets), Err(Error::StateCorrupt)),
            "len {len}"
        );
    }
}

#[test]
fn error_messages_do_not_expose_contents() {
    let (dir, secrets) = prepared();
    let path = dir.path().join("state.bin");
    let mut bad = std::fs::read(&path).unwrap();
    let last = bad.len() - 1;
    bad[last] ^= 1;
    std::fs::write(&path, &bad).unwrap();

    let Err(err) = open(dir.path(), &secrets) else {
        panic!("변조된 파일이 열림");
    };
    for text in [err.to_string(), format!("{err:?}")] {
        assert!(!text.contains("laptop-secret-name"), "{text}");
        assert!(!text.contains(dir.path().to_str().unwrap()), "{text}");
    }
}

#[test]
fn wrong_state_key_fails_to_open() {
    let (dir, secrets) = prepared();
    secrets
        .set(KEY_STATE, StateKey::generate().as_bytes())
        .unwrap();
    assert!(matches!(
        open(dir.path(), &secrets),
        Err(Error::StateCorrupt)
    ));
}

#[test]
fn missing_pieces_are_reported_as_inconsistent() {
    // 상태 파일만 남음
    let (dir, _secrets) = prepared();
    assert!(matches!(
        open(dir.path(), &MemorySecretStore::default()),
        Err(Error::StateInconsistent)
    ));

    // 상태 파일이 사라짐
    let (dir, secrets) = prepared();
    std::fs::remove_file(dir.path().join("state.bin")).unwrap();
    assert!(matches!(
        open(dir.path(), &secrets),
        Err(Error::StateInconsistent)
    ));

    // 그룹 키만 사라짐
    let (dir, secrets) = prepared();
    secrets.delete(&group_key_name(1)).unwrap();
    assert!(matches!(
        open(dir.path(), &secrets),
        Err(Error::StateInconsistent)
    ));

    // 한쪽 비밀만 남음
    let (dir, secrets) = prepared();
    secrets.delete(KEY_STATE).unwrap();
    assert!(matches!(
        open(dir.path(), &secrets),
        Err(Error::StateInconsistent)
    ));
}

#[test]
fn status_debug_has_no_secrets() {
    let (dir, secrets) = prepared();
    let app = open(dir.path(), &secrets).unwrap();
    let shown = format!("{:?}", app.status().unwrap());
    let device_hex = hex(&secrets.get(KEY_DEVICE).unwrap());
    let group_hex = hex(&secrets.get(&group_key_name(1)).unwrap());
    assert!(!shown.contains(&device_hex[..16]));
    assert!(!shown.contains(&group_hex[..16]));
}
