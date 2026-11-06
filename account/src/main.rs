use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use clap::{Parser, Subcommand};
use clip_ferry_account::api::{router, AppState};
use clip_ferry_account::google::{GoogleVerifier, KeySource, RemoteKeys, GOOGLE_JWKS_URL};
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
    /// 접속 토큰 서명 키 (base64url 32바이트, `gen-key`로 만든다)
    #[arg(long, env = "CF_ACCOUNT_SIGNING_KEY", hide_env_values = true)]
    signing_key: Option<String>,
    /// 개발용 로그인 `POST /v1/auth/dev` 허용 (구글 없이 이메일만으로 계정. 공개 서버에서 켜지 않는다)
    #[arg(long, env = "CF_ACCOUNT_DEV_LOGIN")]
    dev_login: bool,
    /// 구글 로그인 시험 페이지 `GET /dev/google`
    #[arg(long, env = "CF_ACCOUNT_DEV_PAGES")]
    dev_pages: bool,
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
    let client_ids: Vec<String> = args
        .google_client_ids
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
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
        dev_login: args.dev_login,
        dev_pages: args.dev_pages,
    });

    // 만료된 갱신 토큰 정리 (한 시간마다)
    let pruner = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60 * 60));
        loop {
            tick.tick().await;
            match pruner.store.prune(clip_ferry_account::now_ms()).await {
                Ok(n) if n > 0 => tracing::info!("pruned {n} expired refresh tokens"),
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
