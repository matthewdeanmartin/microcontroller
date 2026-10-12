import test from 'node:test';
import assert from 'node:assert/strict';
import { checkDomain, inspectEndpoint } from './discovery.mjs';
import { deviceLinks } from '../src/device-links.js';
import http from 'node:http';
import { mergeDevices } from '../src/merge-devices.js';
test('Known domain attempts HTTP and HTTPS even with no discovered IP, recording failures',async()=>{
  const calls=[];
  const result=await checkDomain('nanacoin.local',undefined,async(host,port,secure,connectIp,options)=>{calls.push({host,port,secure,connectIp});options.onError('ENOTFOUND');return null;});
  assert.deepEqual(calls,[{host:'nanacoin.local',port:80,secure:false,connectIp:'nanacoin.local'},{host:'nanacoin.local',port:443,secure:true,connectIp:'nanacoin.local'}]);
  assert.equal(result.checks.length,2);assert.ok(result.checks.every(c=>c.result==='ENOTFOUND'));assert.equal(result.endpoints.length,0);
});
test('Domain results include the connected IP before the response socket is released',async()=>{
  const server=http.createServer((req,res)=>res.end('<title>Board</title>'));await new Promise(r=>server.listen(0,'127.0.0.1',r));
  try{const result=await inspectEndpoint('nanacoin.local',server.address().port,false,'127.0.0.1');assert.equal(result.ip,'127.0.0.1');}finally{await new Promise(r=>server.close(r));}
});
test('Shared summary/detail links include unprobed domains, aliases and IPs without unsafe URLs',()=>{
  const links=deviceLinks({host:'dp-4455012T.local',ip:'192.168.1.166',aliases:['printer-name.local','javascript:alert(1)'],endpoints:[]});
  for(const host of ['dp-4455012T.local','printer-name.local','192.168.1.166'])for(const protocol of ['http','https'])assert.ok(links.some(l=>l.url===`${protocol}://${host}/`));
  assert.ok(links.every(l=>!l.found&&!l.url.includes('javascript')));
  const found=deviceLinks({host:'nanacoin.local',aliases:[],ip:'192.168.1.158',endpoints:[{url:'https://nanacoin.local/',port:443,protocol:'HTTPS',status:200,contentType:'text/html'}]});
  assert.equal(found.find(l=>l.url==='https://nanacoin.local/').found,true);
});
test('Named board and IP discoveries collapse to one entry with saved notes and both URL forms',()=>{
  const base={aliases:[],endpoints:[],sources:[]};
  const previous=[{...base,id:'mastomini.local',host:'mastomini.local',notes:'Upstairs board'}, {...base,id:'192.168.1.161',host:'192.168.1.161',ip:'192.168.1.161',endpoints:[{url:'http://192.168.1.161/'}]}];
  const found=[{...base,id:'192.168.1.161',host:'mastomini.local',ip:'192.168.1.161',aliases:['mastomini.local'],endpoints:[{url:'https://mastomini.local/'}]}];
  const rows=mergeDevices(previous,found);assert.equal(rows.length,1);assert.equal(rows[0].notes,'Upstairs board');assert.equal(rows[0].host,'mastomini.local');assert.equal(rows[0].endpoints.length,2);
  assert.equal(mergeDevices([{...base,host:'a.local'},{...base,host:'b.local'}],[{...base,host:'a.local'}]).length,2);
});
