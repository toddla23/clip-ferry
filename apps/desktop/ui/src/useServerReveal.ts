import { useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AppView } from "./types";

// 서버 주소 칸은 기본으로 숨긴다(F43). 제목을 3초 안에 7번 누르면 보이게 한다(설정에 저장).
export function useServerReveal(view: AppView, onView: (v: AppView) => void) {
  const taps = useRef<number[]>([]);
  return () => {
    if (view.showServerSettings) return;
    const now = Date.now();
    taps.current = [...taps.current.filter((t) => now - t < 3000), now];
    if (taps.current.length >= 7) {
      taps.current = [];
      invoke<AppView>("show_server_settings").then(onView).catch(() => {});
    }
  };
}
