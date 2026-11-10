import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AppView } from "./types";
import { useServerReveal } from "./useServerReveal";

// 로그인 전 화면 (F34). 로그인해야 릴레이에 연결한다. 그룹·페어링은 로그아웃해도 그대로 남는다.
export default function Login({ view, onView }: { view: AppView; onView: (v: AppView) => void }) {
  const [server, setServer] = useState(view.accountUrl);
  const [showServer, setShowServer] = useState(false);
  const [email, setEmail] = useState("");
  const [error, setError] = useState<string | null>(null);
  const reveal = useServerReveal(view, onView);

  async function call(cmd: string, args?: Record<string, unknown>) {
    setError(null);
    try {
      onView(await invoke<AppView>(cmd, args));
    } catch (e) {
      setError(String(e));
    }
  }

  const hasGroup = view.stage === "ready";

  return (
    <main>
      <h1 onClick={reveal}>clip-ferry 로그인</h1>
      <p className="note">
        {hasGroup
          ? "다시 로그인하면 이 기기의 그룹과 페어링은 그대로 이어서 씁니다."
          : "구글 계정으로 로그인하세요. 클립보드 내용은 지금처럼 기기에서만 암호화·복호화됩니다."}
      </p>

      {view.loginBusy ? (
        <div className="stack">
          <p data-testid="login-waiting">브라우저에서 구글 로그인을 마치세요…</p>
          <button className="secondary" data-testid="cancel-login" onClick={() => call("cancel_login")}>
            취소
          </button>
        </div>
      ) : (
        <div className="stack">
          <button data-testid="google-login" onClick={() => call("start_google_login")}>
            Google로 로그인
          </button>
        </div>
      )}

      {(error ?? view.loginMessage) && (
        <p className="warn" data-testid="login-error">
          {error ?? view.loginMessage}
        </p>
      )}

      {view.showServerSettings && (
      <section>
        <button className="link" onClick={() => setShowServer(!showServer)}>
          {showServer ? "서버 설정 닫기" : "서버 설정"}
        </button>
        {showServer && (
          <label>
            계정 서버 주소
            <div className="row">
              <input
                data-testid="account-url"
                value={server}
                onChange={(e) => setServer(e.target.value)}
                placeholder="https://<서버>:8443"
                spellCheck={false}
                disabled={view.loginBusy}
              />
              <button
                disabled={view.loginBusy}
                onClick={() => call("set_account_url", { url: server })}
              >
                저장
              </button>
            </div>
          </label>
        )}
      </section>
      )}

      {view.devLogin && !view.loginBusy && (
        <section>
          <label>
            개발용 로그인 (계정 서버 --dev-login, 개발 빌드에서만)
            <div className="row">
              <input
                data-testid="dev-email"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                placeholder="me@test.dev"
                spellCheck={false}
              />
              <button
                className="secondary"
                data-testid="dev-login"
                disabled={email.trim() === ""}
                onClick={() => call("dev_login", { email })}
              >
                로그인
              </button>
            </div>
          </label>
        </section>
      )}
    </main>
  );
}
