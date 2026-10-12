// `/api` is the backend, same-origin through this proxy; everything else is the app. The
// headers are the ones Vercel served for the Vite build, kept as they were.
import type { NextConfig } from "next";

const backend = process.env.SS_BACKEND_URL || (process.env.NODE_ENV === "production" ? "https://backend.spacestation.teamofsilicons.com" : "http://localhost:8080");
const immutable = [{ key: "Cache-Control", value: "public, max-age=31536000, immutable" }];

const config: NextConfig = {
  poweredByHeader: false,
  agentRules: false,
  skipTrailingSlashRedirect: true,
  rewrites: async () => [{ source: "/api/:path*", destination: `${backend}/api/:path*` }],
  headers: async () => [
    { source: "/brand/:path*", headers: immutable },
    {
      source: "/:path*",
      headers: [
        { key: "X-Content-Type-Options", value: "nosniff" },
        { key: "Referrer-Policy", value: "same-origin" },
        { key: "X-Frame-Options", value: "SAMEORIGIN" },
        { key: "Permissions-Policy", value: "camera=(), microphone=(), geolocation=()" },
      ],
    },
    { source: "/api/:path*", headers: [{ key: "Cache-Control", value: "private, no-store" }] },
    { source: "/api/auth/:path*", headers: [{ key: "Referrer-Policy", value: "no-referrer" }] },
  ],
};

export default config;
