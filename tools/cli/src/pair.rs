//! `cf invite` / `cf join`: 릴레이를 통해 두 프로필을 페어링한다.

use std::time::Duration;

use anyhow::{bail, Context};
use clip_ferry_core::api::{now_ms, ClipFerry};
use clip_ferry_core::pairing::{Invite, InviteProblem, InviterEvent, JoinerEvent, INVITE_TTL_MS};
use clip_ferry_core::transport::{Client, Event, TransportConfig};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::time::{sleep, timeout};

use crate::random_tag;

/// 사용자 입력이 아닌 값(기기 이름)을 터미널에 찍기 전에 제어 문자를 지운다.
pub fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

/// QR을 터미널용 문자로 그린다.
pub fn render_qr(data: &str) -> anyhow::Result<String> {
    let code = qrcode::QrCode::new(data.as_bytes()).context("QR을 만들 수 없습니다")?;
    Ok(code
        .render::<qrcode::render::unicode::Dense1x2>()
        .quiet_zone(true)
        .build())
}

fn describe(problem: InviteProblem) -> &'static str {
    match problem {
        InviteProblem::Expired => "초대가 만료되었습니다",
        InviteProblem::AlreadyUsed => "이미 사용된 초대입니다",
        InviteProblem::SecondJoiner => {
            "경고: 같은 초대로 다른 기기가 참여를 시도했습니다. 초대를 무효화했습니다"
        }
        InviteProblem::Rejected => "거절된 초대입니다",
    }
}

/// 게시한 요청의 `Ack`를 기다린다. 전달된 수를 돌려준다.
async fn wait_ack(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<Event>,
    request_id: u32,
) -> anyhow::Result<u32> {
    let wait = async {
        while let Some(ev) = events.recv().await {
            match ev {
                Event::Ack(a) if a.request_id == request_id => return Ok(a.delivered),
                Event::Error(e) if e.request_id == request_id => {
                    bail!("릴레이 오류: {:?}", e.code())
                }
                Event::PublishFailed { request_id: r } if r == request_id => {
                    bail!("릴레이에 연결되어 있지 않아 보내지 못했습니다")
                }
                _ => {}
            }
        }
        bail!("연결이 종료됨")
    };
    timeout(Duration::from_secs(5), wait)
        .await
        .context("릴레이 응답 시간 초과")?
}

pub async fn invite(app: &ClipFerry, relay: String) -> anyhow::Result<()> {
    let mut inviter = app.create_invite(Some(relay.clone()))?;
    let pair_channel = inviter.invite().pair_channel();
    let qr = inviter.invite().qr_string();

    println!("{}", render_qr(&qr)?);
    println!("초대 코드: {}", inviter.invite().text_code());
    println!("링크: {qr}");
    println!(
        "{}분 안에 다른 기기에서 `join`을 실행하세요. 기다리는 중...",
        INVITE_TTL_MS / 60_000
    );

    let (client, mut events) = Client::connect(&relay, TransportConfig::default());
    client.subscribe(pair_channel, random_tag());

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let deadline = tokio::time::Instant::now()
        + Duration::from_millis(inviter.expires_at_ms().saturating_sub(now_ms()));
    let mut prompted = false;

    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => {
                if let Some(payload) = inviter.expire(now_ms()) {
                    client.publish(pair_channel, payload, vec![]);
                    sleep(Duration::from_millis(200)).await;
                }
                bail!("초대가 만료되었습니다");
            }
            ev = events.recv() => {
                let Some(ev) = ev else { bail!("연결이 종료됨") };
                match ev {
                    Event::Reconnecting { .. } => eprintln!("reconnecting..."),
                    Event::Deliver(d) if d.channel_id == pair_channel => {
                        match inviter.handle(&d.payload, now_ms()) {
                            InviterEvent::None => {}
                            InviterEvent::Offer { payload, joiner_name, sas } => {
                                client.publish(pair_channel, payload, vec![]);
                                if !prompted {
                                    prompted = true;
                                    println!("참여 요청: {}", sanitize(&joiner_name));
                                    println!("확인 숫자: {sas}");
                                    println!("상대 화면의 숫자와 같으면 y, 다르면 n 을 입력하세요:");
                                }
                            }
                            InviterEvent::Invalid(p) => bail!("{}", describe(p)),
                        }
                    }
                    _ => {}
                }
            }
            line = lines.next_line(), if prompted => {
                match line?.as_deref().map(str::trim) {
                    Some("y" | "Y") => {
                        let approval = app.approve_join(&mut inviter, now_ms())?;
                        let id = client.publish(approval.pair_channel, approval.welcome, vec![]);
                        let delivered = wait_ack(&mut events, id).await?;
                        if delivered == 0 {
                            eprintln!("경고: 상대에게 환영 메시지가 전달되지 않았습니다. 상대가 연결되어 있는지 확인하세요");
                        }
                        // 그룹의 다른 기기에게 구성원 변경을 알린다.
                        if let Some(conn) = app.connection() {
                            client.subscribe(approval.group_channel.0, conn.device_tag.0);
                            let id = client.publish(approval.group_channel.0, approval.roster_update, vec![]);
                            wait_ack(&mut events, id).await?;
                        }
                        println!("승인됨: {} 추가됨", sanitize(&approval.joiner.name));
                        return Ok(());
                    }
                    Some("n" | "N") => {
                        let payload = inviter.reject()?;
                        let id = client.publish(pair_channel, payload, vec![]);
                        let _ = wait_ack(&mut events, id).await;
                        println!("거절했습니다");
                        return Ok(());
                    }
                    None => bail!("입력이 닫혔습니다"),
                    Some(_) => println!("y 또는 n 을 입력하세요:"),
                }
            }
        }
    }
}

/// 초대 코드(텍스트 코드 또는 링크)로 그룹에 참여한다.
pub async fn join(
    app: &ClipFerry,
    code: &str,
    relay_override: Option<String>,
    default_relay: &str,
) -> anyhow::Result<()> {
    let invite = Invite::parse(code)?;
    let relay = relay_override
        .or_else(|| invite.relay().map(str::to_string))
        .unwrap_or_else(|| default_relay.to_string());
    let mut joiner = app.join(invite)?;
    let pair_channel = joiner.invite().pair_channel();

    let (client, mut events) = Client::connect(&relay, TransportConfig::default());
    client.subscribe(pair_channel, random_tag());

    // 초대한 쪽이 아직 구독하기 전일 수 있으므로 응답이 올 때까지 다시 보낸다.
    let mut resend = tokio::time::interval(Duration::from_secs(3));
    let mut connected = false;
    let mut offered = false;
    let give_up = tokio::time::Instant::now() + Duration::from_millis(INVITE_TTL_MS);

    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(give_up) => bail!("시간이 초과되었습니다. 초대한 기기가 응답하지 않았습니다"),
            _ = resend.tick(), if connected && !offered => {
                client.publish(pair_channel, joiner.hello(), vec![]);
            }
            ev = events.recv() => {
                let Some(ev) = ev else { bail!("연결이 종료됨") };
                match ev {
                    Event::Connected(_) => {
                        connected = true;
                        client.publish(pair_channel, joiner.hello(), vec![]);
                    }
                    Event::Disconnected => connected = false,
                    Event::Reconnecting { .. } => eprintln!("reconnecting..."),
                    Event::Deliver(d) if d.channel_id == pair_channel => {
                        match joiner.handle(&d.payload) {
                            JoinerEvent::None => {}
                            JoinerEvent::Offer { sas, inviter_name } => {
                                offered = true;
                                println!("확인 숫자: {sas}");
                                println!(
                                    "초대한 기기({})의 숫자와 같은지 확인하고, 그쪽에서 승인할 때까지 기다리는 중...",
                                    sanitize(&inviter_name)
                                );
                            }
                            JoinerEvent::Joined(joined) => {
                                app.accept_join(joined)?;
                                println!("joined");
                                return Ok(());
                            }
                            JoinerEvent::Rejected => bail!("초대한 기기가 거절했거나 초대가 만료되었습니다"),
                            JoinerEvent::Failed(e) => bail!("참여하지 못했습니다: {e}"),
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_removes_control_characters() {
        assert_eq!(sanitize("phone\x1b[2J\n"), "phone?[2J?");
        assert_eq!(sanitize("노트북"), "노트북");
    }

    #[test]
    fn qr_renders_something() {
        let out = render_qr("clipferry://pair/1/AAAA").unwrap();
        assert!(out.lines().count() > 5);
    }
}
