import { Routes } from '@angular/router';

import { nanaOnly } from './api/guards';

/**
 * Routes are lazy so that Nana's admin screens - the largest part of the app
 * and the part most people never open - are not in the initial bundle.
 */
export const routes: Routes = [
  {
    path: 'market',
    loadComponent: () => import('./pages/market').then((m) => m.MarketPage),
    title: 'Market — NanaCoin',
  },
  {
    path: 'send',
    loadComponent: () => import('./pages/send').then((m) => m.SendPage),
    title: 'Send — NanaCoin',
  },
  {
    path: 'offers',
    loadComponent: () => import('./pages/offers').then((m) => m.OffersPage),
    title: 'Offers — NanaCoin',
  },
  {
    path: 'forex',
    loadComponent: () => import('./pages/forex').then((m) => m.ForexPage),
    title: 'Exchange — NanaCoin',
  },
  {
    path: 'history',
    loadComponent: () => import('./pages/history').then((m) => m.HistoryPage),
    title: 'History — NanaCoin',
  },
  {
    path: 'economy',
    loadComponent: () => import('./pages/economy').then((m) => m.EconomyPage),
    title: 'Economy — NanaCoin',
  },
  {
    path: 'clientlog',
    loadComponent: () => import('./pages/clientlog').then((m) => m.ClientLogPage),
    title: 'Browser log — NanaCoin',
  },
  {
    path: 'logs',
    loadComponent: () => import('./pages/logs').then((m) => m.LogsPage),
    title: 'Server logs — NanaCoin',
  },
  {
    path: 'nana',
    loadComponent: () => import('./pages/nana').then((m) => m.NanaPage),
    title: 'Household — NanaCoin',
    // The nav tab is already hidden from everyone else, but hiding a link is
    // not a rule: #/nana typed or bookmarked still rendered the admin screen,
    // with buttons - disable a member, create a user, issue coin - that the
    // server would refuse. The guard makes the hidden tab mean something.
    canActivate: [nanaOnly],
  },
  { path: '', pathMatch: 'full', redirectTo: 'market' },
  { path: '**', redirectTo: 'market' },
];
