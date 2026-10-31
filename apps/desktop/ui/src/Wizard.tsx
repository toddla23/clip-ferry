import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AppView } from "./types";

// 첫 실행: 기기 이름을 정하고 새 그룹을 만들거나 초대 코드로 기존 그룹에 참여한다.
export default function Wizard({ view }: { view: AppView }) {
  // 기기가 이미 만들어져 있으면(참여가 중간에 끊긴 경우) 이름은 그대로 쓴다.
  const fixedName = view.deviceName !== "";
  const [name, setName] = useState(view.deviceName || view.defaultName);
  const [mode, setMode] = useState<"choose" | "join">("choose");
  const [code, setCode] = useState("");
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

  return (
    <main>
      <h1>clip-ferry 시작하기</h1>
      <p className="note">
        이 기기의 이름을 정하고, 새 그룹을 만들거나 이미 쓰는 기기의 초대 코드로 참여하세요.
      </p>

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

      {mode === "choose" ? (
        <div className="stack">
          <button
            data-testid="new-group"
            disabled={!nameOk || busy}
            onClick={() => run("create_group", { name })}
          >
            새 그룹 만들기
          </button>
          <button
            className="secondary"
            data-testid="to-join"
            disabled={!nameOk || busy}
            onClick={() => setMode("join")}
          >
            코드로 참여
          </button>
        </div>
      ) : (
        <>
          <label>
            초대 코드
            <textarea
              data-testid="join-code"
              rows={3}
              value={code}
              placeholder="다른 기기의 기기 추가 화면에 있는 코드를 붙여넣으세요"
              spellCheck={false}
              onChange={(e) => setCode(e.target.value)}
            />
          </label>
          <div className="row">
            <button
              data-testid="join"
              disabled={!nameOk || code.trim() === "" || busy}
              onClick={() => run("start_join", { name, code })}
            >
              참여
            </button>
            <button className="secondary" disabled={busy} onClick={() => setMode("choose")}>
              뒤로
            </button>
          </div>
        </>
      )}
      {error && (
        <p className="warn" data-testid="error">
          {error}
        </p>
      )}
    </main>
  );
}
