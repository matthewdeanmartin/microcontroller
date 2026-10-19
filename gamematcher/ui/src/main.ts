import { bootstrapApplication } from '@angular/platform-browser';
import { ErrorHandler, inject, Injectable, provideBrowserGlobalErrorListeners, provideZonelessChangeDetection } from '@angular/core';
import { CanActivateFn, provideRouter, Router, Routes, withComponentInputBinding } from '@angular/router';
import { Api } from './app/api';
import { App } from './app/app';
import { UpdateRecovery } from './app/update-recovery';
import { Home } from './app/pages/home';
import { SignIn, SignUp, Setup } from './app/pages/auth';

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
 * one. Turn that into a single reload instead of a broken page. With
 * provideBrowserGlobalErrorListeners, window errors and rejected dynamic
 * imports arrive here too.
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
  { path: '', component: Home, canActivate: [signedIn], title: 'Game Matcher' },
  { path: 'setup', component: Setup, canActivate: [setupOnly], title: 'Set up · Game Matcher' },
  { path: 'signin', component: SignIn, canActivate: [signedOut], title: 'Sign in · Game Matcher' },
  {
    path: 'signin/finishing',
    loadComponent: () => import('./app/pages/finishing').then((m) => m.Finishing),
    title: 'Signing in · Game Matcher',
  },
  { path: 'signup', component: SignUp, canActivate: [signedOut], title: 'Make an account · Game Matcher' },
  {
    path: 'games',
    canActivate: [signedIn],
    loadComponent: () => import('./app/pages/games').then((m) => m.Games),
    title: 'Games · Game Matcher',
  },
  {
    path: 'games/:id',
    canActivate: [signedIn],
    loadComponent: () => import('./app/pages/game').then((m) => m.Game),
    title: 'Game · Game Matcher',
  },
  {
    path: 'plans/new',
    canActivate: [signedIn],
    loadComponent: () => import('./app/pages/plan-form').then((m) => m.PlanForm),
    title: 'New plan · Game Matcher',
  },
  {
    path: 'plans/:id/edit',
    canActivate: [signedIn],
    loadComponent: () => import('./app/pages/plan-form').then((m) => m.PlanForm),
    title: 'Edit plan · Game Matcher',
  },
  {
    path: 'plans/:id',
    canActivate: [signedIn],
    loadComponent: () => import('./app/pages/plan').then((m) => m.Plan),
    title: 'Plan · Game Matcher',
  },
  {
    path: 'notifications',
    canActivate: [signedIn],
    loadComponent: () => import('./app/pages/notifications').then((m) => m.Notifications),
    title: 'Notifications · Game Matcher',
  },
  {
    path: 'me',
    canActivate: [signedIn],
    loadComponent: () => import('./app/pages/profile').then((m) => m.Profile),
    title: 'Profile · Game Matcher',
  },
  {
    path: 'admin',
    canActivate: [signedIn, adminOnly],
    loadComponent: () => import('./app/pages/admin').then((m) => m.Admin),
    title: 'Admin · Game Matcher',
  },
  {
    path: 'status',
    canActivate: [signedIn, adminOnly],
    loadComponent: () => import('./app/pages/status').then((m) => m.Status),
    title: 'Status · Game Matcher',
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
