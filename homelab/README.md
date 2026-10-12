# Homelab

A standalone Angular 22.2.2 directory for the home network, backed by a local Node discovery service. Requires Node 24.15+ (this PC has 24.18) and npm.

```sh
cd homelab
npm install
npm start
```

Open **http://127.0.0.1:4201**. First start builds the app. After editing, run `npm run build` before `npm start`, or use `npm run dev` for live reload at http://127.0.0.1:4200. Both services bind only to loopback. Use the same browser and URL consistently: localStorage is per origin, so development and production have separate inventories.

Select your Wi-Fi/Ethernet subnet and click **Scan network**. It scans the complete IPv4 subnet, excluding network/broadcast addresses where applicable, with 24 device workers. You may enter another private CIDR between /16 and /32; large subnets take longer. Stop cancels remaining work after active probes finish. Hyper-V/WSL networks also appear; the interface with a default gateway is listed first.

Discovery combines OS DNS, mDNS A/SRV advertisements, TCP probes (80, 443, 8080, 8443, 8000, 3000, 22, 445, 3389 and advertised service ports), ping, reverse DNS and the ARP neighbor cache. Every seeded or remembered domain also receives explicit HTTP and HTTPS root-page requests, even when subnet probing did not find its IP. Each attempt is recorded on the detail page; hostname probes retain Host/SNI, use mDNS-resolved addresses when available, and otherwise try OS DNS with an eight-second deadline. HTTP root-page inspection captures title, response status, server, content type, redirects and HTTPS certificate metadata. HTTPS trust uses both Node defaults and the Windows/system CA store. Inventory cards and detail pages provide clickable expected/found hostname, alias and IP links even when no web endpoint has yet responded. It does not follow redirects or log in to devices. Classification is a heuristic based on names, gateway addresses and ports, and does not claim to identify an operating system or hardware model. Unknown devices remain generic.

The inventory, aliases, first/last observation times, labels and notes are remembered in browser localStorage. Export JSON backs them up. An unfinished/cancelled scan does not mark unchecked devices offline. `observed` denotes mDNS or neighbor-cache evidence; cached ARP entries can be stale. `online` describes the last probe, not a continuous health check. Devices disappear from neither storage nor inventory when offline. When a known hostname moves to a new IP, its saved notes move with it.

14 operational `.local` names are seeded from reviewed firmware configuration, board profiles, scrape configuration and deployment documentation across this repo, `../mastomini` and `../nanacoin`. The explicit manifest is `scripts/seed-manifest.json`; the generator reads only those approved sources and never recursively searches repos, dependencies, tests, examples or generated files. Existing browser inventories automatically drop obsolete seed-only entries while preserving manually added or actually discovered devices and notes on retained entries. Refresh them from this directory with `npm run seeds`, then restart the service. Add a new `.local` name in the UI; scans try its OS DNS and mDNS records too. Seed presence alone is never treated as evidence of a live board.

The service must run on this PC: browser CORS and local-network policies make direct browser scanning unreliable. Sleeping machines, AP client isolation, firewalls and devices that do not answer these probes may be missed. Only IPv4 private subnets are scanned; IPv6, VLAN routing and exhaustive arbitrary TCP ports are not covered. mDNS support depends on multicast reaching this PC. HTTPS inspection accepts an untrusted certificate only for reading root-page metadata and labels its trust result; normal browser links retain browser certificate enforcement.

```sh
npm test
npm run build
```

Tests cover subnet enumeration and scan boundaries, real TCP/HTTP discovery, bounded response metadata and redirect handling. Dependency versions match the repo's Angular 22.2 line, with all Angular packages pinned to 22.2.2 and a committed npm lockfile.


The initial filter is **Online web services**, and the selected filter persists across routes and reloads. **Only ping** includes active ping responders without current TCP/HTTP responses. Web filters require a current HTML, text, JSON/XML API or HTTP redirect response; AirTunes/AirPlay media-control responses are listed separately. Historical responses are retained for reference but no longer marked confirmed after a fresh scan fails to reproduce them. Confirmed links show HTTP status, content type and server; unverified address shortcuts are collapsed separately. **Unverified** means never reached; **Offline** means previously reached but no current live response; neighbor/mDNS evidence alone is **Observed**. An empty Offline filter can be normal before any known live device disappears.
