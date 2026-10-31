// src-tauri/src/view.rs 의 AppView 와 같은 모양이다.
export type Connection = "connected" | "reconnecting" | "disconnected" | "stopped";
export type Health = "active" | "removed" | "stale";

export interface DeviceView {
  id: string;
  name: string;
  isSelf: boolean;
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
}

export const CONNECTION_LABEL: Record<Connection, string> = {
  connected: "연결됨",
  reconnecting: "재연결 중",
  disconnected: "연결 끊김",
  stopped: "중지됨",
};
