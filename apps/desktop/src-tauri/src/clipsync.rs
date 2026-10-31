//! 클립보드 ↔ 코어 연결: 복사를 감지해 보내고, 받은 텍스트를 클립보드에 쓴다.
//! 암호·프로토콜 로직은 없다. 감지, 쓰기, 에코 방지, 민감 항목 제외만 한다.

use std::collections::hash_map::DefaultHasher;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::time::sleep;

/// 보낼 수 있는 텍스트의 최대 크기 (UTF-8 바이트, 03-protocol).
pub const MAX_TEXT: usize = 1 << 20;
/// 클립보드가 바뀌었는지 보는 간격.
pub const POLL: Duration = Duration::from_millis(250);
/// 바뀐 뒤 이만큼 조용해지면 읽는다 (연속 복사를 하나로 묶는다).
pub const DEBOUNCE: Duration = Duration::from_millis(300);

/// 운영체제 클립보드. 플랫폼마다 구현이 다르고, 테스트에서는 가짜로 바꾼다.
pub trait Clipboard: Send + Sync + 'static {
    /// 내용이 바뀔 때마다 달라지는 값 (Windows 시퀀스 번호, macOS `changeCount`).
    fn change_token(&self) -> u64;
    /// 비밀번호 관리자 등이 "보내지 말라"고 표시한 내용인가.
    fn is_sensitive(&self) -> bool;
    fn read_text(&self) -> Option<String>;
    fn write_text(&self, text: &str) -> Result<(), String>;
}

/// 마지막으로 보냈거나 쓴 내용의 해시. 같은 내용을 되돌려 보내지 않기 위해 쓴다.
/// 내용 자체는 쥐지 않는다.
#[derive(Default)]
struct Echo {
    last: Mutex<Option<u64>>,
}

impl Echo {
    fn digest(text: &str) -> u64 {
        let mut h = DefaultHasher::new();
        text.hash(&mut h);
        h.finish()
    }

    /// 받아서 클립보드에 쓴 내용을 기록한다.
    fn record(&self, text: &str) {
        *self.last.lock().unwrap() = Some(Self::digest(text));
    }

    /// 보내도 되는 새 내용이면 기록하고 `true`.
    fn claim(&self, text: &str) -> bool {
        if text.is_empty() || text.len() > MAX_TEXT {
            return false;
        }
        let h = Self::digest(text);
        let mut last = self.last.lock().unwrap();
        if *last == Some(h) {
            return false;
        }
        *last = Some(h);
        true
    }

    /// 보내지 못했을 때 기록을 지워, 다음에 다시 복사하면 보내지게 한다.
    fn forget(&self, text: &str) {
        let mut last = self.last.lock().unwrap();
        if *last == Some(Self::digest(text)) {
            *last = None;
        }
    }
}

pub struct ClipSync<C: Clipboard> {
    clip: Arc<C>,
    echo: Echo,
}

impl<C: Clipboard> ClipSync<C> {
    pub fn new(clip: C) -> Arc<Self> {
        Arc::new(Self {
            clip: Arc::new(clip),
            echo: Echo::default(),
        })
    }

    /// 다른 기기에서 받은 텍스트를 클립보드에 쓴다. 감지기가 이것을 새 복사로 오해하지 않는다.
    pub fn write_received(&self, text: &str) {
        if text.len() > MAX_TEXT {
            return;
        }
        // 쓰기 전에 기록해야, 쓰자마자 감지기가 읽어도 에코로 판단한다.
        self.echo.record(text);
        if self.clip.write_text(text).is_err() {
            self.echo.forget(text);
        }
    }

    /// 클립보드를 지켜보다가 새 텍스트가 복사되면 `send`로 넘긴다. 끝나지 않는 작업이다.
    /// `enabled`가 `false`인 동안에는 클립보드 내용을 읽지도 않는다.
    /// `send`가 `false`를 돌려주면(못 보냄) 다시 복사할 때 보내지도록 기록을 지운다.
    pub async fn watch<E, S, Fut>(self: Arc<Self>, enabled: E, mut send: S)
    where
        E: Fn() -> bool,
        S: FnMut(String) -> Fut,
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
            if !enabled() || self.clip.is_sensitive() {
                continue;
            }
            let Some(text) = self.clip.read_text() else {
                continue;
            };
            if !self.echo.claim(&text) {
                continue;
            }
            if !send(text.clone()).await {
                self.echo.forget(&text);
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
        text: Option<String>,
        sensitive: bool,
        reads: u32,
    }

    /// 운영체제 대신 쓰는 가짜 클립보드. 쓰기도 실제처럼 토큰을 올린다.
    #[derive(Clone, Default)]
    struct Fake(Arc<Mutex<State>>);

    impl Fake {
        fn copy(&self, text: &str) {
            let mut s = self.0.lock().unwrap();
            s.token += 1;
            s.text = Some(text.to_string());
            s.sensitive = false;
        }
        fn copy_secret(&self, text: &str) {
            self.copy(text);
            self.0.lock().unwrap().sensitive = true;
        }
        fn text(&self) -> Option<String> {
            self.0.lock().unwrap().text.clone()
        }
        fn reads(&self) -> u32 {
            self.0.lock().unwrap().reads
        }
    }

    impl Clipboard for Fake {
        fn change_token(&self) -> u64 {
            self.0.lock().unwrap().token
        }
        fn is_sensitive(&self) -> bool {
            self.0.lock().unwrap().sensitive
        }
        fn read_text(&self) -> Option<String> {
            let mut s = self.0.lock().unwrap();
            s.reads += 1;
            s.text.clone()
        }
        fn write_text(&self, text: &str) -> Result<(), String> {
            self.copy(text);
            Ok(())
        }
    }

    type Sent = Arc<Mutex<Vec<String>>>;

    /// 감시를 시작한다. `ok`가 `false`면 보내기가 실패한 것처럼 행동한다.
    async fn run(fake: &Fake, enabled: Arc<Mutex<bool>>, ok: bool) -> (Arc<ClipSync<Fake>>, Sent) {
        let sync = ClipSync::new(fake.clone());
        let sent: Sent = Arc::default();
        let out = sent.clone();
        let on = enabled;
        tokio::spawn(sync.clone().watch(
            move || *on.lock().unwrap(),
            move |text| {
                let out = out.clone();
                async move {
                    out.lock().unwrap().push(text);
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

    fn sent(s: &Sent) -> Vec<String> {
        s.lock().unwrap().clone()
    }

    #[tokio::test(start_paused = true)]
    async fn sends_a_new_copy_once() {
        let fake = Fake::default();
        fake.copy("이전 내용");
        let (_sync, out) = run(&fake, on(), true).await;
        settle().await;
        assert!(sent(&out).is_empty(), "시작 전에 있던 내용은 보내지 않는다");
        fake.copy("hello");
        settle().await;
        assert_eq!(sent(&out), ["hello"]);
    }

    #[tokio::test(start_paused = true)]
    async fn rapid_copies_are_merged_into_the_last_one() {
        let fake = Fake::default();
        let (_sync, out) = run(&fake, on(), true).await;
        for t in ["a", "b", "c"] {
            fake.copy(t);
            sleep(Duration::from_millis(100)).await;
        }
        settle().await;
        assert_eq!(sent(&out), ["c"]);
    }

    #[tokio::test(start_paused = true)]
    async fn received_text_is_written_but_not_sent_back() {
        let fake = Fake::default();
        let (sync, out) = run(&fake, on(), true).await;
        sync.write_received("from peer");
        settle().await;
        assert_eq!(fake.text().as_deref(), Some("from peer"));
        assert!(sent(&out).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn copying_something_else_after_a_receive_is_sent() {
        let fake = Fake::default();
        let (sync, out) = run(&fake, on(), true).await;
        sync.write_received("from peer");
        settle().await;
        fake.copy("mine");
        settle().await;
        fake.copy("from peer");
        settle().await;
        assert_eq!(sent(&out), ["mine", "from peer"]);
    }

    #[tokio::test(start_paused = true)]
    async fn same_text_copied_twice_in_a_row_is_sent_once() {
        let fake = Fake::default();
        let (_sync, out) = run(&fake, on(), true).await;
        fake.copy("x");
        settle().await;
        fake.copy("x");
        settle().await;
        assert_eq!(sent(&out), ["x"]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_b_a_sends_all_three() {
        let fake = Fake::default();
        let (_sync, out) = run(&fake, on(), true).await;
        for t in ["a", "b", "a"] {
            fake.copy(t);
            settle().await;
        }
        assert_eq!(sent(&out), ["a", "b", "a"]);
    }

    #[tokio::test(start_paused = true)]
    async fn sensitive_content_is_never_read_or_sent() {
        let fake = Fake::default();
        let (_sync, out) = run(&fake, on(), true).await;
        fake.copy_secret("hunter2");
        settle().await;
        assert!(sent(&out).is_empty());
        assert_eq!(fake.reads(), 0, "민감 항목은 읽지도 않는다");
        fake.copy("public");
        settle().await;
        assert_eq!(sent(&out), ["public"]);
    }

    #[tokio::test(start_paused = true)]
    async fn empty_and_oversized_text_is_skipped() {
        let fake = Fake::default();
        let (_sync, out) = run(&fake, on(), true).await;
        fake.copy("");
        settle().await;
        fake.copy(&"가".repeat(MAX_TEXT / 3 + 1));
        settle().await;
        assert!(sent(&out).is_empty());
        fake.copy(&"a".repeat(MAX_TEXT));
        settle().await;
        assert_eq!(sent(&out).len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_send_can_be_retried_by_copying_again() {
        let fake = Fake::default();
        let (_sync, out) = run(&fake, on(), false).await;
        fake.copy("x");
        settle().await;
        fake.copy("x");
        settle().await;
        assert_eq!(sent(&out), ["x", "x"]);
    }

    #[tokio::test(start_paused = true)]
    async fn nothing_is_read_while_disabled() {
        let fake = Fake::default();
        let gate = Arc::new(Mutex::new(false));
        let (_sync, out) = run(&fake, gate.clone(), true).await;
        fake.copy("secret plan");
        settle().await;
        assert!(sent(&out).is_empty());
        assert_eq!(fake.reads(), 0);
        *gate.lock().unwrap() = true;
        fake.copy("later");
        settle().await;
        assert_eq!(
            sent(&out),
            ["later"],
            "꺼져 있던 동안의 내용은 나중에도 보내지 않는다"
        );
    }
}
