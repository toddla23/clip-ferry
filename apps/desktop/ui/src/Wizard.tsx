import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AppView } from "./types";

// 첫 실행: 기기 이름을 정하고, 같은 계정의 다른 기기로 연결하거나(F37),
// 이 계정에 아직 그룹이 없으면 새 그룹을 만든다(계정 하나당 그룹 하나, F44).
export default function Wizard({ view }: { view: AppView }) {
  // 기기가 이미 만들어져 있으면(참여가 중간에 끊긴 경우) 이름은 그대로 쓴다.
  const fixedName = view.deviceName !== "";
  const [name, setName] = useState(view.deviceName || view.defaultName);
  // 처음부터 다시 시작: 0 = 닫힘, 1 = 설명, 2 = 마지막 확인
  const [restart, setRestart] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function run(cmd: string, args: Record<string, unknown>) {
    setError(null);
    setBusy(true);
    try {
      await invoke(cmd, args);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  const nameOk = name.trim().length > 0;
  const has = view.accountHasGroup;

  return (
    <main>
      <h1>clip-ferry 시작하기</h1>
      <p className="note">
        {has === false
          ? "이 계정의 첫 기기입니다. 기기 이름을 정하고 새 그룹을 만드세요."
          : "기기 이름을 정하고, 같은 계정으로 쓰는 다른 기기로 연결하세요."}
      </p>
      {view.error && (
        <p className="warn" data-testid="view-error">
          {view.error}
        </p>
      )}

      <label>
        기기 이름
        <input
          data-testid="name"
          value={name}
          maxLength={40}
          disabled={fixedName || busy}
          onChange={(e) => setName(e.target.value)}
        />
      </label>

      {has === null && <p className="note">계정 정보를 확인하는 중입니다…</p>}

      {has === false && (
        <div className="stack">
          <button
            data-testid="new-group"
            disabled={!nameOk || busy}
            onClick={() => run("create_group", { name })}
          >
            새 그룹 만들기
          </button>
        </div>
      )}

      {has === true && restart === 0 && (
        <div className="stack">
          <button
            data-testid="link-join"
            disabled={!nameOk || busy}
            onClick={() => run("start_link_join", { name })}
          >
            이 계정의 다른 기기로 연결
          </button>
          <p className="note">
            다른 기기에서 clip-ferry가 켜져 있어야 합니다. 그 기기에 요청이 뜨면 연결을 누르세요.
          </p>
          <button
            className="link"
            data-testid="restart"
            disabled={busy}
            onClick={() => setRestart(1)}
          >
            다른 기기를 쓸 수 없나요? (처음부터 다시 시작)
          </button>
        </div>
      )}

      {has === true && restart === 1 && (
        <section data-testid="restart-explain">
          <h2>처음부터 다시 시작</h2>
          <p>
            이 계정의 다른 기기를 모두 잃었거나 쓸 수 없을 때만 하세요. 이 기기에서 새 그룹을
            만들고, 이 계정의 다른 기기들은 다음에 켜질 때 예전 그룹에서 나옵니다. 그 기기들은 다시
            연결해야 합니다.
          </p>
          <div className="row">
            <button className="danger" data-testid="restart-next" onClick={() => setRestart(2)}>
              계속
            </button>
            <button className="secondary" onClick={() => setRestart(0)}>
              취소
            </button>
          </div>
        </section>
      )}

      {has === true && restart === 2 && (
        <section data-testid="restart-confirm">
          <h2>정말 처음부터 다시 시작할까요?</h2>
          <p className="warn">되돌릴 수 없습니다. 다른 기기들은 예전 그룹에서 나오게 됩니다.</p>
          <div className="row">
            <button
              className="danger"
              data-testid="restart-confirm-button"
              disabled={!nameOk || busy}
              onClick={() => run("restart_group", { name })}
            >
              처음부터 다시 시작
            </button>
            <button className="secondary" disabled={busy} onClick={() => setRestart(0)}>
              취소
            </button>
          </div>
        </section>
      )}

      {error && (
        <p className="warn" data-testid="error">
          {error}
        </p>
      )}
    </main>
  );
}
