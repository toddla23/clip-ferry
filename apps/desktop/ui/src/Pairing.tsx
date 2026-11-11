import { invoke } from "@tauri-apps/api/core";
import type { PairingView } from "./types";

const spaced = (sas: string) => `${sas.slice(0, 3)} ${sas.slice(3)}`;

function Sas({ value }: { value: string }) {
  return (
    <div className="sas" data-testid="sas">
      {spaced(value)}
    </div>
  );
}

const close = () => invoke("close_pairing");

// 같은 계정의 새 기기를 받아들이는 화면 (요청 확인 → 결과, F37).
export function InviteScreen({ p }: { p: PairingView }) {
  if (p.phase === "request") {
    return (
      <main>
        <h1>참여 요청</h1>
        <p>
          <b>{p.peerName}</b> 기기가 이 그룹에 참여하려고 합니다. 그 기기 화면에 아래와 같은 숫자가
          떠 있을 때만 승인하세요.
        </p>
        <Sas value={p.sas ?? ""} />
        <div className="row">
          <button data-testid="approve" onClick={() => invoke("decide_invite", { approve: true })}>
            같음, 승인
          </button>
          <button
            className="secondary"
            data-testid="reject"
            onClick={() => invoke("decide_invite", { approve: false })}
          >
            거절
          </button>
        </div>
      </main>
    );
  }

  if (p.phase === "done") {
    return (
      <main>
        <h1>기기를 추가했습니다</h1>
        <p>
          <b>{p.peerName}</b> 기기가 그룹에 들어왔습니다.
        </p>
        {p.message && <p className="warn">{p.message}</p>}
        <button data-testid="ok" onClick={close}>
          확인
        </button>
      </main>
    );
  }

  if (p.phase === "failed") {
    return (
      <main>
        <h1>초대를 마쳤습니다</h1>
        <p className="warn" data-testid="message">
          {p.message}
        </p>
        <button data-testid="ok" onClick={close}>
          닫기
        </button>
      </main>
    );
  }

  // 계정 연결(F37): 새 기기의 응답을 기다린다.
  return (
    <main>
      <h1>기기 연결</h1>
      <p className="note" data-testid="message">
        <b>{p.peerName}</b> 기기에 연결하는 중입니다. {p.message}
      </p>
      <button className="secondary" data-testid="cancel" onClick={close}>
        취소
      </button>
    </main>
  );
}

// 같은 계정의 다른 기기로 참여하는 화면 (연결 → 확인 숫자 → 결과).
export function JoinScreen({ p }: { p: PairingView }) {
  if (p.phase === "failed") {
    return (
      <main>
        <h1>참여하지 못했습니다</h1>
        <p className="warn" data-testid="message">
          {p.message}
        </p>
        <button data-testid="back" onClick={close}>
          돌아가기
        </button>
      </main>
    );
  }
  return (
    <main>
      <h1>그룹에 참여하는 중</h1>
      {p.phase === "waiting" && p.sas ? (
        <>
          <p>
            <b>{p.peerName}</b> 기기 화면에 아래와 같은 숫자가 떠 있는지 확인하세요. 같으면 그
            기기에서 승인할 때까지 기다리면 됩니다.
          </p>
          <Sas value={p.sas} />
        </>
      ) : (
        <p className="note">{p.message ?? "릴레이에 연결하는 중입니다"}</p>
      )}
      <button className="secondary" data-testid="cancel" onClick={close}>
        취소
      </button>
    </main>
  );
}
