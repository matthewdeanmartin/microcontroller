import { defineConfig } from 'vitest/config';

// Plain unit tests of the UI's logic (no browser, no TestBed).
export default defineConfig({
  test: {
    include: ['src/**/*.test.ts'],
    environment: 'node',
  },
});
