import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    include: ['projects/**/*.test.ts'],
    environment: 'node',
  },
});
