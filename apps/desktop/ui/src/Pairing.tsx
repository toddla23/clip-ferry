import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { PairingView } from "./types";

const spaced = (sas: string) => `${sas.slice(0, 3)} ${sas.slice(3)}`;

function useNow(intervalMs: number) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(t);
  }, [intervalMs]);
  return now;
}

function Remaining({ expiresAtMs }: { expiresAtMs: number }) {
  const left = Math.max(0, Math.ceil((expiresAtMs - useNow(500)) / 1000));
  const mm = Math.floor(left / 60);
  const ss = String(left % 60).padStart(2, "0");
  return (
    <span data-testid="remaining">{left === 0 ? "만료됨" : `남은 시간 ${mm}:${ss}`}</span>
  );
}

function Sas({ value }: { value: string }) {
  return (
    <div className="sas" data-testid="sas">
      {spaced(value)}
    </div>
  );
}

const close = () => invoke("close_pairing");

// 다른 기기를 초대하는 화면 (QR + 코드 → 요청 확인 → 결과).
export function InviteScreen({ p }: { p: PairingView }) {
  const [copied, setCopied] = useState(false);

  async function copy() {
    try {
      await navigator.clipboard.writeText(p.textCode ?? "");
      setCopied(true);
    } catch {
      setCopied(false);
    }
  }

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

  return (
    <main>
      <h1>기기 추가</h1>
      <p className="note">
        새 기기에서 clip-ferry를 열고 QR을 스캔하거나 아래 코드를 입력하세요. 코드는 한 번만, 10분
        동안 쓸 수 있습니다.
      </p>
      {p.qr && (
        <svg
          className="qr"
          viewBox={`-2 -2 ${p.qr.width + 4} ${p.qr.width + 4}`}
          shapeRendering="crispEdges"
          role="img"
          aria-label="초대 QR 코드"
        >
          <rect x={-2} y={-2} width={p.qr.width + 4} height={p.qr.width + 4} fill="#fff" />
          <path d={p.qr.path} fill="#000" />
        </svg>
      )}
      <textarea className="code" readOnly rows={3} value={p.textCode ?? ""} data-testid="code" />
      <div className="row between">
        <button className="secondary" onClick={copy}>
          {copied ? "복사했습니다" : "코드 복사"}
        </button>
        {p.expiresAtMs && <Remaining expiresAtMs={p.expiresAtMs} />}
      </div>
      <button className="secondary" data-testid="cancel" onClick={close}>
        취소
      </button>
    </main>
  );
}

// 초대 코드로 참여하는 화면 (연결 → 확인 숫자 → 결과).
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
