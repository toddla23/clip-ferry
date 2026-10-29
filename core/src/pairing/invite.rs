//! 초대: 32바이트 비밀과 그 표현(QR 문자열, 텍스트 코드).

use rand_core::{OsRng, RngCore};
use zeroize::Zeroizing;

use crate::crypto::derive32;

/// 초대 유효 시간: 10분.
pub const INVITE_TTL_MS: u64 = 10 * 60 * 1000;

const URI_PREFIX: &str = "clipferry://pair/";
const URI_VERSION: &str = "1";
/// Crockford base32 (I, L, O, U 제외).
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// 체크 기호 37개: 알파벳 32개 + `* ~ $ = U`.
const CHECK_SYMBOLS: &[u8; 37] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ*~$=U";
/// 비밀 256비트를 5비트씩 나눈 기호 수(마지막 4비트는 0으로 채움).
const DATA_SYMBOLS: usize = 52;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InviteError {
    #[error("초대 코드 길이가 올바르지 않습니다")]
    BadLength,
    #[error("초대 코드에 올바르지 않은 문자가 있습니다")]
    BadCharacter,
    #[error("초대 코드의 체크 기호가 맞지 않습니다. 오타가 있는지 확인하세요")]
    BadChecksum,
    #[error("지원하지 않는 초대 형식입니다")]
    Unsupported,
    #[error("초대 링크가 올바르지 않습니다")]
    BadUri,
}

/// 초대 비밀과 (선택) 릴레이 주소.
#[derive(Clone)]
pub struct Invite {
    secret: Zeroizing<[u8; 32]>,
    relay: Option<String>,
}

impl Invite {
    /// 무작위 비밀로 새 초대를 만든다.
    pub fn generate(relay: Option<String>) -> Self {
        let mut secret = Zeroizing::new([0u8; 32]);
        OsRng.fill_bytes(&mut secret[..]);
        Self { secret, relay }
    }

    pub fn from_secret(secret: [u8; 32], relay: Option<String>) -> Self {
        Self {
            secret: Zeroizing::new(secret),
            relay,
        }
    }

    pub fn relay(&self) -> Option<&str> {
        self.relay.as_deref()
    }

    pub(crate) fn secret(&self) -> &[u8; 32] {
        &self.secret
    }

    /// `HKDF(invite_secret, "clip-ferry v1 pair-key")`
    pub(crate) fn pair_key(&self) -> [u8; 32] {
        derive32(&self.secret[..], b"clip-ferry v1 pair-key")
    }

    /// `HKDF(invite_secret, "clip-ferry v1 pair-channel")`
    pub fn pair_channel(&self) -> [u8; 32] {
        derive32(&self.secret[..], b"clip-ferry v1 pair-channel")
    }

    /// QR에 넣는 문자열: `clipferry://pair/1/<base64url(secret)>[?relay=<url-encoded>]`
    pub fn qr_string(&self) -> String {
        let mut s = format!(
            "{URI_PREFIX}{URI_VERSION}/{}",
            data_encoding::BASE64URL_NOPAD.encode(&self.secret[..])
        );
        if let Some(relay) = &self.relay {
            s.push_str("?relay=");
            s.push_str(&percent_encode(relay));
        }
        s
    }

    /// 사람이 입력하는 텍스트 코드: 비밀 52자 + 체크 기호 1자를 4자씩 `-`로 묶는다.
    pub fn text_code(&self) -> String {
        let values = to_symbols(&self.secret);
        let mut chars: Vec<u8> = values.iter().map(|&v| ALPHABET[v as usize]).collect();
        chars.push(CHECK_SYMBOLS[check_value(&values)]);
        chars
            .chunks(4)
            .map(|c| std::str::from_utf8(c).expect("ASCII"))
            .collect::<Vec<_>>()
            .join("-")
    }

    /// QR 문자열 또는 텍스트 코드를 읽는다.
    pub fn parse(input: &str) -> Result<Self, InviteError> {
        let input = input.trim();
        if input.starts_with(URI_PREFIX) {
            Self::parse_uri(input)
        } else {
            Self::parse_code(input)
        }
    }

    /// 텍스트 코드를 읽는다. 대소문자·하이픈·공백을 무시하고 `O→0`, `I`/`L→1`로 보정한다.
    pub fn parse_code(input: &str) -> Result<Self, InviteError> {
        let chars: Vec<u8> = input
            .bytes()
            .filter(|b| !b.is_ascii_whitespace() && *b != b'-')
            .map(|b| match b.to_ascii_uppercase() {
                b'O' => b'0',
                b'I' | b'L' => b'1',
                c => c,
            })
            .collect();
        if chars.len() != DATA_SYMBOLS + 1 {
            return Err(InviteError::BadLength);
        }

        let mut values = [0u8; DATA_SYMBOLS];
        for (v, c) in values.iter_mut().zip(&chars[..DATA_SYMBOLS]) {
            *v = ALPHABET
                .iter()
                .position(|a| a == c)
                .ok_or(InviteError::BadCharacter)? as u8;
        }
        let check = CHECK_SYMBOLS
            .iter()
            .position(|a| *a == chars[DATA_SYMBOLS])
            .ok_or(InviteError::BadCharacter)?;
        if check != check_value(&values) {
            return Err(InviteError::BadChecksum);
        }
        let secret = from_symbols(&values).ok_or(InviteError::BadCharacter)?;
        Ok(Self::from_secret(secret, None))
    }

    fn parse_uri(input: &str) -> Result<Self, InviteError> {
        let rest = &input[URI_PREFIX.len()..];
        let (version, rest) = rest.split_once('/').ok_or(InviteError::BadUri)?;
        if version != URI_VERSION {
            return Err(InviteError::Unsupported);
        }
        let (secret_part, query) = match rest.split_once('?') {
            Some((s, q)) => (s, Some(q)),
            None => (rest, None),
        };
        let bytes = data_encoding::BASE64URL_NOPAD
            .decode(secret_part.as_bytes())
            .map_err(|_| InviteError::BadUri)?;
        let secret: [u8; 32] = bytes.try_into().map_err(|_| InviteError::BadLength)?;

        let mut relay = None;
        for pair in query.into_iter().flat_map(|q| q.split('&')) {
            if let Some(value) = pair.strip_prefix("relay=") {
                let url = percent_decode(value).ok_or(InviteError::BadUri)?;
                if !(url.starts_with("ws://") || url.starts_with("wss://")) {
                    return Err(InviteError::BadUri);
                }
                relay = Some(url);
            }
        }
        Ok(Self::from_secret(secret, relay))
    }
}

impl std::fmt::Debug for Invite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Invite")
            .field("secret", &"<redacted>")
            .field("relay", &self.relay)
            .finish()
    }
}

/// 256비트를 5비트 기호 52개로 나눈다(앞에서부터, 마지막은 0으로 채움).
fn to_symbols(secret: &[u8; 32]) -> [u8; DATA_SYMBOLS] {
    let mut out = [0u8; DATA_SYMBOLS];
    for (i, v) in out.iter_mut().enumerate() {
        for bit in 0..5 {
            let pos = i * 5 + bit;
            let b = if pos < 256 {
                (secret[pos / 8] >> (7 - pos % 8)) & 1
            } else {
                0
            };
            *v = (*v << 1) | b;
        }
    }
    out
}

/// `to_symbols`의 반대. 채움 비트가 0이 아니면 `None`.
fn from_symbols(values: &[u8; DATA_SYMBOLS]) -> Option<[u8; 32]> {
    let mut secret = [0u8; 32];
    for (i, v) in values.iter().enumerate() {
        for bit in 0..5 {
            let pos = i * 5 + bit;
            let b = (v >> (4 - bit)) & 1;
            if pos < 256 {
                secret[pos / 8] |= b << (7 - pos % 8);
            } else if b != 0 {
                return None;
            }
        }
    }
    Some(secret)
}

/// Crockford 체크 기호: 기호열을 32진수로 읽은 값을 37로 나눈 나머지.
/// 37이 소수이고 32보다 크므로 한 글자 오타와 인접한 두 글자 바꿈을 모두 잡는다.
fn check_value(values: &[u8; DATA_SYMBOLS]) -> usize {
    values
        .iter()
        .fold(0usize, |acc, v| (acc * 32 + *v as usize) % 37)
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invite() -> Invite {
        let mut secret = [0u8; 32];
        for (i, b) in secret.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(37).wrapping_add(11);
        }
        Invite::from_secret(secret, None)
    }

    #[test]
    fn text_code_shape_and_roundtrip() {
        let code = invite().text_code();
        // 4자씩 13묶음 + 체크 기호 1자
        assert_eq!(code.len(), 13 * 4 + 12 + 1 + 1);
        let groups: Vec<_> = code.split('-').collect();
        assert_eq!(groups.len(), 14);
        assert!(groups[..13].iter().all(|g| g.len() == 4));
        assert_eq!(groups[13].len(), 1);
        let parsed = Invite::parse_code(&code).unwrap();
        assert_eq!(parsed.secret(), invite().secret());
        assert_eq!(parsed.pair_channel(), invite().pair_channel());
    }

    #[test]
    fn text_code_test_vector() {
        // secret = (i * 37 + 11) mod 256, i = 0..32
        assert_eq!(
            invite().text_code(),
            "1CR5-AYMZ-RKMG-WCTR-FPHC-FV0H-6SDR-19EA-XWA3-JQM3-N36Z-45SW-C630-9"
        );
    }

    #[test]
    fn text_code_input_is_normalized() {
        let code = invite().text_code();
        let want = invite();

        let lower = code.to_lowercase();
        assert_eq!(Invite::parse_code(&lower).unwrap().secret(), want.secret());

        let spaced = code.replace('-', "  ");
        assert_eq!(Invite::parse_code(&spaced).unwrap().secret(), want.secret());

        let no_sep = code.replace('-', "");
        assert_eq!(Invite::parse_code(&no_sep).unwrap().secret(), want.secret());

        let padded = format!("  {code}\n");
        assert_eq!(Invite::parse_code(&padded).unwrap().secret(), want.secret());
    }

    #[test]
    fn confusable_characters_are_corrected() {
        // 0과 1이 들어 있는 코드를 O, I, L로 입력해도 같은 코드로 읽는다.
        let mut found = false;
        for seed in 0..200u8 {
            let inv = Invite::from_secret([seed; 32], None);
            let code = inv.text_code();
            if code.contains('0') && code.contains('1') {
                let typed = code.replace('0', "o").replace('1', "i");
                assert_eq!(Invite::parse_code(&typed).unwrap().secret(), inv.secret());
                let typed = code.replace('1', "L");
                assert_eq!(Invite::parse_code(&typed).unwrap().secret(), inv.secret());
                found = true;
                break;
            }
        }
        assert!(found);
    }

    #[test]
    fn every_single_character_typo_is_detected() {
        let clean: Vec<u8> = invite()
            .text_code()
            .bytes()
            .filter(|b| *b != b'-')
            .collect();
        for pos in 0..clean.len() {
            let alphabet: &[u8] = if pos == clean.len() - 1 {
                CHECK_SYMBOLS
            } else {
                ALPHABET
            };
            for &c in alphabet {
                if c == clean[pos] {
                    continue;
                }
                let mut bad = clean.clone();
                bad[pos] = c;
                let text = String::from_utf8(bad).unwrap();
                assert!(
                    Invite::parse_code(&text).is_err(),
                    "pos {pos} -> {}",
                    c as char
                );
            }
        }
    }

    #[test]
    fn swapped_adjacent_characters_are_detected() {
        let clean: Vec<u8> = invite()
            .text_code()
            .bytes()
            .filter(|b| *b != b'-')
            .collect();
        for pos in 0..clean.len() - 1 {
            if clean[pos] == clean[pos + 1] {
                continue;
            }
            let mut bad = clean.clone();
            bad.swap(pos, pos + 1);
            let text = String::from_utf8(bad).unwrap();
            assert!(Invite::parse_code(&text).is_err(), "pos {pos}");
        }
    }

    #[test]
    fn malformed_codes_are_rejected() {
        let code = invite().text_code();
        assert_eq!(Invite::parse_code("").err(), Some(InviteError::BadLength));
        assert_eq!(
            Invite::parse_code(&code[..code.len() - 2]).err(),
            Some(InviteError::BadLength)
        );
        assert_eq!(
            Invite::parse_code(&format!("{code}A")).err(),
            Some(InviteError::BadLength)
        );
        let bad_char = code.replacen(&code[..1], "!", 1);
        assert_eq!(
            Invite::parse_code(&bad_char).err(),
            Some(InviteError::BadCharacter)
        );
        // U는 체크 기호 자리에서만 쓸 수 있다.
        let with_u = format!("U{}", &code.replace('-', "")[1..]);
        assert!(Invite::parse_code(&with_u).is_err());
    }

    #[test]
    fn nonzero_padding_bits_are_rejected() {
        let mut values = to_symbols(invite().secret());
        values[DATA_SYMBOLS - 1] |= 1;
        let mut chars: Vec<u8> = values.iter().map(|&v| ALPHABET[v as usize]).collect();
        chars.push(CHECK_SYMBOLS[check_value(&values)]);
        let text = String::from_utf8(chars).unwrap();
        assert_eq!(
            Invite::parse_code(&text).err(),
            Some(InviteError::BadCharacter)
        );
    }

    #[test]
    fn qr_string_roundtrip_with_and_without_relay() {
        let plain = invite();
        let qr = plain.qr_string();
        assert!(qr.starts_with("clipferry://pair/1/"));
        assert!(!qr.contains('?'));
        let back = Invite::parse(&qr).unwrap();
        assert_eq!(back.secret(), plain.secret());
        assert_eq!(back.relay(), None);

        let with_relay = Invite::from_secret(
            *plain.secret(),
            Some("wss://relay.example.com:8443/ws?x=1&y=2".into()),
        );
        let qr = with_relay.qr_string();
        assert!(qr.contains("?relay=wss%3A%2F%2Frelay.example.com%3A8443%2Fws%3Fx%3D1%26y%3D2"));
        let back = Invite::parse(&qr).unwrap();
        assert_eq!(back.secret(), plain.secret());
        assert_eq!(back.relay(), with_relay.relay());
    }

    #[test]
    fn parse_accepts_both_forms() {
        let inv = invite();
        assert_eq!(
            Invite::parse(&inv.text_code()).unwrap().secret(),
            inv.secret()
        );
        assert_eq!(
            Invite::parse(&inv.qr_string()).unwrap().secret(),
            inv.secret()
        );
    }

    #[test]
    fn malformed_uris_are_rejected() {
        let b64 = data_encoding::BASE64URL_NOPAD.encode(invite().secret());
        assert_eq!(
            Invite::parse(&format!("clipferry://pair/2/{b64}")).err(),
            Some(InviteError::Unsupported)
        );
        assert_eq!(
            Invite::parse("clipferry://pair/1/!!!").err(),
            Some(InviteError::BadUri)
        );
        assert_eq!(
            Invite::parse("clipferry://pair/1/AAAA").err(),
            Some(InviteError::BadLength)
        );
        assert_eq!(
            Invite::parse(&format!(
                "clipferry://pair/1/{b64}?relay=http%3A%2F%2Fexample.com"
            ))
            .err(),
            Some(InviteError::BadUri)
        );
        assert_eq!(
            Invite::parse(&format!("clipferry://pair/1/{b64}?relay=%ZZ")).err(),
            Some(InviteError::BadUri)
        );
        assert_eq!(
            Invite::parse("clipferry://pair/1").err(),
            Some(InviteError::BadUri)
        );
    }

    #[test]
    fn derived_values_differ_and_debug_is_redacted() {
        let inv = invite();
        assert_ne!(inv.pair_key(), inv.pair_channel());
        let other = Invite::from_secret([1; 32], None);
        assert_ne!(inv.pair_channel(), other.pair_channel());
        let shown = format!("{inv:?}");
        let b64 = data_encoding::BASE64URL_NOPAD.encode(inv.secret());
        assert!(shown.contains("redacted"));
        assert!(!shown.contains(&b64[..8]));
        assert!(!shown.contains(&inv.text_code()[..4]));
    }

    #[test]
    fn invites_are_unique() {
        assert_ne!(
            Invite::generate(None).secret(),
            Invite::generate(None).secret()
        );
    }
}
