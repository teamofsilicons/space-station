#!/usr/bin/env node
// `space-station-dev …` is `node mission-control.js dev …`.
require("../mission-control.js").main(["dev", ...process.argv.slice(2)]);
