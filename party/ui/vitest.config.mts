import { defineConfig } from 'vitest/config';
import { fileURLToPath } from 'node:url';

// Plain unit tests of the UI's logic (no browser, no TestBed), across the
// local app, the companion site and party-core.
export default defineConfig({
  resolve: {
    alias: {
      'miniframework-ng': fileURLToPath(new URL('../../miniframework/web/projects/miniframework-ng/src/index.ts', import.meta.url)),
      'party-core': fileURLToPath(new URL('./projects/party-core/src/index.ts', import.meta.url)),
    },
  },
  test: {
    include: ['projects/**/*.test.ts'],
    environment: 'node',
  },
});
