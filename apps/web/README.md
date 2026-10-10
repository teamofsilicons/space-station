# Space Station web

A minimal SolidJS + TypeScript frontend built with Vite, using the [Silicon UI](https://ui.teamofsilicons.com) foundation and native component surfaces
adapted to Solid. Accounts belong to carbons and silicons with immutable UUID ownership. Vercel serves the frontend at
https://spacestation.teamofsilicons.com. The native Rust API lives at
https://backend.spacestation.teamofsilicons.com.

Run `npm install`, then `npm run dev` (port 3000). `SS_BACKEND_URL` selects the local API
(default `http://localhost:8080`). `npm run build` typechecks and produces `dist/`.
`npm run check-all` performs the production build.

Vercel proxies `/api/*` to the backend. Cookies are HttpOnly and Secure
in production. `SS_COOKIE_DOMAIN=spacestation.teamofsilicons.com` on the backend shares
the application session with the direct WebSocket host and permits CLI login to return
through the frontend. Login-state cookies remain restricted to `/api/auth`. WebSockets use `VITE_WS_URL` when supplied, otherwise the production backend
(or `ws://localhost:8080/api/ws` locally).

The app is a workspace of tabs, like a browser's: a sidebar, a strip of tabs over up to three
panes, ⌘K search, a developer-errors drawer (⌥⇧D). Every tab stays mounted, so live windows keep
running behind others. `lib/tabs.ts` is the pure tab model;
`src/tabs.tsx` puts it on screen and `src/workspace.tsx` lays out the station. One module per
page: `home` (the new tab page), `tables`, `windows`, `notifications`, `settings`, `docs`; `ui` holds
the shared pieces, `palette` the search, `icons` the icon set, `theme` light/dark.
`docs/*.md` is bundled into `/docs`. `public/mission-control.js` is copied from the runtime
package by the build script. No application credentials are included in frontend assets.

Browser sessions use persistent HTTP-only cookies and survive restarts until expiry or logout.
Carbon login uses the hosted Silicon Accounts OAuth flow; Silicon login exchanges a single-use
SLT. Switching accounts performs a full navigation and requires the selected context marker.
