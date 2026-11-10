// src-tauri/src/view.rs 의 AppView 와 같은 모양이다.
export type Connection = "connected" | "reconnecting" | "disconnected" | "stopped";
export type Health = "active" | "removed" | "stale";

export interface DeviceView {
  id: string;
  name: string;
  isSelf: boolean;
  online: boolean;
}

export interface Qr {
  width: number;
  path: string;
}

export interface PairingView {
  kind: "invite" | "join";
  phase: "connecting" | "waiting" | "request" | "done" | "failed";
  textCode: string | null;
  qr: Qr | null;
  expiresAtMs: number | null;
  peerName: string | null;
  sas: string | null;
  message: string | null;
}

export interface AppView {
  seq: number;
  stage: "setup" | "ready";
  connection: Connection;
  deviceName: string;
  deviceId: string;
  epoch: number | null;
  memberCount: number;
  devices: DeviceView[];
  health: Health;
  relayUrl: string;
  error: string | null;
  pairing: PairingView | null;
  defaultName: string;
  sendEnabled: boolean;
  receiveEnabled: boolean;
  paused: boolean;
  autostart: boolean;
  account: AccountView | null;
  accountUrl: string;
  loginBusy: boolean;
  loginMessage: string | null;
  devLogin: boolean;
  /** 서버 주소 칸을 보인다 (디버그 빌드이거나 제목을 7번 눌렀을 때, F43). 아니면 주소가 빈 문자열로 온다 */
  showServerSettings: boolean;
  linkRequest: LinkRequestView | null;
}

export interface LinkRequestView {
  name: string;
  expiresAtMs: number;
}

export interface AccountView {
  email: string;
  name: string;
}

export const CONNECTION_LABEL: Record<Connection, string> = {
  connected: "연결됨",
  reconnecting: "재연결 중",
  disconnected: "연결 끊김",
  stopped: "중지됨",
};
