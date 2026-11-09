import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import Devices from "./Devices";
import { CONNECTION_LABEL, type AppView } from "./types";

export default function Home({
  view,
  onView,
}: {
  view: AppView;
  onView: (v: AppView) => void;
}) {
  const [relay, setRelay] = useState(view.relayUrl);
  const [message, setMessage] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [busy, setBusy] = useState(false);

  async function deleteAccount() {
    setBusy(true);
    setMessage(null);
    try {
      onView(await invoke<AppView>("delete_account"));
    } catch (e) {
      setMessage(String(e));
    } finally {
      setBusy(false);
    }
  }

  // 명령을 부르고 결과 상태를 반영한다. 실패하면 메시지를 보여 준다.
  async function call(cmd: string, args?: Record<string, unknown>, ok?: string) {
    setMessage(null);
    try {
      onView(await invoke<AppView>(cmd, args));
      if (ok) setMessage(ok);
    } catch (e) {
      setMessage(String(e));
    }
  }

  return (
    <main>
      <header>
        <span className={`dot ${view.paused ? "paused" : view.connection}`} />
        <h1 data-testid="status">
          {view.paused ? "일시정지" : CONNECTION_LABEL[view.connection]}
        </h1>
        {view.paused && (
          <button
            className="secondary"
            data-testid="resume"
            onClick={() => call("set_paused", { paused: false })}
          >
            재개
          </button>
        )}
      </header>

      {view.error && <p className="warn">{view.error}</p>}

      {view.linkRequest && (
        <section data-testid="link-request">
          <h2>새 기기 연결 요청</h2>
          <p>
            같은 계정으로 로그인한 <b>{view.linkRequest.name}</b> 기기가 이 그룹에 들어오려고 합니다.
            연결을 누르면 양쪽 화면에 확인 숫자가 뜹니다.
          </p>
          <div className="row">
            <button data-testid="link-accept" onClick={() => call("accept_link_request")}>
              연결
            </button>
            <button
              className="secondary"
              data-testid="link-dismiss"
              onClick={() => call("dismiss_link_request")}
            >
              무시
            </button>
          </div>
        </section>
      )}

      <section>
        <div className="row between">
          <h2>
            기기 {view.devices.length}대 · 에포크 {view.epoch ?? "-"}
          </h2>
          <button data-testid="add-device" onClick={() => call("start_invite")}>
            기기 추가
          </button>
        </div>
        <Devices view={view} onView={onView} onError={setMessage} />
      </section>

      <section>
        <h2>계정</h2>
        <div className="row between">
          <span data-testid="account-email">
            {view.account?.name ? `${view.account.name} · ` : ""}
            {view.account?.email}
          </span>
          <div className="row">
            <button className="secondary" data-testid="logout" onClick={() => call("logout")}>
              로그아웃
            </button>
            {!deleting && (
              <button
                className="secondary"
                data-testid="delete-account"
                onClick={() => setDeleting(true)}
              >
                계정 삭제
              </button>
            )}
          </div>
        </div>
        {deleting && (
          <div data-testid="delete-account-confirm">
            <p className="warn">
              계정을 삭제하면 서버에 있는 계정 정보·기기 목록·사용 기록이 바로 지워지고 되돌릴 수 없습니다.
              같은 계정으로 로그인한 다른 기기도 곧 로그아웃됩니다. 이 기기의 그룹 정보도 지웁니다.
              클립보드 내용은 원래 서버에 없습니다.
            </p>
            <div className="row">
              <button
                className="danger"
                data-testid="delete-account-ok"
                disabled={busy}
                onClick={deleteAccount}
              >
                {busy ? "삭제하는 중…" : "계정 삭제"}
              </button>
              <button className="secondary" disabled={busy} onClick={() => setDeleting(false)}>
                취소
              </button>
            </div>
          </div>
        )}
      </section>

      <section>
        <h2>설정</h2>
        <label className="check">
          <input
            type="checkbox"
            data-testid="send-enabled"
            checked={view.sendEnabled}
            onChange={(e) =>
              call("set_sync", { send: e.target.checked, receive: view.receiveEnabled })
            }
          />
          복사한 내용을 다른 기기로 보내기
        </label>
        <label className="check">
          <input
            type="checkbox"
            data-testid="receive-enabled"
            checked={view.receiveEnabled}
            onChange={(e) =>
              call("set_sync", { send: view.sendEnabled, receive: e.target.checked })
            }
          />
          다른 기기에서 받은 내용을 클립보드에 쓰기
        </label>
        <label className="check">
          <input
            type="checkbox"
            data-testid="autostart"
            checked={view.autostart}
            onChange={(e) => call("set_autostart", { on: e.target.checked })}
          />
          로그인할 때 자동으로 시작
        </label>
        {!view.paused && (
          <button
            className="secondary"
            data-testid="pause"
            onClick={() => call("set_paused", { paused: true })}
          >
            일시정지
          </button>
        )}
        <label>
          릴레이 주소
          <div className="row">
            <input
              value={relay}
              onChange={(e) => setRelay(e.target.value)}
              placeholder="ws://127.0.0.1:8787"
              spellCheck={false}
            />
            <button onClick={() => call("set_relay_url", { url: relay }, "저장했습니다")}>
              저장
            </button>
          </div>
        </label>
      </section>
      {message && (
        <p className="note" data-testid="message">
          {message}
        </p>
      )}
    </main>
  );
}
