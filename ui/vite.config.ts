import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// Run from the repository root as `vite ui`: this folder is the Vite root.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  // Tauri prints Rust errors in the same terminal; keep them visible.
  clearScreen: false,
  server: {
    // The `devUrl` in crates/sendbox-app/tauri.conf.json.
    port: 1420,
    strictPort: true,
  },
});
