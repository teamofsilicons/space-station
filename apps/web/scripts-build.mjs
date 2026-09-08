import { copyFileSync, existsSync } from 'node:fs';
// Keep the browser host and the runtime distributed in the Rust crate in sync.
const source = new URL('../../packages/space-station/mission-control.js', import.meta.url);
if (existsSync(source)) copyFileSync(source, new URL('./public/mission-control.js', import.meta.url));
