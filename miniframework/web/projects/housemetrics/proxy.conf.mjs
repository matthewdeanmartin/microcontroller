// Dev-server proxy: the API and the board's own pages go to a housemetrics
// server, so the browser only talks to localhost:4200 (same origin, no CORS).
//
//   npm start                                   -> desktop server on :8080
//   HOUSEMETRICS=192.168.1.170 npm start        -> a board, plain HTTP
//   HOUSEMETRICS=https://housemetrics.local npm start
const raw = (process.env['HOUSEMETRICS'] ?? 'localhost:8080').trim();
const target = /^https?:\/\//i.test(raw) ? raw : `http://${raw}`;
console.log(`[proxy] /api, /metrics, /trust, /ca -> ${target}`);

const route = {
  target,
  // The household CA is not in Node's trust store.
  secure: false,
  changeOrigin: true,
  proxyTimeout: 60_000,
  timeout: 60_000,
};

export default {
  '/api': route,
  '/metrics': route,
  '/trust': route,
  '/ca': route,
};
