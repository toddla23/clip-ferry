import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import Home from "./Home";
import { InviteScreen, JoinScreen } from "./Pairing";
import Wizard from "./Wizard";
import type { AppView } from "./types";

export default function App() {
  const [view, setView] = useState<AppView | null>(null);
  // 응답과 이벤트는 도착 순서가 바뀔 수 있다. 번호가 더 큰(나중에 만든) 상태만 받아들인다.
  const lastSeq = useRef(0);

  const apply = useCallback((v: AppView) => {
    if (v.seq <= lastSeq.current) return;
    lastSeq.current = v.seq;
    setView(v);
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

  if (!view) return <main />;

  const p = view.pairing;
  if (p?.kind === "join") return <JoinScreen p={p} />;
  if (view.stage === "setup") return <Wizard view={view} />;
  if (p?.kind === "invite") return <InviteScreen p={p} />;
  return <Home view={view} onView={apply} />;
}
