// Which data the companion site shows. The demo build replaces this file
// with mode.demo.ts (angular.json, configuration "demo").
export const MODE: 'live' | 'demo' = 'live';

/**
 * Where the board publishes (spec/05-companion-site.md#two-repos-one-owner):
 * the data repo is a sibling project site under the same GitHub
 * organization, so a relative path reaches it on the same origin.
 */
export const DATA_BASE = '../party-data/';
