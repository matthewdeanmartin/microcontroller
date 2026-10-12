import { existsSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
if (!existsSync(new URL('../dist/homelab/browser/index.html', import.meta.url))) {
  const result = spawnSync(process.execPath, ['node_modules/@angular/cli/bin/ng.js', 'build'], {stdio:'inherit'});
  if (result.status !== 0) process.exit(result.status || 1);
}
await import('../server/index.mjs');
