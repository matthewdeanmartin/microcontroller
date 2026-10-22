import { bootstrapApplication } from '@angular/platform-browser';
import { provideBrowserGlobalErrorListeners, provideZonelessChangeDetection } from '@angular/core';
import { provideRouter, Routes, withComponentInputBinding, withHashLocation } from '@angular/router';
import { App, EventPage, GroupPage, Landing } from './app/app';

// Hash routing: GitHub Pages has no fallback to index.html for unknown paths.
const routes: Routes = [
  { path: '', component: Landing, title: 'party' },
  { path: 'group/:ref', component: GroupPage, title: 'Group · party' },
  { path: 'event/:id', component: EventPage, title: 'Event · party' },
  { path: '**', redirectTo: '' },
];

bootstrapApplication(App, {
  providers: [
    provideBrowserGlobalErrorListeners(),
    provideZonelessChangeDetection(),
    provideRouter(routes, withHashLocation(), withComponentInputBinding()),
  ],
}).catch((err) => console.error(err));
