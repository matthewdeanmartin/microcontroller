import { readFileSync, writeFileSync } from 'node:fs';
import { buildSeeds } from './seed-policy.mjs';
const manifest = JSON.parse(readFileSync(new URL('./seed-manifest.json', import.meta.url), 'utf8'));
const seeds = buildSeeds(manifest, source => readFileSync(new URL(source, new URL('../', import.meta.url)), 'utf8'));
writeFileSync(new URL('../server/seeds.json', import.meta.url), JSON.stringify(seeds,null,2)+'\n');
console.log(`Saved ${seeds.length} reviewed operational seed names.`);
