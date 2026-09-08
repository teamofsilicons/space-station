import { defineConfig } from "vite";
import solid from "vite-plugin-solid";
export default defineConfig({
  plugins: [solid()],
  server: {
    proxy: {
      "/api": {
        target: process.env.SS_BACKEND_URL || "http://localhost:8080",
        changeOrigin: true,
      },
      "/webhooks/api": {
        target: process.env.SS_BACKEND_URL || "http://localhost:8080",
        changeOrigin: true,
      },
    },
  },
});
