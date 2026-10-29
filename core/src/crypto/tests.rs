use super::group::hkdf_expand;
use super::*;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn test_group() -> GroupKey {
    GroupKey::from_bytes([0x42; 32])
}

#[test]
fn seal_open_roundtrip() {
    let key = test_group().content_key(1);
    let sealed = seal(&key, b"hello", b"aad").unwrap();
    assert_eq!(
        open(&key, &sealed.nonce, &sealed.ciphertext, b"aad").unwrap(),
        b"hello"
    );
}

#[test]
fn seal_open_empty_plaintext() {
    let key = test_group().content_key(1);
    let sealed = seal(&key, b"", b"").unwrap();
    assert!(open(&key, &sealed.nonce, &sealed.ciphertext, b"")
        .unwrap()
        .is_empty());
}

#[test]
fn wrap_unwrap_roundtrip() {
    let dev = DeviceKeys::generate();
    let w = wrap_for(&dev.dh_public(), &dev.device_id(), b"group secret").unwrap();
    assert_eq!(unwrap(&dev, &w).unwrap(), b"group secret");
}

#[test]
fn device_keys_serialize_roundtrip() {
    let dev = DeviceKeys::generate();
    let restored = DeviceKeys::from_bytes(&dev.to_bytes()[..]).unwrap();
    assert_eq!(dev.sign_public(), restored.sign_public());
    assert_eq!(dev.dh_public(), restored.dh_public());
    assert_eq!(dev.device_id(), restored.device_id());
    let w = wrap_for(&dev.dh_public(), &dev.device_id(), b"x").unwrap();
    assert_eq!(unwrap(&restored, &w).unwrap(), b"x");
}

#[test]
fn device_keys_from_bytes_wrong_length() {
    assert_eq!(
        DeviceKeys::from_bytes(&[0u8; 63]).unwrap_err(),
        CryptoError::InvalidLength
    );
    assert_eq!(
        DeviceKeys::from_bytes(&[0u8; 65]).unwrap_err(),
        CryptoError::InvalidLength
    );
}

#[test]
fn tamper_any_bit_fails() {
    let key = test_group().content_key(1);
    let sealed = seal(&key, b"hello world", b"aad").unwrap();
    for i in 0..sealed.ciphertext.len() {
        let mut ct = sealed.ciphertext.clone();
        ct[i] ^= 0x01;
        assert_eq!(
            open(&key, &sealed.nonce, &ct, b"aad"),
            Err(CryptoError::Decrypt),
            "byte {i}"
        );
    }
    let mut nonce = sealed.nonce;
    nonce[0] ^= 0x01;
    assert_eq!(
        open(&key, &nonce, &sealed.ciphertext, b"aad"),
        Err(CryptoError::Decrypt)
    );
}

#[test]
fn wrong_key_fails() {
    let sealed = seal(&test_group().content_key(1), b"hello", b"aad").unwrap();
    let other = GroupKey::from_bytes([0x43; 32]).content_key(1);
    assert_eq!(
        open(&other, &sealed.nonce, &sealed.ciphertext, b"aad"),
        Err(CryptoError::Decrypt)
    );
    let other_epoch = test_group().content_key(2);
    assert_eq!(
        open(&other_epoch, &sealed.nonce, &sealed.ciphertext, b"aad"),
        Err(CryptoError::Decrypt)
    );
}

#[test]
fn wrong_aad_fails() {
    let key = test_group().content_key(1);
    let sealed = seal(&key, b"hello", b"aad").unwrap();
    assert_eq!(
        open(&key, &sealed.nonce, &sealed.ciphertext, b"aae"),
        Err(CryptoError::Decrypt)
    );
    assert_eq!(
        open(&key, &sealed.nonce, &sealed.ciphertext, b""),
        Err(CryptoError::Decrypt)
    );
}

#[test]
fn truncated_fails() {
    let key = test_group().content_key(1);
    let sealed = seal(&key, b"hello", b"aad").unwrap();
    for len in 0..sealed.ciphertext.len() {
        assert_eq!(
            open(&key, &sealed.nonce, &sealed.ciphertext[..len], b"aad"),
            Err(CryptoError::Decrypt),
            "len {len}"
        );
    }
}

#[test]
fn wrong_recipient_unwrap_fails() {
    let alice = DeviceKeys::generate();
    let bob = DeviceKeys::generate();
    let w = wrap_for(&alice.dh_public(), &alice.device_id(), b"secret").unwrap();
    assert_eq!(unwrap(&bob, &w), Err(CryptoError::Decrypt));
}

#[test]
fn wrap_bound_to_recipient_id() {
    // dh 공개키는 alice 것이지만 aad(device_id)가 다르면 alice도 열 수 없다.
    let alice = DeviceKeys::generate();
    let bob = DeviceKeys::generate();
    let w = wrap_for(&alice.dh_public(), &bob.device_id(), b"secret").unwrap();
    assert_eq!(unwrap(&alice, &w), Err(CryptoError::Decrypt));
}

#[test]
fn wrap_tamper_and_truncated_fail() {
    let dev = DeviceKeys::generate();
    let w = wrap_for(&dev.dh_public(), &dev.device_id(), b"secret").unwrap();

    let mut t = w.clone();
    t.ciphertext[0] ^= 0x01;
    assert_eq!(unwrap(&dev, &t), Err(CryptoError::Decrypt));

    let mut t = w.clone();
    t.eph_pub[0] ^= 0x01;
    assert_eq!(unwrap(&dev, &t), Err(CryptoError::Decrypt));

    let mut t = w.clone();
    t.ciphertext.truncate(t.ciphertext.len() - 1);
    assert_eq!(unwrap(&dev, &t), Err(CryptoError::Decrypt));
}

#[test]
fn seal_twice_differs() {
    let key = test_group().content_key(1);
    let a = seal(&key, b"same", b"aad").unwrap();
    let b = seal(&key, b"same", b"aad").unwrap();
    assert_ne!(a.nonce, b.nonce);
    assert_ne!(a.ciphertext, b.ciphertext);
}

#[test]
fn wrap_twice_differs() {
    let dev = DeviceKeys::generate();
    let a = wrap_for(&dev.dh_public(), &dev.device_id(), b"same").unwrap();
    let b = wrap_for(&dev.dh_public(), &dev.device_id(), b"same").unwrap();
    assert_ne!(a.eph_pub, b.eph_pub);
    assert_ne!(a.nonce, b.nonce);
    assert_ne!(a.ciphertext, b.ciphertext);
}

#[test]
fn derived_values_differ_by_purpose_and_epoch() {
    let g = test_group();
    let id = DeviceId([7; 16]);
    assert_ne!(g.content_key(1).as_bytes(), &g.channel_id(1).0);
    assert_ne!(g.content_key(1).as_bytes(), g.content_key(2).as_bytes());
    assert_ne!(g.channel_id(1), g.channel_id(2));
    assert_ne!(g.device_tag(1, &id), g.device_tag(2, &id));
    assert_ne!(g.device_tag(1, &id), g.device_tag(1, &DeviceId([8; 16])));
}

#[test]
fn hkdf_matches_rfc5869_case1() {
    let ikm = [0x0b; 22];
    let salt: Vec<u8> = (0x00..=0x0c).collect();
    let info: Vec<u8> = (0xf0..=0xf9).collect();
    let okm: [u8; 42] = hkdf_expand(&ikm, Some(&salt), &info);
    assert_eq!(
        hex(&okm),
        "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
    );
}

#[test]
fn derivation_test_vectors() {
    // 다른 언어·버전 구현과 비교하는 기준값. group_key = 0x42 * 32, epoch = 1, device_id = 0x07 * 16.
    let g = test_group();
    let id = DeviceId([7; 16]);
    assert_eq!(
        hex(g.content_key(1).as_bytes()),
        "0d3c0416ce723f5e8c0485be92fca1335eb51b38f2ee2f9927c00f0ff4f46b36"
    );
    assert_eq!(
        hex(&g.channel_id(1).0),
        "20e2a2beb9bb8d182f4beaf5dd8ec4815b73cf8e1b8d9706b679ad365f9ad61e"
    );
    assert_eq!(
        hex(&g.device_tag(1, &id).0),
        "bb9b7666a8a08a7a5c7dd8c2a2733ed2"
    );
}

#[test]
fn device_id_test_vector() {
    // sign_pub = 0x01 * 32
    assert_eq!(
        hex(&DeviceId::from_sign_pub(&[1; 32]).0),
        "72cd6e8422c407fb6d098690f1130b7d"
    );
}

#[test]
fn debug_redacted() {
    let g = GroupKey::from_bytes([0xab; 32]);
    let content = g.content_key(1);
    let channel = g.channel_id(1);
    let dev = DeviceKeys::generate();
    let secret_hex = hex(&dev.to_bytes()[..]);

    let outputs = [
        format!("{g:?}"),
        format!("{g:#?}"),
        format!("{content:?}"),
        format!("{channel:?}"),
        format!("{dev:?}"),
        format!("{dev:#?}"),
    ];
    for out in &outputs {
        assert!(out.contains("redacted"), "{out}");
        assert!(!out.contains("171, 171"), "{out}"); // 0xab를 10진수로 찍은 경우
        assert!(!out.to_lowercase().contains("abab"), "{out}");
        assert!(!out.contains(&secret_hex[..16]), "{out}");
        assert!(!out.contains(&secret_hex[64..80]), "{out}");
    }
    let content_hex = hex(content.as_bytes());
    assert!(!outputs[2].contains(&content_hex[..8]));
}
