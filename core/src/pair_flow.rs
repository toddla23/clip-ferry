//! 릴레이를 통한 페어링 진행 (docs/03-protocol.md 4절).
//!
//! `pairing`의 상태 기계에 릴레이 연결을 붙여 처음부터 끝까지 돌리는 작업을 만든다.
//! 계정 연결(4.4, F37)은 `start_link_invite`·`start_link_join`.
//! 셸은 `on_update`로 진행 상황을 받고, 초대하는 쪽은 `decide`로 승인·거절을 알린다.
//! 두 함수 모두 tokio 런타임 안에서 불러야 한다. 반환된 값을 드롭하면 작업이 멈춘다.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::api::{now_ms, ClipFerry, Error};
use crate::crypto::GroupKey;
use crate::link::{self, LinkRequestInfo};
use crate::pairing::{
    Invite, InviteError, InviteProblem, Inviter, InviterEvent, Joiner, JoinerEvent, INVITE_TTL_MS,
};
use crate::pb;
use crate::transport::{Client, Event, TransportConfig};

/// 진행을 시작하지 못한 이유 (잘못된 코드, 이미 그룹이 있음 등).
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error(transparent)]
    Invite(#[from] InviteError),
    #[error(transparent)]
    App(#[from] Error),
}

fn random_tag() -> [u8; 16] {
    let mut tag = [0u8; 16];
    tag.copy_from_slice(&GroupKey::generate().as_bytes()[..16]);
    tag
}

fn describe(problem: InviteProblem) -> &'static str {
    match problem {
        InviteProblem::Expired => "초대가 만료되었습니다",
        InviteProblem::AlreadyUsed => "이미 사용된 초대입니다",
        InviteProblem::SecondJoiner => {
            "다른 기기가 같은 초대로 참여를 시도해 초대를 무효화했습니다. 새 초대를 만드세요"
        }
        InviteProblem::Rejected => "거절된 초대입니다",
    }
}

/// 게시한 요청의 `Ack`를 기다려 전달된 수를 돌려준다. 실패하면 `None`.
async fn wait_ack(events: &mut mpsc::UnboundedReceiver<Event>, request_id: u32) -> Option<u32> {
    let wait = async {
        while let Some(ev) = events.recv().await {
            match ev {
                Event::Ack(a) if a.request_id == request_id => return Some(a.delivered),
                Event::Error(e) if e.request_id == request_id => return None,
                Event::PublishFailed { request_id: r } if r == request_id => return None,
                _ => {}
            }
        }
        None
    };
    timeout(Duration::from_secs(5), wait).await.ok().flatten()
}

// ---------------------------------------------------------------- 초대하는 쪽

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InviteUpdate {
    /// 참여 요청이 왔다. 상대 화면에 같은 `sas`가 떠 있는지 사람이 확인한 뒤 `decide`를 부른다.
    Request { joiner_name: String, sas: String },
    /// 승인했고 로스터에 넣었다. `delivered`가 `false`면 상대가 환영 메시지를 받지 못했을 수 있다.
    Approved {
        joiner_name: String,
        delivered: bool,
    },
    /// 거절했다.
    Rejected,
    /// 만료·무효·오류로 끝났다. 사용자에게 보여 줄 문장.
    Ended(String),
}

/// 진행 중인 초대. 드롭하면 초대를 닫는다.
pub struct InviteFlow {
    pub text_code: String,
    pub qr_string: String,
    pub expires_at_ms: u64,
    decide: mpsc::UnboundedSender<bool>,
    task: JoinHandle<()>,
}

impl InviteFlow {
    /// 요청을 받은 뒤에 승인(`true`)이나 거절(`false`)을 알린다. 요청이 없으면 무시된다.
    pub fn decide(&self, approve: bool) {
        let _ = self.decide.send(approve);
    }
}

impl Drop for InviteFlow {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub fn start_invite(
    app: &Arc<ClipFerry>,
    relay_url: &str,
    config: TransportConfig,
    on_update: impl Fn(InviteUpdate) + Send + Sync + 'static,
) -> Result<InviteFlow, Error> {
    let inviter = app.create_invite(Some(relay_url.to_string()))?;
    let text_code = inviter.invite().text_code();
    let qr_string = inviter.invite().qr_string();
    Ok(run_invite(
        app, relay_url, config, inviter, None, text_code, qr_string, on_update,
    ))
}

/// 같은 계정의 새 기기가 보낸 참여 요청(`Session`의 `EventSink::on_link_request`)에 응한다.
/// 계정 채널에 `LinkInvite`를 게시하고, 그 뒤는 QR 초대와 같다(`Request` → `decide`).
/// 돌려받는 `InviteFlow`의 `text_code`·`qr_string`은 비어 있다.
pub fn start_link_invite(
    app: &Arc<ClipFerry>,
    relay_url: &str,
    config: TransportConfig,
    account_channel: [u8; 32],
    request: &LinkRequestInfo,
    on_update: impl Fn(InviteUpdate) + Send + Sync + 'static,
) -> Result<InviteFlow, Error> {
    let (inviter, message) = app.create_link_invite(request)?;
    Ok(run_invite(
        app,
        relay_url,
        config,
        inviter,
        Some((account_channel, message)),
        String::new(),
        String::new(),
        on_update,
    ))
}

#[allow(clippy::too_many_arguments)]
fn run_invite(
    app: &Arc<ClipFerry>,
    relay_url: &str,
    config: TransportConfig,
    mut inviter: Inviter,
    announce: Option<([u8; 32], Vec<u8>)>,
    text_code: String,
    qr_string: String,
    on_update: impl Fn(InviteUpdate) + Send + Sync + 'static,
) -> InviteFlow {
    let pair_channel = inviter.invite().pair_channel();
    let expires_at_ms = inviter.expires_at_ms();

    let (client, mut events) = Client::connect(relay_url, config);
    client.subscribe(pair_channel, random_tag());
    if let Some((channel, _)) = &announce {
        client.subscribe(*channel, random_tag());
    }
    let (decide, mut decisions) = mpsc::unbounded_channel::<bool>();
    let app = app.clone();

    let task = tokio::spawn(async move {
        let deadline = tokio::time::Instant::now()
            + Duration::from_millis(expires_at_ms.saturating_sub(now_ms()));
        let mut asked = false;
        // 계정 연결: 새 기기가 응답할 때까지 `LinkInvite`를 다시 게시한다.
        let mut heard = announce.is_none();
        let mut resend = tokio::time::interval(Duration::from_secs(3));
        let end = |msg: &str| on_update(InviteUpdate::Ended(msg.to_string()));
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => {
                    if let Some(payload) = inviter.expire(now_ms()) {
                        client.publish(pair_channel, payload, vec![]);
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                    return end("초대가 만료되었습니다");
                }
                _ = resend.tick(), if !heard => {
                    if let Some((channel, message)) = &announce {
                        client.publish(*channel, message.clone(), vec![]);
                    }
                }
                ev = events.recv() => {
                    let Some(ev) = ev else { return end("릴레이 연결이 종료되었습니다") };
                    let Event::Deliver(d) = ev else { continue };
                    if d.channel_id != pair_channel { continue }
                    match inviter.handle(&d.payload, now_ms()) {
                        InviterEvent::None => {}
                        InviterEvent::Reply(payload) => {
                            heard = true;
                            client.publish(pair_channel, payload, vec![]);
                        }
                        InviterEvent::Offer { payload, joiner_name, sas } => {
                            heard = true;
                            client.publish(pair_channel, payload, vec![]);
                            if !asked {
                                asked = true;
                                on_update(InviteUpdate::Request { joiner_name, sas });
                            }
                        }
                        InviterEvent::Invalid(p) => return end(describe(p)),
                    }
                }
                Some(approve) = decisions.recv(), if asked => {
                    if approve {
                        let approval = match app.approve_join(&mut inviter, now_ms()) {
                            Ok(a) => a,
                            Err(e) => return end(&e.to_string()),
                        };
                        let id = client.publish(approval.pair_channel, approval.welcome, vec![]);
                        let delivered = wait_ack(&mut events, id).await.is_some_and(|n| n > 0);
                        // 그룹의 다른 기기에게 구성원 변경을 알린다.
                        if let Some(conn) = app.connection() {
                            client.subscribe(approval.group_channel.0, conn.device_tag.0);
                            let id = client.publish(
                                approval.group_channel.0,
                                approval.roster_update,
                                vec![],
                            );
                            let _ = wait_ack(&mut events, id).await;
                        }
                        return on_update(InviteUpdate::Approved {
                            joiner_name: approval.joiner.name,
                            delivered,
                        });
                    }
                    if let Ok(payload) = inviter.reject() {
                        let id = client.publish(pair_channel, payload, vec![]);
                        let _ = wait_ack(&mut events, id).await;
                    }
                    return on_update(InviteUpdate::Rejected);
                }
            }
        }
    });

    InviteFlow {
        text_code,
        qr_string,
        expires_at_ms,
        decide,
        task,
    }
}

// ---------------------------------------------------------------- 참여하는 쪽

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinUpdate {
    /// 릴레이에 연결되었다. 초대한 기기의 응답을 기다린다.
    /// (계정 연결이면 계정의 다른 기기가 요청에 응하기를 기다린다.)
    Connected,
    /// 초대한 기기가 응답했다. 같은 `sas`가 그쪽 화면에도 떠 있는지 확인하고 승인을 기다린다.
    WaitingApproval { inviter_name: String, sas: String },
    /// 그룹에 들어왔고 로컬에 저장했다.
    Joined,
    /// 실패로 끝났다. 사용자에게 보여 줄 문장.
    Ended(String),
}

/// 진행 중인 참여. 드롭하면 멈춘다.
pub struct JoinFlow {
    task: JoinHandle<()>,
}

impl Drop for JoinFlow {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// 초대 코드(텍스트 코드 또는 링크)로 참여를 시작한다.
/// 릴레이는 `relay_override` > 초대에 들어 있는 주소 > `default_relay` 순으로 정한다.
pub fn start_join(
    app: &Arc<ClipFerry>,
    code: &str,
    relay_override: Option<String>,
    default_relay: &str,
    config: TransportConfig,
    on_update: impl Fn(JoinUpdate) + Send + Sync + 'static,
) -> Result<JoinFlow, StartError> {
    let invite = Invite::parse(code)?;
    let relay = relay_override
        .or_else(|| invite.relay().map(str::to_string))
        .unwrap_or_else(|| default_relay.to_string());
    let joiner = app.join(invite)?;
    Ok(run_join(
        app,
        &relay,
        config,
        Stage::Joining(Box::new(joiner)),
        on_update,
    ))
}

/// 같은 계정으로 로그인한 다른 기기에 참여를 요청한다(F37).
/// `ticket`은 계정 서버가 이 기기의 공개키로 서명해 준 참여 확인서다.
/// 계정의 기기가 응하면(`LinkInvite`) 봉인 후 공개 페어링으로 이어진다.
pub fn start_link_join(
    app: &Arc<ClipFerry>,
    relay_url: &str,
    config: TransportConfig,
    ticket: pb::LinkTicket,
    on_update: impl Fn(JoinUpdate) + Send + Sync + 'static,
) -> Result<JoinFlow, Error> {
    if app.connection().is_some() {
        return Err(Error::GroupExists);
    }
    app.link_identity()?;
    let channel = link::account_channel(&ticket.user_id);
    let (request_id, request) = link::build_request(ticket);
    Ok(run_join(
        app,
        relay_url,
        config,
        Stage::Asking {
            channel,
            request_id,
            request,
        },
        on_update,
    ))
}

enum Stage {
    /// 계정 연결: 계정 채널에 요청을 올리고 `LinkInvite`를 기다린다.
    Asking {
        channel: [u8; 32],
        request_id: [u8; 16],
        request: Vec<u8>,
    },
    Joining(Box<Joiner>),
}

fn run_join(
    app: &Arc<ClipFerry>,
    relay_url: &str,
    config: TransportConfig,
    mut stage: Stage,
    on_update: impl Fn(JoinUpdate) + Send + Sync + 'static,
) -> JoinFlow {
    let (client, mut events) = Client::connect(relay_url, config);
    match &stage {
        Stage::Asking { channel, .. } => client.subscribe(*channel, random_tag()),
        Stage::Joining(j) => client.subscribe(j.invite().pair_channel(), random_tag()),
    }
    let app = app.clone();

    let task = tokio::spawn(async move {
        // 상대가 아직 구독하기 전일 수 있으므로 응답이 올 때까지 다시 보낸다.
        let mut resend = tokio::time::interval(Duration::from_secs(3));
        let mut connected = false;
        let mut offered = false;
        let give_up = tokio::time::Instant::now() + Duration::from_millis(INVITE_TTL_MS);
        let end = |msg: &str| on_update(JoinUpdate::Ended(msg.to_string()));
        let send = |stage: &Stage| match stage {
            Stage::Asking {
                channel, request, ..
            } => {
                client.publish(*channel, request.clone(), vec![]);
            }
            Stage::Joining(j) => {
                if let Some(m) = j.outgoing() {
                    client.publish(j.invite().pair_channel(), m, vec![]);
                }
            }
        };
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(give_up) => {
                    return end(match stage {
                        Stage::Asking { .. } => {
                            "시간이 초과되었습니다. 같은 계정의 다른 기기에서 앱이 켜져 있는지 확인하세요"
                        }
                        Stage::Joining(_) => "시간이 초과되었습니다. 초대한 기기가 응답하지 않았습니다",
                    });
                }
                _ = resend.tick(), if connected => send(&stage),
                ev = events.recv() => {
                    let Some(ev) = ev else { return end("릴레이 연결이 종료되었습니다") };
                    match ev {
                        Event::Connected(_) => {
                            if !connected {
                                on_update(JoinUpdate::Connected);
                            }
                            connected = true;
                            send(&stage);
                        }
                        Event::Disconnected => connected = false,
                        Event::Deliver(d) => match &mut stage {
                            Stage::Asking { channel, request_id, .. } => {
                                if d.channel_id != *channel { continue }
                                let joiner = match app.link_join(&d.payload, request_id) {
                                    Ok(Some(j)) => j,
                                    Ok(None) => continue,
                                    Err(e) => return end(&e.to_string()),
                                };
                                client.unsubscribe(*channel);
                                client.subscribe(joiner.invite().pair_channel(), random_tag());
                                stage = Stage::Joining(Box::new(joiner));
                                send(&stage);
                            }
                            Stage::Joining(joiner) => {
                                if d.channel_id != joiner.invite().pair_channel() { continue }
                                match joiner.handle(&d.payload) {
                                    JoinerEvent::None => {}
                                    JoinerEvent::Offer { sas, inviter_name } => {
                                        if !offered {
                                            offered = true;
                                            // 계정 연결은 여기서 키를 공개한다(`PairReveal`).
                                            send(&stage);
                                            on_update(JoinUpdate::WaitingApproval { inviter_name, sas });
                                        }
                                    }
                                    JoinerEvent::Joined(joined) => {
                                        return match app.accept_join(joined) {
                                            Ok(()) => on_update(JoinUpdate::Joined),
                                            Err(e) => end(&e.to_string()),
                                        };
                                    }
                                    JoinerEvent::Rejected => {
                                        return end("초대한 기기가 거절했거나 초대가 만료되었습니다");
                                    }
                                    JoinerEvent::Failed(e) => {
                                        return end(&format!("참여하지 못했습니다: {e}"));
                                    }
                                }
                            }
                        },
                        _ => {}
                    }
                }
            }
        }
    });

    JoinFlow { task }
}
