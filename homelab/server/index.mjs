import http from 'node:http';
import { readFile, stat } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { networks, range, scan } from './discovery.mjs';
const seeds=JSON.parse(await readFile(new URL('./seeds.json',import.meta.url),'utf8'));
const root=path.resolve(fileURLToPath(new URL('../dist/homelab/browser/',import.meta.url)));
let job=null;
const json=(res,status,data)=>{res.writeHead(status,{'Content-Type':'application/json','Cache-Control':'no-store'});res.end(JSON.stringify(data));};
const server=http.createServer(async(req,res)=>{
  try {
    // Loopback binding plus Host/Origin checks keep network discovery local to this app.
    if(!/^127\.0\.0\.1:4201$|^localhost:4201$/.test(req.headers.host||''))return json(res,403,{error:'Local requests only'});
    if(req.headers.origin&&!['http://localhost:4201','http://127.0.0.1:4201','http://localhost:4200','http://127.0.0.1:4200'].includes(req.headers.origin))return json(res,403,{error:'Untrusted origin'});
    const url=new URL(req.url,'http://127.0.0.1:4201');
    if(req.method==='GET'&&url.pathname==='/api/config')return json(res,200,{networks:await networks(),seeds});
    if(req.method==='GET'&&url.pathname==='/api/scan')return json(res,200,job||{phase:'Idle',running:false,devices:[],checked:0,total:0,warnings:[]});
    if(req.method==='POST'&&url.pathname==='/api/scan') {
      if(job?.running)return json(res,409,{error:'A scan is already running'});
      let body='';for await(const chunk of req){body+=chunk;if(body.length>65536)return json(res,413,{error:'Request too large'});}
      const data=JSON.parse(body||'{}');range(data.cidr||'');
      const remembered=Array.isArray(data.hosts)?data.hosts.filter(h=>typeof h==='string'&&/^[a-z0-9-]+(?:\.[a-z0-9-]+)*\.local$/i.test(h)).slice(0,256):[];
      job={running:true,cancelled:false,phase:'Starting',checked:0,total:0,devices:[],warnings:[],startedAt:new Date().toISOString(),cidr:data.cidr};
      scan(job,seeds,remembered,data.cidr).catch(e=>{job.phase='Failed';job.warnings.push(e.message);}).finally(()=>{job.running=false;job.finishedAt=new Date().toISOString();});
      return json(res,202,job);
    }
    if(req.method==='POST'&&url.pathname==='/api/cancel'){if(job)job.cancelled=true;return json(res,200,{ok:true});}
    if(url.pathname.startsWith('/api/'))return json(res,404,{error:'Unknown API route'});
    if(req.method!=='GET')return json(res,405,{error:'Method not allowed'});
    let file=path.resolve(root,'.'+decodeURIComponent(url.pathname));if(!file.startsWith(root+path.sep))file=path.join(root,'index.html');
    try {if(!(await stat(file)).isFile())file=path.join(root,'index.html');}catch {file=path.join(root,'index.html');}
    const content=await readFile(file);const mime={'.html':'text/html','.js':'text/javascript','.css':'text/css','.ico':'image/x-icon','.json':'application/json'};
    res.writeHead(200,{'Content-Type':mime[path.extname(file)]||'application/octet-stream','X-Content-Type-Options':'nosniff'});res.end(content);
  }catch(e){json(res,400,{error:e.message});}
});
server.listen(4201,'127.0.0.1',()=>console.log('Homelab is ready at http://127.0.0.1:4201'));
for(const signal of ['SIGINT','SIGTERM'])process.on(signal,()=>{if(job)job.cancelled=true;server.close(()=>process.exit());});
