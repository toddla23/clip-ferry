use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use clap::{Parser, Subcommand};
use clip_ferry_account::admin::normalize_emails;
use clip_ferry_account::api::{router, AppState};
use clip_ferry_account::google::{
    CodeExchange, GoogleVerifier, KeySource, RemoteKeys, GOOGLE_JWKS_URL, GOOGLE_TOKEN_URL,
};
use clip_ferry_account::store::Store;
use clip_ferry_account::tokens::Signer;
use tokio::net::TcpListener;

/// clip-ferry 계정 서버
#[derive(Parser, Debug)]
#[command(name = "clip-ferry-account", version, about)]
struct Args {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    /// 듣기 주소
    #[arg(long, env = "CF_ACCOUNT_LISTEN", default_value = "127.0.0.1:8788")]
    listen: String,
    /// MariaDB 주소 (예: mysql://clipferry:암호@mariadb:3306/clipferry)
    #[arg(long, env = "CF_ACCOUNT_DATABASE_URL", hide_env_values = true)]
    database_url: Option<String>,
    /// DB 없이 메모리에만 둔다 (개발 전용, 재시작하면 사라진다)
    #[arg(long, env = "CF_ACCOUNT_MEMORY_STORE")]
    memory_store: bool,
    /// 받아 줄 구글 OAuth 클라이언트 ID (쉼표로 여러 개: 웹·데스크톱·Android·iOS)
    #[arg(long, env = "CF_ACCOUNT_GOOGLE_CLIENT_IDS", value_delimiter = ',')]
    google_client_ids: Vec<String>,
    /// Android 앱이 Credential Manager의 serverClientId로 쓸 "웹 애플리케이션" 클라이언트 ID (F35).
    /// 주지 않으면 CF_ACCOUNT_GOOGLE_CLIENT_IDS의 첫 번째를 쓴다
    #[arg(long, env = "CF_ACCOUNT_GOOGLE_WEB_CLIENT_ID")]
    google_web_client_id: Option<String>,
    /// 데스크톱 앱용 구글 클라이언트 ID ("데스크톱 앱" 유형, F34). 주면 받아 줄 클라이언트 목록에도 더한다
    #[arg(long, env = "CF_ACCOUNT_GOOGLE_DESKTOP_CLIENT_ID")]
    google_desktop_client_id: Option<String>,
    /// 데스크톱 클라이언트의 보안 비밀 (구글이 코드 교환 때 요구한다. 앱에 넣지 않고 이 서버만 갖는다, D-020)
    #[arg(
        long,
        env = "CF_ACCOUNT_GOOGLE_DESKTOP_CLIENT_SECRET",
        hide_env_values = true
    )]
    google_desktop_client_secret: Option<String>,
    /// 접속 토큰 서명 키 (base64url 32바이트, `gen-key`로 만든다)
    #[arg(long, env = "CF_ACCOUNT_SIGNING_KEY", hide_env_values = true)]
    signing_key: Option<String>,
    /// 개발용 로그인 `POST /v1/auth/dev` 허용 (구글 없이 이메일만으로 계정. 공개 서버에서 켜지 않는다)
    #[arg(long, env = "CF_ACCOUNT_DEV_LOGIN")]
    dev_login: bool,
    /// 구글 로그인 시험 페이지 `GET /dev/google`
    #[arg(long, env = "CF_ACCOUNT_DEV_PAGES")]
    dev_pages: bool,
    /// 릴레이가 사용량을 보낼 때 쓰는 공유 비밀 (릴레이의 CF_RELAY_USAGE_SECRET과 같게). 없으면 받지 않는다
    #[arg(long, env = "CF_ACCOUNT_USAGE_SECRET", hide_env_values = true)]
    usage_secret: Option<String>,
    /// 운영자 페이지 `/admin`에 들어올 수 있는 구글 계정 이메일 (쉼표로 여러 개, F38). 없으면 열지 않는다
    #[arg(long, env = "CF_ACCOUNT_ADMIN_EMAILS", value_delimiter = ',')]
    admin_emails: Vec<String>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// 새 서명 키를 만들어 출력한다 (CF_ACCOUNT_SIGNING_KEY에 넣는다)
    GenKey,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_target(false).init();
    let args = Args::parse();

    if let Some(Cmd::GenKey) = args.cmd {
        println!("{}", Signer::generate_seed_base64());
        return Ok(());
    }

    let signer =
        Signer::from_base64(args.signing_key.as_deref().context(
            "CF_ACCOUNT_SIGNING_KEY가 없습니다 (`clip-ferry-account gen-key`로 만든다)",
        )?)?;
    let mut client_ids: Vec<String> = args
        .google_client_ids
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    // Android 앱의 serverClientId: 따로 주지 않으면 목록의 첫 번째(웹 클라이언트)
    let android = args
        .google_web_client_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| client_ids.first().cloned());
    if let Some(id) = &android {
        if !client_ids.contains(id) {
            client_ids.push(id.clone());
        }
    } else {
        tracing::warn!("no google web client; android login is disabled");
    }
    let desktop = match (
        args.google_desktop_client_id.as_deref().map(str::trim).filter(|s| !s.is_empty()),
        args.google_desktop_client_secret.as_deref().map(str::trim).filter(|s| !s.is_empty()),
    ) {
        (Some(id), Some(secret)) => {
            if !client_ids.iter().any(|c| c == id) {
                client_ids.push(id.to_string());
            }
            Some(CodeExchange::new(GOOGLE_TOKEN_URL, id, secret))
        }
        (None, None) => None,
        _ => anyhow::bail!(
            "데스크톱 구글 로그인에는 CF_ACCOUNT_GOOGLE_DESKTOP_CLIENT_ID와 _SECRET이 둘 다 필요합니다"
        ),
    };
    if desktop.is_none() {
        tracing::warn!("no google desktop client; desktop login is disabled");
    }
    if client_ids.is_empty() {
        tracing::warn!("no google client ids configured; google login is disabled");
    }

    let store = match (&args.database_url, args.memory_store) {
        (Some(url), _) => Store::connect_mariadb(url)
            .await
            .context("MariaDB에 연결하지 못했습니다")?,
        (None, true) => {
            tracing::warn!("memory store: data is lost on restart (development only)");
            Store::memory()
        }
        (None, false) => {
            anyhow::bail!("CF_ACCOUNT_DATABASE_URL이 없습니다 (개발 중이면 --memory-store)")
        }
    };
    let admin_emails = normalize_emails(&args.admin_emails);
    if admin_emails.is_empty() {
        tracing::info!("no admin emails; /admin is disabled");
    } else {
        // 이메일은 로그에 남기지 않는다
        tracing::info!("/admin enabled for {} account(s)", admin_emails.len());
    }
    if args.dev_login {
        tracing::warn!(
            "dev login enabled: anyone can create accounts without google (development only)"
        );
    }

    let state = Arc::new(AppState {
        store,
        signer,
        google: GoogleVerifier::new(
            client_ids,
            KeySource::Remote(RemoteKeys::new(GOOGLE_JWKS_URL)),
        ),
        google_desktop: desktop,
        google_android: android,
        dev_login: args.dev_login,
        dev_pages: args.dev_pages,
        usage_secret: args.usage_secret.clone().filter(|s| !s.is_empty()),
        admin_emails,
    });

    // 만료된 갱신 토큰·보관 기간이 지난 사용량 정리 (한 시간마다)
    let pruner = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60 * 60));
        loop {
            tick.tick().await;
            match pruner.store.prune(clip_ferry_account::now_ms()).await {
                Ok((tokens, usage)) if tokens + usage > 0 => {
                    tracing::info!("pruned {tokens} expired refresh tokens, {usage} old usage rows")
                }
                Ok(_) => {}
                Err(e) => tracing::warn!("prune failed: {e}"),
            }
        }
    });

    let listener = TcpListener::bind(&args.listen).await?;
    tracing::info!(
        "listening on {} (signing key {})",
        listener.local_addr()?,
        state.signer.kid()
    );
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutting down");
        })
        .await?;
    Ok(())
}
