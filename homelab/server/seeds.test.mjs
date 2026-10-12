import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { buildSeeds } from '../scripts/seed-policy.mjs';
import { reconcileSeeds } from '../src/seed-migration.js';
test('Only reviewed operational names are seeded, with eligible evidence',()=>{
  const manifest=JSON.parse(readFileSync(new URL('../scripts/seed-manifest.json',import.meta.url),'utf8'));
  const actual=buildSeeds(manifest,p=>readFileSync(new URL(p,new URL('../',import.meta.url)),'utf8'));
  assert.deepEqual(actual,JSON.parse(readFileSync(new URL('./seeds.json',import.meta.url),'utf8')));
  for(const h of ['other.local','board.local','peer.local','example.local','garage.local','printer.local'])assert.ok(!actual.some(s=>s.host===h));
  for(const source of ['../tests/config.rs','../node_modules/pkg/config.js','../src/app.test.ts','../examples/README.md','../archive/README.md'])assert.throws(()=>buildSeeds([{host:'other.local',sources:[source]}],()=> 'other.local'));
});
test('Removes obsolete seed-only browser entries while retaining discovered/manual devices and notes',()=>{
  const rows=[{host:'other.local',kind:'Reference',evidence:'Repository reference; may be an example or test fixture'}, {host:'other.local',ip:'192.168.1.2',kind:'Device',notes:'Real device'}, {host:'custom.local',evidence:'Added manually',notes:'Keep me'}, {host:'mastomini.local',kind:'Reference',sources:['old/test.rs'],notes:'Keep board notes'}];
  const next=reconcileSeeds(rows,[{host:'mastomini.local',sources:['config.json']}]);
  assert.equal(next.length,3);assert.equal(next[0].notes,'Real device');assert.equal(next[1].notes,'Keep me');assert.equal(next[2].notes,'Keep board notes');assert.deepEqual(next[2].sources,['config.json']);
});
