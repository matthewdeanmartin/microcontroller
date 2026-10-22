// Dev-server proxy for the local app: the API and the board's own pages go
// to a party server, so the browser only talks to localhost:4200 (same
// origin; the server accepts its unsafe requests via its dev-origin
// allowlist).
//
//   npm start                          -> desktop server on :8090
//   PARTY=192.168.1.170 npm start      -> a board, plain HTTP
const raw = (process.env['PARTY'] ?? 'localhost:8090').trim();
const target = /^https?:\/\//i.test(raw) ? raw : `http://${raw}`;
console.log(`[proxy] /api, /trust, /ca -> ${target}`);

const route = { target, secure: false, changeOrigin: true, proxyTimeout: 30_000, timeout: 30_000 };

export default { '/api': route, '/trust': route, '/ca': route, '/metrics': route };
