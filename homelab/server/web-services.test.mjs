import test from 'node:test';
import assert from 'node:assert/strict';
import { FILTERS, loadFilter, matchesFilter, isWebEndpoint } from '../src/web-services.js';
import { mergeDevices } from '../src/merge-devices.js';
import { deviceLinks } from '../src/device-links.js';
const base={id:'192.168.1.187',host:'192.168.1.187',ip:'192.168.1.187',aliases:[],sources:[],status:'online',kind:'Device',endpoints:[],openPorts:[],evidence:'Responded to ping'};
const media={url:'http://Tanos-Laptop.local:7000/',protocol:'HTTP',port:7000,status:403,server:'AirTunes/870.14.1',available:true};
test('Only ping excludes TCP and other HTTP responders and includes devices with stale web history',()=>{
  assert.ok(FILTERS.includes('Only ping'));assert.equal(matchesFilter(base,'Only ping'),true);
  assert.equal(matchesFilter({...base,openPorts:[22]},'Only ping'),false);
  assert.equal(matchesFilter({...base,endpoints:[media]},'Only ping'),false);
  assert.equal(matchesFilter({...base,endpoints:[{...media,available:false}]},'Only ping'),true);
});
test('Default and saved filters survive page recreation; web services require current browser/API content',()=>{
  assert.equal(loadFilter({getItem:()=>null}),'Online web services');
  assert.equal(loadFilter({getItem:()=> 'Only ping'}),'Only ping');
  assert.equal(loadFilter({getItem:()=> 'bogus'}),'Online web services');
  assert.equal(isWebEndpoint(media),false);assert.equal(matchesFilter({...base,endpoints:[media]},'Web services'),false);
  const html={...media,server:'board',status:200,contentType:'text/html'};
  assert.equal(matchesFilter({...base,endpoints:[html]},'Online web services'),true);
  assert.equal(matchesFilter({...base,status:'offline',endpoints:[html]},'Online web services'),false);
  assert.equal(matchesFilter({...base,endpoints:[{...html,available:false}]},'Web services'),false);
});
test('Ping-only fresh scans invalidate old HTTP endpoints and shortcuts never claim confirmation',()=>{
  const old={...base,endpoints:[media],lastSeen:'2026-10-12T12:00:00Z'};
  const current=mergeDevices([old],[base])[0];assert.equal(current.endpoints[0].available,false);
  assert.equal(matchesFilter(current,'Only ping'),true);assert.equal(matchesFilter(current,'Web services'),false);
  assert.ok(deviceLinks(current).every(l=>!l.found));
  assert.equal(mergeDevices([old],[{...base,status:'observed',evidence:'Neighbor cache (may be stale)'}])[0].status,'offline');
});
