# Space Station web

A minimal SolidJS + TypeScript frontend built with Vite, using Silicon IAM's shared mark,
IBM Plex typography, colors and navigation conventions. Vercel serves the frontend at
https://spacestation.teamofsilicons.com. The native Rust API lives at
https://backend.spacestation.teamofsilicons.com.

Run `npm install`, then `npm run dev` (port 3000). `SS_BACKEND_URL` selects the local API
(default `http://localhost:8080`). `npm run build` typechecks and produces `dist/`.
`npm run check-all` also runs the API, notification-definition and agent-prompt tests.

Vercel proxies `/api/*` and `/webhooks/api/` to the backend. Cookies are HttpOnly and Secure
in production. `SS_COOKIE_DOMAIN=spacestation.teamofsilicons.com` on the backend shares
the application session with the direct WebSocket host and permits CLI login to return
through the frontend. Login-state cookies remain restricted to `/api/auth`. WebSockets use `VITE_WS_URL` when supplied, otherwise the production backend
(or `ws://localhost:8080/api/ws` locally).

The pages cover organization login/switching, tables and key rotation, Space Windows and
versioned code, the sandboxed live runtime, notifications and tests, API keys and webhooks.
`docs/*.md` is bundled into `/docs`. `public/mission-control.js` is copied from the runtime
package by the build script. No application credentials are included in frontend assets.
