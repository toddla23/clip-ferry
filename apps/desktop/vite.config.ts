import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri가 devUrl(1420)로 접속한다. 소스는 ui/, 빌드 결과는 dist/.
export default defineConfig({
  root: "ui",
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { outDir: "../dist", emptyOutDir: true },
});
