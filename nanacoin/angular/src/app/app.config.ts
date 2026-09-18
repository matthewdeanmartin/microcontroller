import {
  ApplicationConfig,
  provideBrowserGlobalErrorListeners,
  provideZonelessChangeDetection,
} from '@angular/core';
import { provideHttpClient, withFetch } from '@angular/common/http';
import { provideRouter, withHashLocation } from '@angular/router';

import { routes } from './app.routes';

export const appConfig: ApplicationConfig = {
  providers: [
    provideBrowserGlobalErrorListeners(),
    // Signals throughout, so zone.js has nothing to do.
    provideZonelessChangeDetection(),
    provideHttpClient(withFetch()),
    // Hash routing: this is a static site that may end up on a plain file
    // host with no rewrite rules, where /market would 404 on refresh.
    provideRouter(routes, withHashLocation()),
    // ApiBase is a plain root-provided service: the address it holds must be
    // changeable at runtime, which a bootstrap-time factory value could not be.
  ],
};
