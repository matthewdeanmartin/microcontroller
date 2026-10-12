// Explicitly reviewed operational hosts avoid tests embedded in production files too.
export function buildSeeds(manifest, readSource) {
  return manifest.map(({host,sources}) => {
    if (!/^[a-z0-9-]+\.local$/.test(host)) throw Error(`Invalid seed hostname: ${host}`);
    if (!sources?.length) throw Error(`Missing source for ${host}`);
    for (const source of sources) {
      if (/(?:^|\/)(?:node_modules|tests?|__tests__|fixtures?|examples?|dist|build|target|archive|bench|loadtests|clienttests|conformance|\.git|\.angular)(?:\/|$)|(?:^|\/)(?:test_|server_tests)|\.(?:spec|test)\./i.test(source)) throw Error(`Ineligible seed source: ${source}`);
      const text = readSource(source);
      const stem = host.slice(0,-6);
      const fqdn = new RegExp(`(?<![a-z0-9-])${host.replaceAll('.', '\\.')}\\b`, 'i');
      const assignment = new RegExp(`\\bHOSTNAME\\s*=\\s*["']${stem}["']`);
      if (!fqdn.test(text) && !assignment.test(text)) throw Error(`No operational evidence for ${host} in ${source}`);
    }
    return {host,sources};
  }).sort((a,b)=>a.host.localeCompare(b.host));
}
