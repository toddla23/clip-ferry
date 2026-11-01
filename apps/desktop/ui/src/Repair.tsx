import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AppView } from "./types";

// 그룹에서 제거되었거나 너무 오래 연결되지 않았을 때: 처음 화면으로 돌아가 다시 페어링한다.
export default function Repair({ view }: { view: AppView }) {
  const [confirm, setConfirm] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function leave() {
    setError(null);
    try {
      await invoke("leave_group");
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <main>
      <header>
        <span className="dot stopped" />
        <h1 data-testid="repair-title">다시 페어링 필요</h1>
      </header>
      <p className="warn" data-testid="repair-reason">
        {view.health === "removed"
          ? "이 기기는 그룹에서 제거되었습니다."
          : "너무 오래 다른 기기와 연결되지 않아 그룹의 최신 키를 받지 못했습니다."}{" "}
        더 이상 보내거나 받지 않습니다.
      </p>
      <p className="note">
        다시 쓰려면 이 기기의 그룹 정보를 지운 뒤, 이미 쓰는 기기에서 초대 코드를 받아 참여하세요.
        이 기기의 이름({view.deviceName})은 그대로입니다.
      </p>
      {!confirm ? (
        <button data-testid="repair" onClick={() => setConfirm(true)}>
          다시 페어링
        </button>
      ) : (
        <div className="row">
          <button data-testid="repair-confirm" className="danger" onClick={leave}>
            이 기기에서 그룹 정보 지우기
          </button>
          <button className="secondary" onClick={() => setConfirm(false)}>
            취소
          </button>
        </div>
      )}
      {error && <p className="warn">{error}</p>}
    </main>
  );
}
