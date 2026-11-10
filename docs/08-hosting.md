# 08. 공개 서버 후보 (O-04, F40)

조사일: 2026-11-10. **결정: 1단계는 F(Azure for Students)** — D-025. 이 문서는 근거로 남긴다.

## 올릴 것

지금 Pi에서 docker compose로 돌리는 세 가지를 그대로 옮긴다.

- 릴레이(`compose.yml`): WebSocket, 디스크 쓰기 없음, 메모리 적음
- 계정 서버(`compose.account.yml`): axum
- MariaDB: 볼륨 + **백업 필요**(계정·기기·사용량)

앞단 TLS는 Pi에서는 `tailscale serve`가 했고, 공개 서버에서는 Caddy(인증서 자동)나 호스팅이 맡는다. 사용자는 대부분 한국에 있다고 보고 지연을 비교했다.

## 비교

가격은 2026년에 여러 번 바뀌었고 출처마다 숫자가 다르다. **고르기 전에 공식 가격표에서 다시 확인한다.** 원화는 $1 ≈ 1,400원, €1 ≈ 1,600원으로 어림했다(괄호 안이 원래 값).

| | 선택지 | 월 비용(대략) | 한국에서 지연 | 장점 | 단점 |
|---|---|---|---|---|---|
| **A** | **서울 VPS**(AWS Lightsail 서울 / Vultr 서울) + Caddy | 약 7,000~14,000원(1GB 서버, 확인 필요) | 가장 짧음 | Pi compose를 거의 그대로(앞단만 Caddy). 서버를 직접 다룬다 | OS 업데이트·DB 백업을 직접 챙긴다 |
| B | Hetzner(유럽·싱가포르) + Caddy | 약 9,600원(ARM CAX11 €5.99) | 유럽은 왕복 약 250ms, 싱가포르는 중간 | 같은 사양이면 저렴했다 | 2026년 세 번 가격 인상, 9~10월 저가 요금제 신규 생성이 막혔다는 보고 |
| C | Fly.io(도쿄) | 약 7,000~14,000원(기본 머신 1대 $2.19 ≈ 3,100원, 실제로는 2~3대 + 볼륨 $5~10) | 짧음 | TLS·WebSocket 기본, 명령 하나로 배포 | MariaDB를 따로 머신에 올려야 해 Pi 구성과 달라짐. 새 계정은 무료 제공량 없음 |
| D | Oracle 무료(춘천) | 0원 | 짧음 | 무료 | 춘천에서는 무료 ARM 서버를 못 만든다는 보고, 2026-06 무료 한도 절반(4→2 OCPU, 24→12GB). 운영에는 불안정 |
| E | 지금 Pi 공개(Tailscale Funnel) | 0원 | 집 회선에 따라 | 바꿀 것이 거의 없음 | 정전·집 회선·재부팅에 서비스가 묶임, 업로드 대역폭 작음 |
| **F** | **Azure for Students VM**(East Asia·홍콩 — 학생 구독은 Korea Central·Japan East 무료 크기 불가) + Caddy | 0원(학생 무료 VM 월 750시간 + 크레딧 $100로 IP·디스크·트래픽) | 짧음 | 카드 없이, 학생이면 해마다 갱신. A와 같은 방식(compose + Caddy) | 메모리 1GB(B1s·B2ats v2·B2pts v2). 크레딧·학생 자격이 끝나면 멈춤. 학습용 혜택이라 유료화 전에 다시 볼 것 |

도메인 비용은 어느 쪽이든 따로 든다(O-03).

## 결정: F (Azure for Students) — 사용자가 크레딧 $100 보유

방식은 아래 A 추천과 같다(compose + Caddy). 돈이 드는 시점(크레딧·학생 자격 종료, 유료화)이 오면 A로 옮기는 것을 먼저 본다.

- 학생 혜택 출처: https://azure.microsoft.com/free/students (크레딧 기간은 페이지마다 12개월·90/180일로 달라 가입 화면 기준)
- AWS 새 계정은 2026년 기준 크레딧 $100~200·6개월이고 끝나면 계정이 닫힌다. DigitalOcean 학생 크레딧은 2026-08-01에 끝났다.

## 처음 추천했던 것: A (서울 VPS + Caddy)

- Pi의 compose를 거의 그대로 옮긴다. 앞단 `tailscale serve`만 Caddy로 바꾼다.
- 한국 사용자에게 동기화가 가장 빠르다.
- 릴레이를 나눌 때(F26)도 같은 방식으로 서버를 더한다.
- 남는 일: MariaDB 백업 자동화(매일 덤프 → 다른 곳에 보관). F40에서 만든다.
- Lightsail은 스냅샷·고정 IP 관리가 편하고, Vultr는 조금 더 저렴할 수 있다. A로 정하면 두 곳의 서울 1GB 요금을 확인해 하나로 좁힌다.

## 출처

- Hetzner 2026 가격 인상: https://northflank.com/blog/hetzner-cloud-server-price-increases
- Hetzner 저가 요금제 신규 생성 막힘: https://www.bitdoze.com/hetzner-cloud-cost-optimized-plans/
- Fly.io 가격: https://fly.io/pricing , 2026 변경: https://fly.io/pricing-update/
- Oracle 무료 한도 축소: https://infoq.com/news/2026/07/oracle-cloud-free-tier-limits/ , https://space-node.net/blog/oracle-cloud-always-free-limits-2026
- AWS Lightsail 가격: https://aws.amazon.com/ko/lightsail/pricing/ , 서울 지역 제공: https://aws.amazon.com/about-aws/whats-new/2026/04/amazon-lightsail-malaysia
