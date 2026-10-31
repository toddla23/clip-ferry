//! 운영체제 클립보드 구현. 텍스트 읽기·쓰기는 `arboard`, 변경 감지와 민감 항목 판별은 플랫폼별 API.

use std::sync::Mutex;

use crate::clipsync::{Clipboard, Content, Image};

pub struct SystemClipboard {
    // Linux(X11)에서는 쓴 내용을 가진 인스턴스가 살아 있어야 해서 하나를 계속 쥔다.
    inner: Mutex<Option<arboard::Clipboard>>,
}

impl SystemClipboard {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut arboard::Clipboard) -> R) -> Option<R> {
        let mut guard = self.inner.lock().unwrap();
        if guard.is_none() {
            *guard = arboard::Clipboard::new().ok();
        }
        guard.as_mut().map(f)
    }
}

impl Clipboard for SystemClipboard {
    fn change_token(&self) -> u64 {
        platform::change_token(self)
    }

    fn is_sensitive(&self) -> bool {
        platform::is_sensitive()
    }

    /// 텍스트가 있으면 텍스트, 없으면 이미지를 읽는다.
    fn read(&self) -> Option<Content> {
        self.with(|c| {
            if let Some(text) = c.get_text().ok().filter(|t| !t.is_empty()) {
                return Some(Content::Text(text));
            }
            c.get_image().ok().map(|img| {
                Content::Image(Image {
                    width: img.width,
                    height: img.height,
                    rgba: img.bytes.into_owned(),
                })
            })
        })
        .flatten()
    }

    fn write(&self, content: &Content) -> Result<(), String> {
        self.with(|c| match content {
            Content::Text(t) => c.set_text(t.clone()).map_err(|e| e.to_string()),
            Content::Image(i) => c
                .set_image(arboard::ImageData {
                    width: i.width,
                    height: i.height,
                    bytes: std::borrow::Cow::Borrowed(&i.rgba),
                })
                .map_err(|e| e.to_string()),
        })
        .unwrap_or_else(|| Err("클립보드를 열 수 없습니다".to_string()))
    }
}

#[cfg(windows)]
mod platform {
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, GetClipboardSequenceNumber, IsClipboardFormatAvailable,
        OpenClipboard, RegisterClipboardFormatW,
    };
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};

    use super::SystemClipboard;

    pub fn change_token(_: &SystemClipboard) -> u64 {
        u64::from(unsafe { GetClipboardSequenceNumber() })
    }

    fn available(name: PCWSTR) -> Option<u32> {
        let format = unsafe { RegisterClipboardFormatW(name) };
        (format != 0 && unsafe { IsClipboardFormatAvailable(format) }.is_ok()).then_some(format)
    }

    /// 이 포맷이 클립보드에 있고 값(DWORD)이 0인가.
    fn dword_is_zero(format: u32) -> bool {
        unsafe {
            if OpenClipboard(None).is_err() {
                return false;
            }
            let mut zero = false;
            if let Ok(HANDLE(h)) = GetClipboardData(format) {
                let mem = HGLOBAL(h);
                if GlobalSize(mem) >= 4 {
                    let p = GlobalLock(mem) as *const u32;
                    if !p.is_null() {
                        zero = p.read_unaligned() == 0;
                        let _ = GlobalUnlock(mem);
                    }
                }
            }
            let _ = CloseClipboard();
            zero
        }
    }

    /// 비밀번호 관리자 등이 남기는 "처리하지 말라"는 표시.
    /// - `ExcludeClipboardContentFromMonitorProcessing`: 있으면 제외
    /// - `Clipboard Viewer Ignore`: 있으면 제외 (KeePass 계열)
    /// - `CanIncludeInClipboardHistory`, `CanUploadToCloudClipboard`: 값이 0이면 제외
    pub fn is_sensitive() -> bool {
        if available(w!("ExcludeClipboardContentFromMonitorProcessing")).is_some()
            || available(w!("Clipboard Viewer Ignore")).is_some()
        {
            return true;
        }
        [
            w!("CanIncludeInClipboardHistory"),
            w!("CanUploadToCloudClipboard"),
        ]
        .into_iter()
        .filter_map(available)
        .any(dword_is_zero)
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use objc2_app_kit::NSPasteboard;

    use super::SystemClipboard;

    pub fn change_token(_: &SystemClipboard) -> u64 {
        NSPasteboard::generalPasteboard().changeCount() as u64
    }

    /// `nspasteboard.org` 규약: 비밀번호 관리자 등이 붙이는 "숨김"/"일시" 표시.
    pub fn is_sensitive() -> bool {
        let Some(types) = NSPasteboard::generalPasteboard().types() else {
            return false;
        };
        types.iter().any(|t| {
            let t = t.to_string();
            t == "org.nspasteboard.ConcealedType" || t == "org.nspasteboard.TransientType"
        })
    }
}

/// Windows·macOS가 아닌 곳(개발용 Linux): 변경 감지는 내용 해시로 하고, 민감 표시는 보지 않는다.
#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    use super::SystemClipboard;
    use crate::clipsync::{Clipboard, Content};

    pub fn change_token(clip: &SystemClipboard) -> u64 {
        let mut h = DefaultHasher::new();
        match clip.read() {
            Some(Content::Text(t)) => t.hash(&mut h),
            Some(Content::Image(i)) => i.rgba.hash(&mut h),
            None => 0u8.hash(&mut h),
        }
        h.finish()
    }

    pub fn is_sensitive() -> bool {
        false
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use windows::Win32::Foundation::{HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

    /// 실제 클립보드에 포맷을 붙이는 비밀번호 관리자 흉내. 사용자의 클립보드를 덮어쓰므로 `--ignored`.
    fn copy_with_marker(text: &str, marker: Option<(windows::core::PCWSTR, Option<u32>)>) {
        unsafe {
            OpenClipboard(None).unwrap();
            EmptyClipboard().unwrap();
            let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
            let mem = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2).unwrap();
            std::ptr::copy_nonoverlapping(wide.as_ptr(), GlobalLock(mem) as *mut u16, wide.len());
            let _ = GlobalUnlock(mem);
            SetClipboardData(13, Some(HANDLE(mem.0))).unwrap(); // CF_UNICODETEXT
            if let Some((name, value)) = marker {
                let format = RegisterClipboardFormatW(name);
                let size = if value.is_some() { 4 } else { 1 };
                let mem: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, size).unwrap();
                if let Some(v) = value {
                    (GlobalLock(mem) as *mut u32).write_unaligned(v);
                    let _ = GlobalUnlock(mem);
                }
                SetClipboardData(format, Some(HANDLE(mem.0))).unwrap();
            }
            CloseClipboard().unwrap();
        }
    }

    #[test]
    #[ignore = "실제 클립보드를 덮어쓴다"]
    fn password_manager_markers_are_detected() {
        use windows::core::w;
        let clip = SystemClipboard::new();

        copy_with_marker("plain", None);
        assert!(!clip.is_sensitive());
        assert!(clip.read() == Some(Content::Text("plain".into())));

        let before = clip.change_token();
        copy_with_marker(
            "pw",
            Some((w!("ExcludeClipboardContentFromMonitorProcessing"), None)),
        );
        assert!(clip.is_sensitive());
        assert_ne!(clip.change_token(), before);

        copy_with_marker("pw", Some((w!("Clipboard Viewer Ignore"), None)));
        assert!(clip.is_sensitive());

        copy_with_marker("pw", Some((w!("CanIncludeInClipboardHistory"), Some(0))));
        assert!(clip.is_sensitive());

        copy_with_marker("ok", Some((w!("CanIncludeInClipboardHistory"), Some(1))));
        assert!(!clip.is_sensitive());

        clip.write(&Content::Text("written".into())).unwrap();
        assert!(clip.read() == Some(Content::Text("written".into())));
    }

    /// 이미지를 쓰고 다시 읽으면 크기와 불투명 픽셀이 그대로 돌아온다. 실제 클립보드를 덮어쓰므로 `--ignored`.
    #[test]
    #[ignore = "실제 클립보드를 덮어쓴다"]
    fn image_round_trips_through_the_clipboard() {
        let clip = SystemClipboard::new();
        let (w, h) = (64usize, 48usize);
        let rgba: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                [
                    (i % 251) as u8,
                    (i * 3 % 241) as u8,
                    (i * 7 % 239) as u8,
                    255,
                ]
            })
            .collect();
        let before = clip.change_token();
        clip.write(&Content::Image(Image {
            width: w,
            height: h,
            rgba: rgba.clone(),
        }))
        .unwrap();
        assert_ne!(clip.change_token(), before);
        let Some(Content::Image(got)) = clip.read() else {
            panic!("이미지를 읽지 못함");
        };
        assert_eq!((got.width, got.height), (w, h));
        assert!(got.rgba == rgba, "픽셀이 달라짐");
    }
}
