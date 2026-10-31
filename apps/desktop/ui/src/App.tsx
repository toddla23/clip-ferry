import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// src-tauri/src/view.rs 의 AppView 와 같은 모양이다.
type Connection = "connected" | "reconnecting" | "disconnected" | "stopped";
type Health = "active" | "removed" | "stale";

interface AppView {
  seq: number;
  connection: Connection;
  deviceName: string;
  deviceId: string;
  epoch: number | null;
  memberCount: number;
  health: Health;
  relayUrl: string;
  error: string | null;
}

const LABEL: Record<Connection, string> = {
  connected: "연결됨",
  reconnecting: "재연결 중",
  disconnected: "연결 끊김",
  stopped: "중지됨",
};

export default function App() {
  const [view, setView] = useState<AppView | null>(null);
  const [relay, setRelay] = useState("");
  const [message, setMessage] = useState<string | null>(null);

  // 응답과 이벤트는 도착 순서가 바뀔 수 있다. 번호가 더 큰(나중에 만든) 상태만 받아들인다.
  const lastSeq = useRef(0);

  const apply = useCallback((v: AppView) => {
    if (v.seq <= lastSeq.current) return;
    lastSeq.current = v.seq;
    setView(v);
    setRelay((current) => (current === "" ? v.relayUrl : current));
  }, []);

  useEffect(() => {
    // 창이 코어보다 먼저 뜨면 첫 호출이 실패할 수 있어 몇 번 다시 시도한다.
    let cancelled = false;
    const load = (tries: number) =>
      invoke<AppView>("get_view").then(apply, () => {
        if (!cancelled && tries > 0) setTimeout(() => load(tries - 1), 200);
      });
    load(25);
    const unlisten = listen<AppView>("view-changed", (e) => apply(e.payload));
    return () => {
      cancelled = true;
      unlisten.then((f) => f());
    };
  }, [apply]);

  async function saveRelay() {
    setMessage(null);
    try {
      apply(await invoke<AppView>("set_relay_url", { url: relay }));
      setMessage("저장했습니다");
    } catch (e) {
      setMessage(String(e));
    }
  }

  if (!view) return <main />;

  return (
    <main>
      <header>
        <span className={`dot ${view.connection}`} />
        <h1 data-testid="status">{LABEL[view.connection]}</h1>
      </header>

      {view.health !== "active" && (
        <p className="warn">
          그룹에서 제거되었거나 너무 오래 연결되지 않았습니다. 다시 페어링하세요.
        </p>
      )}
      {view.error && <p className="warn">{view.error}</p>}

      <dl>
        <dt>기기 이름</dt>
        <dd>{view.deviceName}</dd>
        <dt>기기 ID</dt>
        <dd>{view.deviceId}</dd>
        <dt>그룹</dt>
        <dd>
          에포크 {view.epoch ?? "-"} · 구성원 {view.memberCount}명
        </dd>
      </dl>

      <label>
        릴레이 주소
        <div className="row">
          <input
            value={relay}
            onChange={(e) => setRelay(e.target.value)}
            placeholder="ws://127.0.0.1:8787"
            spellCheck={false}
          />
          <button onClick={saveRelay}>저장</button>
        </div>
      </label>
      {message && <p className="note">{message}</p>}
    </main>
  );
}
