import { bootstrapApplication } from '@angular/platform-browser';
import { provideBrowserGlobalErrorListeners, provideZonelessChangeDetection } from '@angular/core';
import { provideRouter, Routes } from '@angular/router';
import { App } from './app/app';
import { Dashboard } from './app/pages/dashboard';

// Path routing: the board serves index.html for every extension-less path.
const routes: Routes = [
  { path: '', component: Dashboard, title: 'housemetrics' },
  { path: 'system', loadComponent: () => import('./app/pages/system').then((m) => m.System), title: 'System · housemetrics' },
  { path: 'lab', loadComponent: () => import('./app/pages/lab').then((m) => m.Lab), title: 'Format lab · housemetrics' },
  { path: 'devices', loadComponent: () => import('./app/pages/devices').then((m) => m.Devices), title: 'Devices · housemetrics' },
  { path: '**', redirectTo: '' },
];

bootstrapApplication(App, {
  providers: [provideBrowserGlobalErrorListeners(), provideZonelessChangeDetection(), provideRouter(routes)],
}).catch((err) => console.error(err));
