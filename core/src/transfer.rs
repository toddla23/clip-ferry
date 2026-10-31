//! 이미지 전송: 보내는 쪽의 분할과 받는 쪽의 조립 (docs/03-protocol.md 2.3).
//!
//! 이미지는 `ImageStart` 한 건과 `ImageChunk` 여러 건으로 나뉘어 각각 별도의 봉투로 간다.
//! 받는 쪽은 메모리에서 조립하고, 길이·청크 수·SHA-256을 확인한 뒤에만 이미지를 내보낸다.

use std::collections::HashMap;

use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};

use crate::crypto::DeviceId;
use crate::pb;

/// 이미지 한 장의 최대 크기(원본 바이트).
pub const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
/// 청크 한 건의 크기.
pub const CHUNK_BYTES: usize = 256 * 1024;
/// 첫 청크 안내(`ImageStart`)가 온 뒤 다 모이기까지 기다리는 시간.
pub const ASSEMBLY_TIMEOUT_MS: u64 = 60_000;
/// 한 기기에서 동시에 조립할 수 있는 전송 수.
pub const MAX_ACTIVE_PER_SENDER: usize = 2;
/// 허용하는 형식.
pub const ALLOWED_MIMES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];

/// 보내기 전에 거부하는 이유.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageError {
    Empty,
    TooLarge,
    UnsupportedMime,
}

/// 나눈 이미지: `start`를 먼저, 이어서 `chunks`를 차례로 보낸다.
#[derive(Debug, Clone, PartialEq)]
pub struct OutgoingImage {
    pub start: pb::ImageStart,
    pub chunks: Vec<pb::ImageChunk>,
}

fn chunk_count(total: usize) -> usize {
    total.div_ceil(CHUNK_BYTES)
}

/// 이미지를 청크로 나눈다. 형식·크기를 확인한다.
pub fn split(
    bytes: &[u8],
    mime: &str,
    width: u32,
    height: u32,
) -> Result<OutgoingImage, ImageError> {
    if bytes.is_empty() {
        return Err(ImageError::Empty);
    }
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(ImageError::TooLarge);
    }
    if !ALLOWED_MIMES.contains(&mime) {
        return Err(ImageError::UnsupportedMime);
    }
    let mut transfer_id = [0u8; 16];
    OsRng.fill_bytes(&mut transfer_id);
    let chunks: Vec<pb::ImageChunk> = bytes
        .chunks(CHUNK_BYTES)
        .enumerate()
        .map(|(i, data)| pb::ImageChunk {
            transfer_id: transfer_id.to_vec(),
            index: i as u32,
            data: data.to_vec(),
        })
        .collect();
    Ok(OutgoingImage {
        start: pb::ImageStart {
            transfer_id: transfer_id.to_vec(),
            mime: mime.to_string(),
            total_len: bytes.len() as u64,
            chunk_count: chunks.len() as u32,
            sha256: Sha256::digest(bytes).to_vec(),
            width,
            height,
        },
        chunks,
    })
}

/// 다 모여 검증을 통과한 이미지.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub mime: String,
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// 받는 쪽이 전송을 거부하는 이유.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssembleError {
    /// `ImageStart`의 값이 규칙에 맞지 않는다.
    BadStart,
    /// 이미 같은 `transfer_id`로 조립 중이다.
    DuplicateStart,
    /// 이 기기에서 이미 2개를 조립 중이다.
    TooManyTransfers,
    /// `ImageStart`를 받은 적 없거나 시간이 지나 버려진 전송의 청크.
    UnknownTransfer,
    /// 번호·길이가 맞지 않거나 같은 청크가 두 번 왔다.
    BadChunk,
    /// 다 모였지만 SHA-256이 맞지 않는다.
    HashMismatch,
}

struct Pending {
    mime: String,
    total_len: usize,
    sha256: [u8; 32],
    width: u32,
    height: u32,
    chunks: Vec<Option<Vec<u8>>>,
    received: usize,
    deadline_ms: u64,
}

impl Pending {
    fn expected_len(&self, index: usize) -> usize {
        if index + 1 == self.chunks.len() {
            self.total_len - index * CHUNK_BYTES
        } else {
            CHUNK_BYTES
        }
    }
}

/// 보낸 기기별로 이미지를 조립한다. 청크는 `ImageStart` 뒤라면 순서가 뒤바뀌어도 된다.
#[derive(Default)]
pub struct Assembler {
    pending: HashMap<(DeviceId, [u8; 16]), Pending>,
}

impl Assembler {
    /// 시간이 지난 전송을 버리고 그 수를 돌려준다.
    pub fn expire(&mut self, now_ms: u64) -> usize {
        let before = self.pending.len();
        self.pending.retain(|_, p| p.deadline_ms > now_ms);
        before - self.pending.len()
    }

    /// 조립 중인 전송 수.
    pub fn active(&self) -> usize {
        self.pending.len()
    }

    pub fn start(
        &mut self,
        sender: DeviceId,
        s: &pb::ImageStart,
        now_ms: u64,
    ) -> Result<(), AssembleError> {
        self.expire(now_ms);
        let id: [u8; 16] = s
            .transfer_id
            .as_slice()
            .try_into()
            .map_err(|_| AssembleError::BadStart)?;
        let sha256: [u8; 32] = s
            .sha256
            .as_slice()
            .try_into()
            .map_err(|_| AssembleError::BadStart)?;
        let total_len = usize::try_from(s.total_len).map_err(|_| AssembleError::BadStart)?;
        if !ALLOWED_MIMES.contains(&s.mime.as_str())
            || total_len == 0
            || total_len > MAX_IMAGE_BYTES
            || s.chunk_count as usize != chunk_count(total_len)
        {
            return Err(AssembleError::BadStart);
        }
        if self.pending.contains_key(&(sender, id)) {
            return Err(AssembleError::DuplicateStart);
        }
        let active = self.pending.keys().filter(|(d, _)| *d == sender).count();
        if active >= MAX_ACTIVE_PER_SENDER {
            return Err(AssembleError::TooManyTransfers);
        }
        self.pending.insert(
            (sender, id),
            Pending {
                mime: s.mime.clone(),
                total_len,
                sha256,
                width: s.width,
                height: s.height,
                chunks: vec![None; s.chunk_count as usize],
                received: 0,
                deadline_ms: now_ms.saturating_add(ASSEMBLY_TIMEOUT_MS),
            },
        );
        Ok(())
    }

    /// 청크를 넣는다. 마지막 청크가 들어와 검증까지 통과하면 이미지를 돌려준다.
    /// 청크가 잘못되면 그 전송 전체를 버린다.
    pub fn chunk(
        &mut self,
        sender: DeviceId,
        c: &pb::ImageChunk,
        now_ms: u64,
    ) -> Result<Option<Image>, AssembleError> {
        self.expire(now_ms);
        let id: [u8; 16] = c
            .transfer_id
            .as_slice()
            .try_into()
            .map_err(|_| AssembleError::UnknownTransfer)?;
        let key = (sender, id);
        let p = self
            .pending
            .get_mut(&key)
            .ok_or(AssembleError::UnknownTransfer)?;

        let index = c.index as usize;
        let ok = index < p.chunks.len()
            && p.chunks[index].is_none()
            && c.data.len() == p.expected_len(index);
        if !ok {
            self.pending.remove(&key);
            return Err(AssembleError::BadChunk);
        }
        p.chunks[index] = Some(c.data.clone());
        p.received += 1;
        if p.received < p.chunks.len() {
            return Ok(None);
        }

        let p = self.pending.remove(&key).expect("방금 있었다");
        let mut bytes = Vec::with_capacity(p.total_len);
        for part in p.chunks.into_iter().flatten() {
            bytes.extend_from_slice(&part);
        }
        if bytes.len() != p.total_len || Sha256::digest(&bytes).as_slice() != p.sha256 {
            return Err(AssembleError::HashMismatch);
        }
        Ok(Some(Image {
            mime: p.mime,
            bytes,
            width: p.width,
            height: p.height,
        }))
    }
}

/// 바이트의 머리말로 알아낸 형식과 크기.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sniffed {
    pub mime: &'static str,
    pub width: u32,
    pub height: u32,
}

fn be16(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from(u16::from_be_bytes([
        *b.get(at)?,
        *b.get(at + 1)?,
    ])))
}

fn le16(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from(u16::from_le_bytes([
        *b.get(at)?,
        *b.get(at + 1)?,
    ])))
}

fn le24(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *b.get(at)?,
        *b.get(at + 1)?,
        *b.get(at + 2)?,
        0,
    ]))
}

/// PNG·JPEG·GIF·WebP 머리말을 읽어 형식과 크기를 알아낸다. 아니면 `None`.
/// 이미지를 디코딩하지는 않는다(헤더만 본다).
pub fn sniff(b: &[u8]) -> Option<Sniffed> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        let w = u32::from_be_bytes(b.get(16..20)?.try_into().ok()?);
        let h = u32::from_be_bytes(b.get(20..24)?.try_into().ok()?);
        return Some(Sniffed {
            mime: "image/png",
            width: w,
            height: h,
        });
    }
    if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        return Some(Sniffed {
            mime: "image/gif",
            width: le16(b, 6)?,
            height: le16(b, 8)?,
        });
    }
    if b.starts_with(&[0xFF, 0xD8]) {
        return sniff_jpeg(b);
    }
    if b.len() >= 30 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        let (width, height) = match &b[12..16] {
            b"VP8 " => (le16(b, 26)? & 0x3FFF, le16(b, 28)? & 0x3FFF),
            b"VP8L" => {
                let bits = u32::from_le_bytes(b.get(21..25)?.try_into().ok()?);
                ((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1)
            }
            b"VP8X" => (le24(b, 24)? + 1, le24(b, 27)? + 1),
            _ => return None,
        };
        return Some(Sniffed {
            mime: "image/webp",
            width,
            height,
        });
    }
    None
}

fn sniff_jpeg(b: &[u8]) -> Option<Sniffed> {
    let mut i = 2;
    while i + 4 <= b.len() {
        if b[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = b[i + 1];
        match marker {
            0xFF => i += 1,
            // 크기 없는 표지
            0x01 | 0xD0..=0xD9 => i += 2,
            // SOF0..SOF15 (DHT·JPG·DAC 제외)
            0xC0..=0xCF if !matches!(marker, 0xC4 | 0xC8 | 0xCC) => {
                return Some(Sniffed {
                    mime: "image/jpeg",
                    height: be16(b, i + 5)?,
                    width: be16(b, i + 7)?,
                });
            }
            _ => i += 2 + be16(b, i + 2)? as usize,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: u64 = 1_000_000;

    fn dev(n: u8) -> DeviceId {
        DeviceId([n; 16])
    }

    /// 길이가 `len`인 서로 다른 바이트열.
    fn data(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 31 % 251) as u8).collect()
    }

    fn feed(a: &mut Assembler, sender: DeviceId, img: &OutgoingImage, now: u64) -> Option<Image> {
        a.start(sender, &img.start, now).unwrap();
        let mut done = None;
        for c in &img.chunks {
            done = a.chunk(sender, c, now).unwrap();
        }
        done
    }

    #[test]
    fn split_makes_chunks_of_256_kib_and_a_matching_start() {
        let bytes = data(CHUNK_BYTES * 2 + 5);
        let img = split(&bytes, "image/png", 10, 20).unwrap();
        assert_eq!(img.start.chunk_count, 3);
        assert_eq!(img.start.total_len, bytes.len() as u64);
        assert_eq!((img.start.width, img.start.height), (10, 20));
        assert_eq!(img.chunks.len(), 3);
        assert_eq!(img.chunks[0].data.len(), CHUNK_BYTES);
        assert_eq!(img.chunks[2].data.len(), 5);
        assert!(img
            .chunks
            .iter()
            .all(|c| c.transfer_id == img.start.transfer_id));
        assert_eq!(img.start.sha256, Sha256::digest(&bytes).to_vec());
    }

    #[test]
    fn split_checks_size_and_format() {
        assert_eq!(split(&[], "image/png", 1, 1), Err(ImageError::Empty));
        assert_eq!(
            split(&vec![0; MAX_IMAGE_BYTES + 1], "image/png", 1, 1),
            Err(ImageError::TooLarge)
        );
        assert_eq!(
            split(&[1], "image/bmp", 1, 1),
            Err(ImageError::UnsupportedMime)
        );
        assert!(split(&vec![0; MAX_IMAGE_BYTES], "image/webp", 1, 1).is_ok());
    }

    #[test]
    fn assembles_what_was_split() {
        let bytes = data(CHUNK_BYTES * 3 + 123);
        let img = split(&bytes, "image/jpeg", 4, 5).unwrap();
        let got = feed(&mut Assembler::default(), dev(1), &img, T0).unwrap();
        assert_eq!(got.bytes, bytes);
        assert_eq!(got.mime, "image/jpeg");
        assert_eq!((got.width, got.height), (4, 5));
    }

    #[test]
    fn a_single_small_chunk_works() {
        let img = split(&[1, 2, 3], "image/gif", 1, 1).unwrap();
        assert_eq!(
            feed(&mut Assembler::default(), dev(1), &img, T0)
                .unwrap()
                .bytes,
            [1, 2, 3]
        );
    }

    #[test]
    fn chunks_may_arrive_in_any_order() {
        let bytes = data(CHUNK_BYTES * 4 + 1);
        let img = split(&bytes, "image/png", 1, 1).unwrap();
        let mut a = Assembler::default();
        a.start(dev(1), &img.start, T0).unwrap();
        let mut done = None;
        for i in [3, 0, 4, 2, 1] {
            done = a.chunk(dev(1), &img.chunks[i], T0).unwrap();
        }
        assert_eq!(done.unwrap().bytes, bytes);
        assert_eq!(a.active(), 0);
    }

    #[test]
    fn a_missing_chunk_never_completes_and_times_out() {
        let img = split(&data(CHUNK_BYTES * 3), "image/png", 1, 1).unwrap();
        let mut a = Assembler::default();
        a.start(dev(1), &img.start, T0).unwrap();
        assert!(a.chunk(dev(1), &img.chunks[0], T0).unwrap().is_none());
        assert!(a.chunk(dev(1), &img.chunks[2], T0).unwrap().is_none());
        assert_eq!(a.active(), 1);
        assert_eq!(a.expire(T0 + ASSEMBLY_TIMEOUT_MS - 1), 0);
        assert_eq!(a.expire(T0 + ASSEMBLY_TIMEOUT_MS), 1);
        assert_eq!(a.active(), 0);
        // 버려진 뒤에 늦게 온 청크는 모르는 전송이다.
        assert_eq!(
            a.chunk(dev(1), &img.chunks[1], T0 + ASSEMBLY_TIMEOUT_MS + 1),
            Err(AssembleError::UnknownTransfer)
        );
    }

    #[test]
    fn a_late_chunk_after_the_deadline_is_rejected_even_without_expire() {
        let img = split(&data(CHUNK_BYTES * 2), "image/png", 1, 1).unwrap();
        let mut a = Assembler::default();
        a.start(dev(1), &img.start, T0).unwrap();
        a.chunk(dev(1), &img.chunks[0], T0).unwrap();
        assert_eq!(
            a.chunk(dev(1), &img.chunks[1], T0 + ASSEMBLY_TIMEOUT_MS),
            Err(AssembleError::UnknownTransfer)
        );
    }

    #[test]
    fn hash_mismatch_is_rejected_and_discarded() {
        let mut img = split(&data(CHUNK_BYTES + 10), "image/png", 1, 1).unwrap();
        img.chunks[1].data[0] ^= 1;
        let mut a = Assembler::default();
        a.start(dev(1), &img.start, T0).unwrap();
        a.chunk(dev(1), &img.chunks[0], T0).unwrap();
        assert_eq!(
            a.chunk(dev(1), &img.chunks[1], T0),
            Err(AssembleError::HashMismatch)
        );
        assert_eq!(a.active(), 0);
    }

    #[test]
    fn bad_chunks_discard_the_whole_transfer() {
        let img = split(&data(CHUNK_BYTES * 2 + 7), "image/png", 1, 1).unwrap();
        let cases: Vec<(&str, pb::ImageChunk)> = vec![
            (
                "번호가 범위 밖",
                pb::ImageChunk {
                    index: 3,
                    ..img.chunks[0].clone()
                },
            ),
            (
                "중간 청크가 짧음",
                pb::ImageChunk {
                    data: vec![0; 5],
                    ..img.chunks[1].clone()
                },
            ),
            (
                "마지막 청크가 김",
                pb::ImageChunk {
                    data: vec![0; 8],
                    ..img.chunks[2].clone()
                },
            ),
            (
                "번호와 길이가 안 맞음",
                pb::ImageChunk {
                    index: 2,
                    ..img.chunks[0].clone()
                },
            ),
        ];
        for (name, bad) in cases {
            let mut a = Assembler::default();
            a.start(dev(1), &img.start, T0).unwrap();
            assert_eq!(
                a.chunk(dev(1), &bad, T0),
                Err(AssembleError::BadChunk),
                "{name}"
            );
            assert_eq!(a.active(), 0, "{name}");
        }
    }

    #[test]
    fn a_duplicate_chunk_discards_the_transfer() {
        let img = split(&data(CHUNK_BYTES * 2), "image/png", 1, 1).unwrap();
        let mut a = Assembler::default();
        a.start(dev(1), &img.start, T0).unwrap();
        a.chunk(dev(1), &img.chunks[0], T0).unwrap();
        assert_eq!(
            a.chunk(dev(1), &img.chunks[0], T0),
            Err(AssembleError::BadChunk)
        );
        assert_eq!(a.active(), 0);
    }

    #[test]
    fn chunks_without_a_start_are_unknown() {
        let img = split(&data(10), "image/png", 1, 1).unwrap();
        assert_eq!(
            Assembler::default().chunk(dev(1), &img.chunks[0], T0),
            Err(AssembleError::UnknownTransfer)
        );
    }

    #[test]
    fn another_device_cannot_feed_my_transfer() {
        let img = split(&data(CHUNK_BYTES * 2), "image/png", 1, 1).unwrap();
        let mut a = Assembler::default();
        a.start(dev(1), &img.start, T0).unwrap();
        assert_eq!(
            a.chunk(dev(2), &img.chunks[0], T0),
            Err(AssembleError::UnknownTransfer)
        );
    }

    #[test]
    fn invalid_start_values_are_rejected() {
        let good = split(&data(CHUNK_BYTES + 1), "image/png", 1, 1)
            .unwrap()
            .start;
        let cases: Vec<(&str, pb::ImageStart)> = vec![
            (
                "id 길이",
                pb::ImageStart {
                    transfer_id: vec![1; 15],
                    ..good.clone()
                },
            ),
            (
                "해시 길이",
                pb::ImageStart {
                    sha256: vec![0; 31],
                    ..good.clone()
                },
            ),
            (
                "형식",
                pb::ImageStart {
                    mime: "image/bmp".into(),
                    ..good.clone()
                },
            ),
            (
                "크기 0",
                pb::ImageStart {
                    total_len: 0,
                    chunk_count: 0,
                    ..good.clone()
                },
            ),
            (
                "20MiB 초과",
                pb::ImageStart {
                    total_len: MAX_IMAGE_BYTES as u64 + 1,
                    chunk_count: 81,
                    ..good.clone()
                },
            ),
            (
                "청크 수 불일치",
                pb::ImageStart {
                    chunk_count: 5,
                    ..good.clone()
                },
            ),
        ];
        for (name, bad) in cases {
            assert_eq!(
                Assembler::default().start(dev(1), &bad, T0),
                Err(AssembleError::BadStart),
                "{name}"
            );
        }
    }

    #[test]
    fn at_most_two_transfers_per_device_at_a_time() {
        let mut a = Assembler::default();
        let imgs: Vec<_> = (0..4)
            .map(|_| split(&data(CHUNK_BYTES + 1), "image/png", 1, 1).unwrap())
            .collect();
        a.start(dev(1), &imgs[0].start, T0).unwrap();
        a.start(dev(1), &imgs[1].start, T0).unwrap();
        assert_eq!(
            a.start(dev(1), &imgs[2].start, T0),
            Err(AssembleError::TooManyTransfers)
        );
        // 다른 기기는 영향받지 않는다.
        a.start(dev(2), &imgs[2].start, T0).unwrap();
        // 하나가 끝나면 다시 시작할 수 있다.
        for c in &imgs[0].chunks {
            a.chunk(dev(1), c, T0).unwrap();
        }
        a.start(dev(1), &imgs[3].start, T0).unwrap();
        // 같은 id를 두 번 시작할 수 없다.
        assert_eq!(
            a.start(dev(1), &imgs[3].start, T0),
            Err(AssembleError::DuplicateStart)
        );
    }

    #[test]
    fn expired_transfers_free_the_slot() {
        let mut a = Assembler::default();
        let imgs: Vec<_> = (0..3)
            .map(|_| split(&data(CHUNK_BYTES + 1), "image/png", 1, 1).unwrap())
            .collect();
        a.start(dev(1), &imgs[0].start, T0).unwrap();
        a.start(dev(1), &imgs[1].start, T0).unwrap();
        a.start(dev(1), &imgs[2].start, T0 + ASSEMBLY_TIMEOUT_MS)
            .unwrap();
    }

    #[test]
    fn a_full_size_image_assembles() {
        let bytes = data(MAX_IMAGE_BYTES);
        let img = split(&bytes, "image/png", 1, 1).unwrap();
        assert_eq!(img.chunks.len(), 80);
        let got = feed(&mut Assembler::default(), dev(1), &img, T0).unwrap();
        assert_eq!(got.bytes, bytes);
    }

    #[test]
    fn sniffs_png_gif_jpeg_webp_headers() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&640u32.to_be_bytes());
        png.extend_from_slice(&480u32.to_be_bytes());
        assert_eq!(
            sniff(&png),
            Some(Sniffed {
                mime: "image/png",
                width: 640,
                height: 480
            })
        );

        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&300u16.to_le_bytes());
        gif.extend_from_slice(&200u16.to_le_bytes());
        assert_eq!(
            sniff(&gif),
            Some(Sniffed {
                mime: "image/gif",
                width: 300,
                height: 200
            })
        );

        // SOI, APP0(길이 16), SOF0(높이 120, 너비 160)
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        jpeg.extend_from_slice(&[0; 14]);
        jpeg.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 120, 0x00, 160]);
        assert_eq!(
            sniff(&jpeg),
            Some(Sniffed {
                mime: "image/jpeg",
                width: 160,
                height: 120
            })
        );

        let mut webp = b"RIFF\0\0\0\0WEBPVP8X\n\0\0\0\0\0\0\0".to_vec();
        webp.extend_from_slice(&[0x7F, 0x02, 0x00, 0xDF, 0x01, 0x00]); // 640x480 (-1)
        webp.extend_from_slice(&[0; 4]);
        assert_eq!(
            sniff(&webp),
            Some(Sniffed {
                mime: "image/webp",
                width: 640,
                height: 480
            })
        );
    }

    #[test]
    fn sniff_rejects_other_or_truncated_data() {
        assert_eq!(sniff(b"hello world, not an image"), None);
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n"), None);
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF]), None);
        assert_eq!(sniff(b"BM......"), None);
        assert_eq!(sniff(&[]), None);
    }
}
