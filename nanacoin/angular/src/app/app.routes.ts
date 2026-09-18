import { Routes } from '@angular/router';

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
    path: 'logs',
    loadComponent: () => import('./pages/logs').then((m) => m.LogsPage),
    title: 'Server logs — NanaCoin',
  },
  {
    path: 'nana',
    loadComponent: () => import('./pages/nana').then((m) => m.NanaPage),
    title: 'Household — NanaCoin',
  },
  { path: '', pathMatch: 'full', redirectTo: 'market' },
  { path: '**', redirectTo: 'market' },
];
