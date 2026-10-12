export function reconcileSeeds(devices, seeds) {
  const current = new Map(seeds.map(s => [s.host,s]));
  return devices.filter(d => {
    const oldSeed = d.evidence?.startsWith('Repository reference') || d.kind === 'Reference';
    return !oldSeed || current.has(d.host) || !!d.ip || !!d.lastSeen;
  }).map(d => current.has(d.host) ? {...d,sources:current.get(d.host).sources} : d);
}
