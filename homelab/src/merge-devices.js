export function mergeDevices(previous, found) {
  let rows=[...previous];
  for (const fresh of found) {
    const matches=rows.filter(d=>(!!d.ip&&d.ip===fresh.ip)||d.host===fresh.host||fresh.aliases.includes(d.host)||d.aliases.some(a=>fresh.aliases.includes(a)));
    const saved=matches.find(d=>d.notes||d.label)||matches[0];
    rows=rows.filter(d=>!matches.includes(d));
    const positive=found.find(d=>saved?.ip&&d.ip===saved.ip&&d.status==='online');
    const failedName=!fresh.ip&&saved?.ip;
    const current=failedName?{...saved,status:positive?saved.status:saved.lastSeen?'offline':'unverified',checks:fresh.checks}:fresh.status==='observed'&&saved?.lastSeen?{...fresh,status:'offline',lastSeen:saved.lastSeen}:fresh;
    const responses=failedName&&positive?saved.endpoints.filter(e=>e.available!==false):fresh.endpoints;
    rows.push({...current,notes:saved?.notes,label:saved?.label,firstSeen:saved?.firstSeen||fresh.lastSeen,
      aliases:[...new Set([...fresh.aliases,...matches.flatMap(d=>[d.host,...d.aliases])])].filter(h=>h!==current.ip),
      endpoints:[...new Map([...matches.flatMap(d=>d.endpoints).map(e=>({...e,available:false})),...responses.map(e=>({...e,available:true}))].map(e=>[e.url,e])).values()],
      sources:[...new Set([...fresh.sources,...matches.flatMap(d=>d.sources)])]});
  }
  return rows;
}
