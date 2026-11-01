import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AppView, DeviceView } from "./types";

// 기기 목록: 온라인 표시, 이 기기 이름 변경, 다른 기기 제거(확인 후).
export default function Devices({
  view,
  onView,
  onError,
}: {
  view: AppView;
  onView: (v: AppView) => void;
  onError: (message: string) => void;
}) {
  const [renaming, setRenaming] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [removing, setRemoving] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function run(cmd: string, args: Record<string, unknown>, done: () => void) {
    setBusy(true);
    try {
      onView(await invoke<AppView>(cmd, args));
      done();
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  }

  const row = (d: DeviceView) => (
    <li key={d.id} data-testid={`device-${d.id}`}>
      <span
        className={`dot sm ${d.online ? "online" : ""}`}
        title={d.online ? "온라인" : "오프라인"}
        data-testid="presence"
        data-online={d.online}
      />
      {renaming === d.id ? (
        <>
          <input
            data-testid="rename-input"
            value={name}
            maxLength={40}
            autoFocus
            onChange={(e) => setName(e.target.value)}
          />
          <button
            data-testid="rename-save"
            disabled={busy || name.trim() === ""}
            onClick={() => run("rename_device", { name }, () => setRenaming(null))}
          >
            저장
          </button>
          <button className="secondary" disabled={busy} onClick={() => setRenaming(null)}>
            취소
          </button>
        </>
      ) : removing === d.id ? (
        <>
          <span className="name warn">{d.name}을(를) 제거할까요?</span>
          <button
            data-testid="remove-confirm"
            className="danger"
            disabled={busy}
            onClick={() => run("remove_device", { id: d.id }, () => setRemoving(null))}
          >
            제거
          </button>
          <button className="secondary" disabled={busy} onClick={() => setRemoving(null)}>
            취소
          </button>
        </>
      ) : (
        <>
          <span className="name">{d.name}</span>
          {d.isSelf && <span className="badge">이 기기</span>}
          <span className="muted">{d.id}</span>
          {d.isSelf ? (
            <button
              className="link"
              data-testid="rename"
              onClick={() => {
                setName(d.name);
                setRenaming(d.id);
              }}
            >
              이름 변경
            </button>
          ) : (
            <button className="link" data-testid="remove" onClick={() => setRemoving(d.id)}>
              제거
            </button>
          )}
        </>
      )}
    </li>
  );

  return (
    <ul className="devices" data-testid="devices">
      {view.devices.map(row)}
    </ul>
  );
}
