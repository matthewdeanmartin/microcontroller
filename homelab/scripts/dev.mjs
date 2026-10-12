import { spawn } from 'node:child_process';
const server = spawn(process.execPath, ['server/index.mjs'], {stdio:'inherit'});
const ui = spawn(process.execPath, ['node_modules/@angular/cli/bin/ng.js', 'serve'], {stdio:'inherit'});
function stop() {server.kill(); ui.kill();}
process.on('SIGINT', stop); process.on('SIGTERM', stop);
ui.on('exit', code => {stop(); process.exit(code || 0);});
