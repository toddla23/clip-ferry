import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
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

  async function saveRelay() {
    setMessage(null);
    try {
      onView(await invoke<AppView>("set_relay_url", { url: relay }));
      setMessage("저장했습니다");
    } catch (e) {
      setMessage(String(e));
    }
  }

  async function addDevice() {
    setMessage(null);
    try {
      onView(await invoke<AppView>("start_invite"));
    } catch (e) {
      setMessage(String(e));
    }
  }

  return (
    <main>
      <header>
        <span className={`dot ${view.connection}`} />
        <h1 data-testid="status">{CONNECTION_LABEL[view.connection]}</h1>
      </header>

      {view.health !== "active" && (
        <p className="warn">
          그룹에서 제거되었거나 너무 오래 연결되지 않았습니다. 다시 페어링하세요.
        </p>
      )}
      {view.error && <p className="warn">{view.error}</p>}

      <section>
        <div className="row between">
          <h2>
            기기 {view.devices.length}대 · 에포크 {view.epoch ?? "-"}
          </h2>
          <button data-testid="add-device" onClick={addDevice} disabled={view.health !== "active"}>
            기기 추가
          </button>
        </div>
        <ul className="devices" data-testid="devices">
          {view.devices.map((d) => (
            <li key={d.id}>
              <span>{d.name}</span>
              {d.isSelf && <span className="badge">이 기기</span>}
              <span className="muted">{d.id}</span>
            </li>
          ))}
        </ul>
      </section>

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
