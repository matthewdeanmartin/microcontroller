import os from 'node:os';
import net from 'node:net';
import http from 'node:http';
import https from 'node:https';
import dns from 'node:dns/promises';
import dnsCallback from 'node:dns';
import { getCACertificates } from 'node:tls';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import mdns from 'multicast-dns';
const exec = promisify(execFile);
const trustRoots = [...new Set([...getCACertificates('default'), ...getCACertificates('system')])];
const timeout = (promise, ms, fallback) => Promise.race([promise.catch(() => fallback), new Promise(resolve => {const t=setTimeout(()=>resolve(fallback),ms);t.unref();})]);
export const privateIP = ip => /^(10\.|192\.168\.|172\.(1[6-9]|2\d|3[01])\.)/.test(ip) && net.isIP(ip) === 4;
const number = ip => ip.split('.').reduce((n,x)=>(n*256+Number(x))>>>0,0);
const address = n => [24,16,8,0].map(s=>(n>>>s)&255).join('.');
export function range(cidr) {
  const [ip,bitsText] = cidr.split('/'); const bits = Number(bitsText);
  if (!privateIP(ip) || !/^\d+$/.test(bitsText || '') || bits < 16 || bits > 32) throw Error('Choose a private IPv4 subnet between /16 and /32.');
  const mask = bits === 0 ? 0 : (0xffffffff << (32-bits))>>>0;
  const base = (number(ip)&mask)>>>0; const size = 2**(32-bits);
  const start = size > 2 ? 1 : 0; const end = size > 2 ? size-1 : size;
  const ips = Array.from({length:end-start},(_,i)=>address(base+i+start));
  if (!ips.every(privateIP)) throw Error('Entire subnet must be private.');
  return ips;
}
export async function networks() {
  let gateways=[];
  try {
    if (process.platform === 'win32') {const {stdout}=await exec('powershell.exe',['-NoProfile','-NonInteractive','-Command','Get-NetRoute -DestinationPrefix 0.0.0.0/0 | Select-Object -ExpandProperty NextHop'],{timeout:5000});gateways=stdout.trim().split(/\s+/).filter(privateIP);}
    else if (process.platform === 'linux') {const {stdout}=await exec('ip',['route'],{timeout:2000});gateways=[...stdout.matchAll(/default via ([\d.]+)/g)].map(m=>m[1]);}
    else {const {stdout}=await exec('route',['-n','get','default'],{timeout:2000});gateways=[stdout.match(/gateway:\s*([\d.]+)/)?.[1]].filter(Boolean);}
  } catch {}
  return Object.entries(os.networkInterfaces()).flatMap(([name,list])=>(list||[]).filter(a=>a.family==='IPv4'&&!a.internal&&privateIP(a.address)).map(a=>({name,address:a.address,cidr:a.cidr,gateway:gateways.find(g=>{const b=Number(a.cidr.split('/')[1]);const mask=(0xffffffff<<(32-b))>>>0;return (number(g)&mask)===(number(a.address)&mask);})||null}))).sort((a,b)=>Number(!!b.gateway)-Number(!!a.gateway));
}
export function portOpen(host,port,ms=700) {return new Promise(resolve=>{const s=net.connect({host,port});const done=ok=>{s.destroy();resolve(ok);};s.setTimeout(ms,()=>done(false));s.once('connect',()=>done(true));s.once('error',()=>done(false));});}
export function inspectEndpoint(host,port,secure,connectIp=host,options={}) {
  return new Promise(resolve=>{
    let finished=false;let deadline;const done=x=>{if(!finished){finished=true;clearTimeout(deadline);resolve(x);}};
    const url=`${secure?'https':'http'}://${host}${port===(secure?443:80)?'':':'+port}/`;
    const req=(secure?https:http).get(url,{hostname:connectIp,servername:net.isIP(host)?undefined:host,ca:trustRoots,lookup:(name,opts,cb)=>dnsCallback.lookup(name,{family:4},(error,ip)=>{if(error)return cb(error);if(!privateIP(ip))return cb(Error('Domain resolved outside private IPv4 networks'));cb(null,opts.all?[{address:ip,family:4}]:ip,4);}),timeout:1800,rejectUnauthorized:false,headers:{Host:host+(port===(secure?443:80)?'':':'+port),'User-Agent':'Homelab-local-discovery/1.0','Accept':'text/html,application/json'}},res=>{
      const socket=res.socket;const cert=secure?socket.getPeerCertificate():null;const remoteIP=socket.remoteAddress?.replace(/^::ffff:/,'');const tls=secure?{trusted:!!socket.authorized,error:socket.authorizationError||null,subject:cert?.subject?.CN||null,issuer:cert?.issuer?.CN||null,expires:cert?.valid_to||null}:null;
      let body='';let bytes=0;
      res.on('data',chunk=>{bytes+=chunk.length;if(bytes<=32768) body+=chunk.toString();else {finish();res.destroy();}});
      const finish=()=>{done({url,ip:remoteIP,port,protocol:secure?'HTTPS':'HTTP',status:res.statusCode,checkedAt:new Date().toISOString(),available:true,title:body.match(/<title[^>]*>([^<]*)<\/title>/i)?.[1]?.trim().slice(0,160)||null,server:res.headers.server||null,contentType:res.headers['content-type']||null,redirect:res.headers.location||null,tls});};
      res.on('end',finish);res.on('error',()=>done(null));
    });
    deadline=setTimeout(()=>{options.onError?.('Request deadline exceeded');req.destroy();done(null);},options.deadline||2500);
    req.on('timeout',()=>{options.onError?.('Connection timed out');req.destroy();done(null);});req.on('error',e=>{options.onError?.(e.code||e.message);done(null);});
  });
}
async function neighbors() {
  try {const {stdout}=await exec('arp',['-a'],{timeout:3000});return [...stdout.matchAll(/(\d+\.\d+\.\d+\.\d+)[^\r\n]*?([a-f\d]{2}(?:[:-][a-f\d]{2}){5})/gi)].filter(m=>privateIP(m[1])&&!/^ff[:-]ff/i.test(m[2])).map(m=>({ip:m[1],mac:m[2]}));}catch{return [];}
}
export async function checkDomain(host, knownIp, probe=inspectEndpoint) {
  const checks=[];
  const endpoints=(await Promise.all([80,443].map(async port=>{
    let error=null;const result=await probe(host,port,port===443,knownIp||host,{deadline:8000,onError:e=>{error=e;}});
    checks.push({url:`${port===443?'https':'http'}://${host}/`,checkedAt:new Date().toISOString(),result:result?`HTTP ${result.status}`:error||'No HTTP response'});
    return result;
  }))).filter(Boolean);
  return {checks,endpoints};
}
export async function scan(job, seeds, remembered, cidr) {
  const ips=range(cidr);const allowed=new Set(ips);const nets=await networks();
  const names=new Map();const services=new Map();let multicast;
  job.phase='Resolving names';job.total=ips.length;
  try {
    const network=nets.find(n=>allowed.has(n.address));
    multicast=mdns({interface:network?.address,bind:'0.0.0.0'});multicast.on('error',e=>{job.warnings.push(`mDNS: ${e.message}`);});
    const queried=new Set();
    multicast.on('response',packet=>{for(const r of [...(packet.answers||[]),...(packet.additionals||[])]) {
      if (r.type==='A'&&allowed.has(r.data)) {const hosts=names.get(r.data)||[];if(!hosts.includes(r.name))hosts.push(r.name);names.set(r.data,hosts);}
      if(r.type==='PTR'&&typeof r.data==='string'&&r.data.endsWith('.local')&&!queried.has(r.data)){queried.add(r.data);multicast.query([{name:r.data,type:'SRV'},{name:r.data,type:'PTR'}]);}
      if(r.type==='SRV'&&r.data.port>0&&r.data.port<=65535) {const list=services.get(r.data.target)||[];if(!list.some(s=>s.name===r.name&&s.port===r.data.port))list.push({name:r.name,port:r.data.port});services.set(r.data.target,list);if(!queried.has(r.data.target)){queried.add(r.data.target);multicast.query([{name:r.data.target,type:'A'}]);}}
    }});
    multicast.query([{name:'_http._tcp.local',type:'PTR'},{name:'_https._tcp.local',type:'PTR'},{name:'_workstation._tcp.local',type:'PTR'},{name:'_services._dns-sd._udp.local',type:'PTR'},...seeds.map(s=>({name:s.host,type:'A'})),...remembered.filter(h=>h.endsWith('.local')).map(name=>({name,type:'A'}))]);
  } catch(e) {job.warnings.push(`mDNS unavailable: ${e.message}`);}
  try {
    await Promise.all([...new Set([...seeds.map(s=>s.host),...remembered])].map(async host=>{const result=await timeout(dns.lookup(host,{family:4}),1800,null);if(result&&allowed.has(result.address)){const list=names.get(result.address)||[];if(!list.includes(host))list.push(host);names.set(result.address,list);}}));
    job.phase='Checking addresses';
    let cursor=0;
    await Promise.all(Array.from({length:24},async()=>{while(cursor<ips.length&&!job.cancelled){const ip=ips[cursor++];
      const advertised=(names.get(ip)||[]).flatMap(h=>services.get(h)||[]);
      const ports=[...new Set([80,443,8080,8443,8000,3000,22,445,3389,...advertised.map(s=>s.port)])];
      const open=(await Promise.all(ports.map(async port=>await portOpen(ip,port)?port:null))).filter(Boolean);
      let alive=open.length>0;
      if(!alive) {try {const args=process.platform==='win32'?['-n','1','-w','650',ip]:['-c','1','-W','1',ip];const {stdout}=await exec('ping',args,{timeout:1700});alive=/ttl[= ]/i.test(stdout);}catch{}}
      if(alive || names.has(ip) || nets.some(n=>n.address===ip)) {
        const hosts=names.get(ip)||[];const reverse=await timeout(dns.reverse(ip),450,[]);for(const h of reverse)if(!hosts.includes(h))hosts.push(h);
        const primary=hosts.find(h=>seeds.some(s=>s.host===h))||hosts[0]||ip;
        const endpoints=(await Promise.all(open.filter(p=>![22,445,3389].includes(p)).map(p=>inspectEndpoint(primary,p,p===443||p===8443||advertised.some(s=>s.port===p&&s.name.includes('_https')),ip)))).filter(Boolean);
        const gateway=nets.some(n=>n.gateway===ip);const local=nets.some(n=>n.address===ip);const board=/espressif|esp32|nanacoin|mastomini|minicloud|housemetrics|secrets/i.test(hosts.join(' ')+' '+endpoints.map(e=>e.title).join(' '));
        job.devices.push({id:ip,ip,host:primary,aliases:hosts,endpoints,openPorts:open,services:advertised,kind:gateway?'Router':local?'This PC':board?'Board':open.some(p=>[445,3389].includes(p))?'Computer':'Device',status:alive||local?'online':'observed',evidence:local?'Local network interface':open.length?'Responded on TCP':alive?'Responded to ping':'mDNS advertisement',lastSeen:new Date().toISOString(),sources:seeds.filter(s=>hosts.includes(s.host)).flatMap(s=>s.sources)});
      }
      job.checked++;
    }}));
    job.phase='Checking known domains';
    const domainHosts=[...new Set([...seeds.map(s=>s.host),...remembered])];let domainCursor=0;
    job.domainsChecked=0;job.domainsTotal=domainHosts.length;
    await Promise.all(Array.from({length:6},async()=>{while(domainCursor<domainHosts.length&&!job.cancelled){
      const host=domainHosts[domainCursor++];const knownIp=[...names].find(([,hosts])=>hosts.includes(host))?.[0];
      const {checks,endpoints}=await checkDomain(host,knownIp);
      const ip=endpoints.find(e=>e.ip)?.ip||knownIp;
      let device=job.devices.find(d=>(ip&&d.ip===ip)||d.host===host||d.aliases.includes(host));
      if(device){device.checks=[...(device.checks||[]),...checks];device.aliases=[...new Set([...device.aliases,host])];device.endpoints=[...new Map([...device.endpoints,...endpoints].map(e=>[e.url,e])).values()];if(endpoints.length){if(net.isIP(device.host))device.host=host;device.status='online';device.lastSeen=new Date().toISOString();}}
      else {device={id:ip||host,ip,host,aliases:[host],endpoints,openPorts:endpoints.map(e=>e.port),kind:endpoints.length?'Board':'Reference',status:endpoints.length?'online':'unverified',evidence:endpoints.length?'Responded to named HTTP/HTTPS probe':'Known domain HTTP/HTTPS attempts did not respond',lastSeen:endpoints.length?new Date().toISOString():null,checks,sources:seeds.find(s=>s.host===host)?.sources||[]};job.devices.push(device);}
      device.sources=[...new Set([...device.sources,...(seeds.find(s=>s.host===host)?.sources||[])])];
      job.domainsChecked++;
    }}));
    job.phase='Reading neighbors';
    for(const n of await neighbors()) if(allowed.has(n.ip)) {const existing=job.devices.find(d=>d.ip===n.ip);if(existing)existing.mac=n.mac;else job.devices.push({id:n.ip,ip:n.ip,host:n.ip,aliases:[],endpoints:[],openPorts:[],kind:nets.some(x=>x.gateway===n.ip)?'Router':'Device',status:'observed',evidence:'Neighbor cache (may be stale)',mac:n.mac,lastSeen:null,sources:[]});}
    job.phase=job.cancelled?'Cancelled':'Complete';
  } finally {multicast?.destroy();}
}
