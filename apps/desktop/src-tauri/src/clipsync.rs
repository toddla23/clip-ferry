//! 클립보드 ↔ 코어 연결: 복사를 감지해 보내고, 받은 내용을 클립보드에 쓴다.
//! 암호·프로토콜 로직은 없다. 감지, 쓰기, 에코 방지, 민감 항목 제외만 한다.

use std::collections::hash_map::DefaultHasher;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::time::sleep;

/// 보낼 수 있는 텍스트의 최대 크기 (UTF-8 바이트, 03-protocol).
pub const MAX_TEXT: usize = 1 << 20;
/// 다루는 이미지의 픽셀 데이터(RGBA) 최대 크기. 이보다 크면 보내지도 받지도 않는다.
pub const MAX_RGBA: usize = 256 << 20;
/// 클립보드가 바뀌었는지 보는 간격.
pub const POLL: Duration = Duration::from_millis(250);
/// 바뀐 뒤 이만큼 조용해지면 읽는다 (연속 복사를 하나로 묶는다).
pub const DEBOUNCE: Duration = Duration::from_millis(300);

/// 클립보드의 비트맵. `rgba`는 `width * height * 4` 바이트다.
#[derive(Clone, PartialEq, Eq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// 픽셀 내용은 찍지 않는다 (크기만).
impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Image({}x{}, {} bytes)",
            self.width,
            self.height,
            self.rgba.len()
        )
    }
}

impl Image {
    fn is_valid(&self) -> bool {
        self.width > 0
            && self.height > 0
            && self.rgba.len() <= MAX_RGBA
            && self
                .width
                .checked_mul(self.height)
                .and_then(|n| n.checked_mul(4))
                == Some(self.rgba.len())
    }
}

/// 클립보드에 들어 있는 내용.
#[derive(Clone, PartialEq, Eq)]
pub enum Content {
    Text(String),
    Image(Image),
}

/// 운영체제 클립보드. 플랫폼마다 구현이 다르고, 테스트에서는 가짜로 바꾼다.
pub trait Clipboard: Send + Sync + 'static {
    /// 내용이 바뀔 때마다 달라지는 값 (Windows 시퀀스 번호, macOS `changeCount`).
    fn change_token(&self) -> u64;
    /// 비밀번호 관리자 등이 "보내지 말라"고 표시한 내용인가.
    fn is_sensitive(&self) -> bool;
    /// 텍스트가 있으면 텍스트, 없으면 이미지.
    fn read(&self) -> Option<Content>;
    fn write(&self, content: &Content) -> Result<(), String>;
}

/// 마지막으로 보냈거나 쓴 내용의 해시. 같은 내용을 되돌려 보내지 않기 위해 쓴다.
/// 내용 자체는 쥐지 않는다.
#[derive(Default)]
struct Echo {
    last: Mutex<Option<u64>>,
}

impl Echo {
    fn digest(content: &Content) -> u64 {
        let mut h = DefaultHasher::new();
        match content {
            Content::Text(t) => {
                0u8.hash(&mut h);
                t.hash(&mut h);
            }
            Content::Image(i) => {
                1u8.hash(&mut h);
                (i.width, i.height).hash(&mut h);
                i.rgba.hash(&mut h);
            }
        }
        h.finish()
    }

    fn sendable(content: &Content) -> bool {
        match content {
            Content::Text(t) => !t.is_empty() && t.len() <= MAX_TEXT,
            Content::Image(i) => i.is_valid(),
        }
    }

    /// 받아서 클립보드에 쓴 내용을 기록한다.
    fn record(&self, digest: u64) {
        *self.last.lock().unwrap() = Some(digest);
    }

    /// 보내도 되는 새 내용이면 기록하고 해시를 돌려준다.
    fn claim(&self, content: &Content) -> Option<u64> {
        if !Self::sendable(content) {
            return None;
        }
        let digest = Self::digest(content);
        let mut last = self.last.lock().unwrap();
        if *last == Some(digest) {
            return None;
        }
        *last = Some(digest);
        Some(digest)
    }

    /// 보내지 못했을 때 기록을 지워, 다음에 다시 복사하면 보내지게 한다.
    fn forget(&self, digest: u64) {
        let mut last = self.last.lock().unwrap();
        if *last == Some(digest) {
            *last = None;
        }
    }
}

pub struct ClipSync<C: Clipboard> {
    clip: Arc<C>,
    echo: Echo,
    /// 우리가 클립보드에 쓴 직후의 변경 토큰. 이 변경은 사용자의 복사가 아니다.
    /// (이미지는 쓴 뒤 다시 읽으면 픽셀이 조금 달라질 수 있어 해시만으로는 부족하다)
    own_write: AtomicU64,
}

impl<C: Clipboard> ClipSync<C> {
    pub fn new(clip: C) -> Arc<Self> {
        Arc::new(Self {
            clip: Arc::new(clip),
            echo: Echo::default(),
            own_write: AtomicU64::new(0),
        })
    }

    /// 다른 기기에서 받은 내용을 클립보드에 쓴다. 감지기가 이것을 새 복사로 오해하지 않는다.
    /// 쓰지 못하면 사용자에게 보여 줄 한국어 이유를 돌려준다.
    pub fn write_received(&self, content: &Content) -> Result<(), String> {
        if !Echo::sendable(content) {
            return Err("다룰 수 있는 크기를 넘거나 비어 있습니다".to_string());
        }
        // 쓰기 전에 기록해야, 쓰자마자 감지기가 읽어도 에코로 판단한다.
        let digest = Echo::digest(content);
        self.echo.record(digest);
        let result = self.clip.write(content);
        // 쓰다가 실패해도 클립보드가 일부 바뀌었을 수 있다. 그 변경도 우리가 한 것이라 복사로 보지 않는다.
        self.own_write
            .store(self.clip.change_token(), Ordering::SeqCst);
        if result.is_err() {
            self.echo.forget(digest);
        }
        result
    }

    /// 클립보드를 지켜보다가 새 내용이 복사되면 `send`로 넘긴다. 끝나지 않는 작업이다.
    /// `enabled`가 `false`인 동안에는 클립보드 내용을 읽지도 않는다.
    /// `skip_sensitive`이면 민감 표시가 붙은 복사는 읽지도 보내지도 않는다.
    /// `send`가 `false`를 돌려주면(못 보냄) 다시 복사할 때 보내지도록 기록을 지운다.
    pub async fn watch<E, S, Fut>(self: Arc<Self>, enabled: E, skip_sensitive: bool, mut send: S)
    where
        E: Fn() -> bool,
        S: FnMut(Content) -> Fut,
        Fut: Future<Output = bool>,
    {
        // 시작할 때 이미 있던 내용은 보내지 않는다.
        let mut seen = self.clip.change_token();
        loop {
            sleep(POLL).await;
            let mut token = self.clip.change_token();
            if token == seen {
                continue;
            }
            loop {
                sleep(DEBOUNCE).await;
                let now = self.clip.change_token();
                if now == token {
                    break;
                }
                token = now;
            }
            seen = token;
            if token == self.own_write.load(Ordering::SeqCst) {
                continue;
            }
            if !enabled() || (skip_sensitive && self.clip.is_sensitive()) {
                continue;
            }
            let Some(content) = self.clip.read() else {
                continue;
            };
            let Some(digest) = self.echo.claim(&content) else {
                continue;
            };
            if !send(content).await {
                self.echo.forget(digest);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct State {
        token: u64,
        content: Option<Content>,
        sensitive: bool,
        reads: u32,
        /// 쓴 뒤 내용이 조금 달라지는 클립보드(이미지 재인코딩)를 흉내 낸다.
        distort_writes: bool,
        /// 쓰다가 일부만 바뀐 채 실패하는 클립보드를 흉내 낸다.
        fail_writes: bool,
    }

    /// 운영체제 대신 쓰는 가짜 클립보드. 쓰기도 실제처럼 토큰을 올린다.
    #[derive(Clone, Default)]
    struct Fake(Arc<Mutex<State>>);

    fn text(s: &str) -> Content {
        Content::Text(s.to_string())
    }

    fn image(shade: u8) -> Content {
        Content::Image(Image {
            width: 2,
            height: 2,
            rgba: vec![shade; 16],
        })
    }

    impl Fake {
        fn copy(&self, content: Content) {
            let mut s = self.0.lock().unwrap();
            s.token += 1;
            s.content = Some(content);
            s.sensitive = false;
        }
        fn copy_text(&self, t: &str) {
            self.copy(text(t));
        }
        fn copy_secret(&self, t: &str) {
            self.copy_text(t);
            self.0.lock().unwrap().sensitive = true;
        }
        fn content(&self) -> Option<Content> {
            self.0.lock().unwrap().content.clone()
        }
        fn text(&self) -> Option<String> {
            match self.content() {
                Some(Content::Text(t)) => Some(t),
                _ => None,
            }
        }
        fn reads(&self) -> u32 {
            self.0.lock().unwrap().reads
        }
        fn distort_writes(&self) {
            self.0.lock().unwrap().distort_writes = true;
        }
        fn fail_writes(&self) {
            self.0.lock().unwrap().fail_writes = true;
        }
    }

    impl Clipboard for Fake {
        fn change_token(&self) -> u64 {
            self.0.lock().unwrap().token
        }
        fn is_sensitive(&self) -> bool {
            self.0.lock().unwrap().sensitive
        }
        fn read(&self) -> Option<Content> {
            let mut s = self.0.lock().unwrap();
            s.reads += 1;
            s.content.clone()
        }
        fn write(&self, content: &Content) -> Result<(), String> {
            if self.0.lock().unwrap().fail_writes {
                self.copy(content.clone());
                return Err("클립보드가 쓰기를 거부했습니다".to_string());
            }
            let distort = self.0.lock().unwrap().distort_writes;
            let written = match content {
                Content::Image(i) if distort => {
                    let mut i = i.clone();
                    i.rgba[0] = i.rgba[0].wrapping_add(1);
                    Content::Image(i)
                }
                other => other.clone(),
            };
            self.copy(written);
            Ok(())
        }
    }

    type Sent = Arc<Mutex<Vec<Content>>>;

    /// 감시를 시작한다. `ok`가 `false`면 보내기가 실패한 것처럼 행동한다.
    async fn run(fake: &Fake, enabled: Arc<Mutex<bool>>, ok: bool) -> (Arc<ClipSync<Fake>>, Sent) {
        run_with(fake, enabled, ok, true).await
    }

    async fn run_with(
        fake: &Fake,
        enabled: Arc<Mutex<bool>>,
        ok: bool,
        skip_sensitive: bool,
    ) -> (Arc<ClipSync<Fake>>, Sent) {
        let sync = ClipSync::new(fake.clone());
        let sent: Sent = Arc::default();
        let out = sent.clone();
        let on = enabled;
        tokio::spawn(sync.clone().watch(
            move || *on.lock().unwrap(),
            skip_sensitive,
            move |content| {
                let out = out.clone();
                async move {
                    out.lock().unwrap().push(content);
                    ok
                }
            },
        ));
        // 감시 작업이 시작 시점의 기준값을 잡을 때까지 한 번 돌려 준다.
        sleep(Duration::from_millis(1)).await;
        (sync, sent)
    }

    fn on() -> Arc<Mutex<bool>> {
        Arc::new(Mutex::new(true))
    }

    async fn settle() {
        sleep(Duration::from_secs(2)).await;
    }

    fn sent_texts(s: &Sent) -> Vec<String> {
        s.lock()
            .unwrap()
            .iter()
            .map(|c| match c {
                Content::Text(t) => t.clone(),
                Content::Image(_) => "<image>".to_string(),
            })
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn sends_a_new_copy_once() {
        let fake = Fake::default();
        fake.copy_text("이전 내용");
        let (_sync, out) = run(&fake, on(), true).await;
        settle().await;
        assert!(
            sent_texts(&out).is_empty(),
            "시작 전에 있던 내용은 보내지 않는다"
        );
        fake.copy_text("hello");
        settle().await;
        assert_eq!(sent_texts(&out), ["hello"]);
    }

    #[tokio::test(start_paused = true)]
    async fn rapid_copies_are_merged_into_the_last_one() {
        let fake = Fake::default();
        let (_sync, out) = run(&fake, on(), true).await;
        for t in ["a", "b", "c"] {
            fake.copy_text(t);
            sleep(Duration::from_millis(100)).await;
        }
        settle().await;
        assert_eq!(sent_texts(&out), ["c"]);
    }

    #[tokio::test(start_paused = true)]
    async fn received_text_is_written_but_not_sent_back() {
        let fake = Fake::default();
        let (sync, out) = run(&fake, on(), true).await;
        sync.write_received(&text("from peer")).unwrap();
        settle().await;
        assert_eq!(fake.text().as_deref(), Some("from peer"));
        assert!(sent_texts(&out).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn copying_something_else_after_a_receive_is_sent() {
        let fake = Fake::default();
        let (sync, out) = run(&fake, on(), true).await;
        sync.write_received(&text("from peer")).unwrap();
        settle().await;
        fake.copy_text("mine");
        settle().await;
        fake.copy_text("from peer");
        settle().await;
        assert_eq!(sent_texts(&out), ["mine", "from peer"]);
    }

    #[tokio::test(start_paused = true)]
    async fn same_text_copied_twice_in_a_row_is_sent_once() {
        let fake = Fake::default();
        let (_sync, out) = run(&fake, on(), true).await;
        fake.copy_text("x");
        settle().await;
        fake.copy_text("x");
        settle().await;
        assert_eq!(sent_texts(&out), ["x"]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_b_a_sends_all_three() {
        let fake = Fake::default();
        let (_sync, out) = run(&fake, on(), true).await;
        for t in ["a", "b", "a"] {
            fake.copy_text(t);
            settle().await;
        }
        assert_eq!(sent_texts(&out), ["a", "b", "a"]);
    }

    #[tokio::test(start_paused = true)]
    async fn sensitive_content_is_never_read_or_sent() {
        let fake = Fake::default();
        let (_sync, out) = run(&fake, on(), true).await;
        fake.copy_secret("hunter2");
        settle().await;
        assert!(sent_texts(&out).is_empty());
        assert_eq!(fake.reads(), 0, "민감 항목은 읽지도 않는다");
        fake.copy_text("public");
        settle().await;
        assert_eq!(sent_texts(&out), ["public"]);
    }

    #[tokio::test(start_paused = true)]
    async fn sensitive_content_is_sent_when_the_setting_allows_it() {
        let fake = Fake::default();
        let (_sync, out) = run_with(&fake, on(), true, false).await;
        fake.copy_secret("hunter2");
        settle().await;
        assert_eq!(sent_texts(&out), ["hunter2"]);
    }

    #[tokio::test(start_paused = true)]
    async fn empty_and_oversized_text_is_skipped() {
        let fake = Fake::default();
        let (_sync, out) = run(&fake, on(), true).await;
        fake.copy_text("");
        settle().await;
        fake.copy_text(&"가".repeat(MAX_TEXT / 3 + 1));
        settle().await;
        assert!(sent_texts(&out).is_empty());
        fake.copy_text(&"a".repeat(MAX_TEXT));
        settle().await;
        assert_eq!(sent_texts(&out).len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_send_can_be_retried_by_copying_again() {
        let fake = Fake::default();
        let (_sync, out) = run(&fake, on(), false).await;
        fake.copy_text("x");
        settle().await;
        fake.copy_text("x");
        settle().await;
        assert_eq!(sent_texts(&out), ["x", "x"]);
    }

    #[tokio::test(start_paused = true)]
    async fn nothing_is_read_while_disabled() {
        let fake = Fake::default();
        let gate = Arc::new(Mutex::new(false));
        let (_sync, out) = run(&fake, gate.clone(), true).await;
        fake.copy_text("secret plan");
        settle().await;
        assert!(sent_texts(&out).is_empty());
        assert_eq!(fake.reads(), 0);
        *gate.lock().unwrap() = true;
        fake.copy_text("later");
        settle().await;
        assert_eq!(
            sent_texts(&out),
            ["later"],
            "꺼져 있던 동안의 내용은 나중에도 보내지 않는다"
        );
    }

    // ---------------------------------------------------------------- 이미지

    #[tokio::test(start_paused = true)]
    async fn a_copied_image_is_sent_once() {
        let fake = Fake::default();
        let (_sync, out) = run(&fake, on(), true).await;
        fake.copy(image(7));
        settle().await;
        fake.copy(image(7));
        settle().await;
        assert_eq!(out.lock().unwrap().len(), 1);
        assert!(out.lock().unwrap()[0] == image(7));
        fake.copy(image(8));
        settle().await;
        assert_eq!(out.lock().unwrap().len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_received_image_is_written_and_not_sent_back() {
        let fake = Fake::default();
        let (sync, out) = run(&fake, on(), true).await;
        sync.write_received(&image(5)).unwrap();
        settle().await;
        assert!(fake.content() == Some(image(5)));
        assert!(out.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_received_image_is_not_sent_back_even_if_the_clipboard_changes_its_pixels() {
        let fake = Fake::default();
        fake.distort_writes();
        let (sync, out) = run(&fake, on(), true).await;
        sync.write_received(&image(5)).unwrap();
        settle().await;
        assert!(
            fake.content() != Some(image(5)),
            "가짜 클립보드가 픽셀을 바꿨다"
        );
        assert!(
            out.lock().unwrap().is_empty(),
            "해시가 달라도 우리가 쓴 변경은 무시한다"
        );
        // 그 뒤의 진짜 복사는 보낸다.
        fake.copy(image(9));
        settle().await;
        assert_eq!(out.lock().unwrap().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn invalid_images_are_neither_sent_nor_written() {
        let fake = Fake::default();
        let (sync, out) = run(&fake, on(), true).await;
        let bad = Content::Image(Image {
            width: 2,
            height: 2,
            rgba: vec![0; 15],
        });
        let empty = Content::Image(Image {
            width: 0,
            height: 0,
            rgba: vec![],
        });
        fake.copy(bad.clone());
        settle().await;
        fake.copy(empty.clone());
        settle().await;
        assert!(out.lock().unwrap().is_empty());
        assert!(sync.write_received(&bad).is_err());
        assert!(sync.write_received(&empty).is_err());
        assert!(fake.content() == Some(empty));
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_write_reports_the_reason_and_the_partial_result_is_not_echoed() {
        let fake = Fake::default();
        fake.fail_writes();
        let (sync, out) = run(&fake, on(), true).await;
        let err = sync.write_received(&image(3)).unwrap_err();
        assert!(err.contains("거부"), "{err}");
        settle().await;
        assert!(
            out.lock().unwrap().is_empty(),
            "쓰다 만 내용을 다른 기기로 되돌려 보내면 안 된다"
        );
        // 실패한 뒤의 진짜 복사는 보낸다.
        fake.copy_text("next");
        settle().await;
        assert_eq!(sent_texts(&out), ["next"]);
    }

    #[tokio::test(start_paused = true)]
    async fn text_after_an_image_is_sent_normally() {
        let fake = Fake::default();
        let (_sync, out) = run(&fake, on(), true).await;
        fake.copy(image(1));
        settle().await;
        fake.copy_text("hi");
        settle().await;
        assert_eq!(sent_texts(&out), ["<image>", "hi"]);
    }
}
