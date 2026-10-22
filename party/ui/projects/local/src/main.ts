import { bootstrapApplication } from '@angular/platform-browser';
import { ErrorHandler, inject, Injectable, provideBrowserGlobalErrorListeners, provideZonelessChangeDetection } from '@angular/core';
import { CanActivateFn, provideRouter, Router, Routes, withComponentInputBinding } from '@angular/router';
import { Api } from './app/api';
import { App } from './app/app';
import { UpdateRecovery } from './app/update-recovery';
import { Home } from './app/pages/home';
import { Finishing, SignIn, SignUp, Setup } from './app/pages/auth';

async function ready(api: Api): Promise<void> {
  if (!api.ready()) await api.refresh().catch(() => undefined);
}

/** Signed-in pages: set the board up first, then sign in. */
const signedIn: CanActivateFn = async () => {
  const api = inject(Api);
  const router = inject(Router);
  await ready(api);
  if (api.status()?.setup_needed) return router.parseUrl('/setup');
  return api.me() ? true : router.parseUrl('/signin');
};

/** Sign-in pages: pointless once signed in. */
const signedOut: CanActivateFn = async () => {
  const api = inject(Api);
  const router = inject(Router);
  await ready(api);
  if (api.status()?.setup_needed) return router.parseUrl('/setup');
  return api.me() ? router.parseUrl('/') : true;
};

const setupOnly: CanActivateFn = async () => {
  const api = inject(Api);
  const router = inject(Router);
  await ready(api);
  return api.status()?.setup_needed ? true : router.parseUrl('/');
};

const adminOnly: CanActivateFn = async () => {
  const api = inject(Api);
  const router = inject(Router);
  await ready(api);
  return api.me()?.role === 'admin' ? true : router.parseUrl('/');
};

/**
 * A board update renames the hashed chunks; an open tab then fails to load
 * one. Turn that into a single reload instead of a broken page.
 */
@Injectable()
class GlobalErrorHandler implements ErrorHandler {
  private readonly recovery = inject(UpdateRecovery);

  handleError(error: unknown): void {
    if (!this.recovery.recover(error)) console.error(error);
  }
}

// Path routing: the board serves index.html for every extension-less path.
const routes: Routes = [
  { path: '', component: Home, canActivate: [signedIn], title: 'party' },
  { path: 'setup', component: Setup, canActivate: [setupOnly], title: 'Set up · party' },
  { path: 'signin', component: SignIn, canActivate: [signedOut], title: 'Sign in · party' },
  { path: 'signup', component: SignUp, canActivate: [signedOut], title: 'Make an account · party' },
  { path: 'signin/finishing', component: Finishing, title: 'Signing in · party' },
  {
    path: 'invite/:token',
    loadComponent: () => import('./app/pages/invite').then((m) => m.Invite),
    title: 'Invitation · party',
  },
  {
    path: 'admin/channels',
    canActivate: [signedIn, adminOnly],
    loadComponent: () => import('./app/pages/channels').then((m) => m.Channels),
    title: 'Channels · party',
  },
  {
    path: 'groups',
    canActivate: [signedIn],
    loadComponent: () => import('./app/pages/groups').then((m) => m.Groups),
    title: 'Groups · party',
  },
  {
    path: 'groups/new',
    canActivate: [signedIn],
    loadComponent: () => import('./app/pages/group-form').then((m) => m.GroupForm),
    title: 'New group · party',
  },
  {
    path: 'groups/:ref/edit',
    canActivate: [signedIn],
    loadComponent: () => import('./app/pages/group-form').then((m) => m.GroupForm),
    title: 'Edit group · party',
  },
  {
    path: 'groups/:ref',
    canActivate: [signedIn],
    loadComponent: () => import('./app/pages/group').then((m) => m.Group),
    title: 'Group · party',
  },
  {
    path: 'groups/:ref/events/new',
    canActivate: [signedIn],
    loadComponent: () => import('./app/pages/event-form').then((m) => m.EventForm),
    title: 'New event · party',
  },
  {
    path: 'events/:id/edit',
    canActivate: [signedIn],
    loadComponent: () => import('./app/pages/event-form').then((m) => m.EventForm),
    title: 'Edit event · party',
  },
  {
    path: 'events/:id',
    canActivate: [signedIn],
    loadComponent: () => import('./app/pages/event').then((m) => m.EventPage),
    title: 'Event · party',
  },
  {
    path: 'notifications',
    canActivate: [signedIn],
    loadComponent: () => import('./app/pages/notifications').then((m) => m.Notifications),
    title: 'Notifications · party',
  },
  {
    path: 'people',
    canActivate: [signedIn],
    loadComponent: () => import('./app/pages/people').then((m) => m.People),
    title: 'People · party',
  },
  {
    path: 'me',
    canActivate: [signedIn],
    loadComponent: () => import('./app/pages/me').then((m) => m.Profile),
    title: 'Me · party',
  },
  {
    path: 'admin',
    canActivate: [signedIn, adminOnly],
    loadComponent: () => import('./app/pages/admin').then((m) => m.Admin),
    title: 'Admin · party',
  },
  { path: '**', redirectTo: '' },
];

bootstrapApplication(App, {
  providers: [
    provideBrowserGlobalErrorListeners(),
    provideZonelessChangeDetection(),
    provideRouter(routes, withComponentInputBinding()),
    { provide: ErrorHandler, useClass: GlobalErrorHandler },
  ],
}).catch((err) => console.error(err));
