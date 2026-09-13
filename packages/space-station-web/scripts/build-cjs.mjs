import { readFile, writeFile } from 'node:fs/promises';

const source = await readFile(new URL('../index.js', import.meta.url), 'utf8');
const cjs = source.replace(
  /\nexport \{ createSpaceStationWeb, toIngestBatch \};\nexport default createSpaceStationWeb;\s*$/,
  '\nmodule.exports = { createSpaceStationWeb, toIngestBatch, default: createSpaceStationWeb };\n',
);
if (cjs === source) throw new Error('index.js export footer changed; update the CJS build script');
await writeFile(new URL('../index.cjs', import.meta.url), `'use strict';\n${cjs}`);
