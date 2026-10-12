export function isWebEndpoint(endpoint) {
  if (!['HTTP','HTTPS'].includes(endpoint.protocol) || !Number.isInteger(endpoint.status)) return false;
  if (/AirTunes|AirPlay|RAOP/i.test(endpoint.server||'')) return false;
  return !!endpoint.title || /^(?:text\/(?:html|plain)|application\/(?:json|[^;]+\+json|xml|[^;]+\+xml|xhtml\+xml))(?:;|$)/i.test(endpoint.contentType||'') ||
    (endpoint.status>=300 && endpoint.status<400 && !!endpoint.redirect);
}
export function currentWebEndpoints(device) {
  return device.endpoints.filter(e=>e.available!==false && isWebEndpoint(e));
}
export function matchesFilter(device, filter) {
  const web=currentWebEndpoints(device).length>0;
  switch(filter) {
    case 'Online web services': return device.status==='online'&&web;
    case 'Web services': return web;
    case 'Online': return device.status==='online';
    case 'Only ping': return device.status==='online' && device.evidence==='Responded to ping' && !(device.openPorts||[]).length && !device.endpoints.some(e=>e.available!==false);
    case 'Boards': return device.kind==='Board';
    case 'Unverified': return device.status==='unverified';
    case 'Offline': return device.status==='offline';
    default: return true;
  }
}
export const FILTERS=['Online web services','All','Online','Web services','Only ping','Boards','Unverified','Offline'];
export function loadFilter(storage) {
  try {const value=storage.getItem('homelab.filter.v1');return FILTERS.includes(value)?value:'Online web services';} catch {return 'Online web services';}
}
