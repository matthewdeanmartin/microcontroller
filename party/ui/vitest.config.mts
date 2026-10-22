import { defineConfig } from 'vitest/config';

// Plain unit tests of the UI's logic (no browser, no TestBed), across the
// local app, the companion site and party-core.
export default defineConfig({
  test: {
    include: ['projects/**/*.test.ts'],
    environment: 'node',
  },
});
