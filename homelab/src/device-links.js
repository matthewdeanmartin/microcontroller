import { currentWebEndpoints } from './web-services.js';
export function deviceLinks(device) {
  const confirmed=currentWebEndpoints(device);
  const found = new Set(confirmed.map(e=>e.url));
  const urls = new Set([...found].filter(url=>/^https?:\/\//.test(url)));
  const hosts = [...new Set([device.host,...device.aliases,device.ip].filter(Boolean))];
  for (const host of hosts) {
    if (!/^[a-z0-9-]+(?:\.[a-z0-9-]+)+\.?$/i.test(host)) continue;
    urls.add(`https://${host}/`); urls.add(`http://${host}/`);
    if (host===device.ip) for (const endpoint of confirmed) {
      if (endpoint.port) urls.add(`${endpoint.protocol==='HTTPS'?'https':'http'}://${host}${[80,443].includes(endpoint.port)?'':':'+endpoint.port}/`);
    }
  }
  return [...urls].map(url=>({url,found:found.has(url),endpoint:confirmed.find(e=>e.url===url)}));
}
