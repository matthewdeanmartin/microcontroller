/* Everything stays on the device; codes/passwords are never put in URLs or storage. */
(() => {
  'use strict';
  const $ = id => document.getElementById(id);
  const apiRoot = '/wifi-setup/api/';
  let code = '', busy = false, timer = null, connected = false, retryDirty = false;
  const bytes = value => new TextEncoder().encode(value).length;
  function message(text, error = false) {
    $('message').textContent = text;
    $('message').classList.toggle('error', error);
  }
  function setBusy(value) {
    busy = value;
    ['connect', 'rescan', 'save-retry', 'finish'].forEach(id => { $(id).disabled = value; });
  }
  async function api(path, body, post = false) {
    const headers = { Accept: 'application/json', 'X-Wifi-Setup-Request': '1' };
    if (code) headers['X-Wifi-Setup-Code'] = code;
    if (body !== undefined) headers['Content-Type'] = 'application/json';
    const response = await fetch(apiRoot + path, {
      method: post ? 'POST' : 'GET', headers, cache: 'no-store',
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const data = await response.json();
    if (!response.ok) {
      const error = new Error(data.message || data.error || `Device returned ${response.status}`);
      error.status = response.status;
      throw error;
    }
    return data;
  }
  function schedule(delay = 800) { clearTimeout(timer); timer = setTimeout(poll, delay); }
  function networks(names) {
    const selected = $('networks').value;
    $('networks').replaceChildren();
    if (!names.length) $('networks').add(new Option('No networks found — rescan or enter a name', ''));
    else {
      $('networks').add(new Option('Choose a network', ''));
      names.forEach(name => $('networks').add(new Option(name, name)));
      if (names.includes(selected)) $('networks').value = selected;
    }
  }
  async function poll() {
    try {
      const status = await api('status');
      if (!status.active) { closed(); return; }
      const running = ['scanning', 'joining', 'saving', 'closing'].includes(status.phase);
      setBusy(running);
      if (status.networks) networks(status.networks);
      if (!retryDirty && document.activeElement !== $('retry-minutes')) $('retry-minutes').value = status.retry_minutes;
      if (status.phase === 'connected' && status.address) {
        connected = true;
        $('settings').hidden = true;
        $('success').hidden = false;
        const url = new URL(`http://${status.address}/`);
        // The backend supplies IPv4; constrain links even if a custom backend is buggy.
        if (!/^\d{1,3}(\.\d{1,3}){3}$/.test(status.address)) throw new Error('Invalid device address');
        $('address').href = url.href;
        $('address').textContent = url.href;
        message('Wi-Fi connected. Your settings have been saved.');
      } else if (status.phase === 'error') {
        message(status.error || 'Connection failed. Check your network and password, then retry.', true);
      } else if (running) {
        const text = { scanning: 'Scanning nearby Wi-Fi…', joining: 'Connecting and waiting for a network address…', saving: 'Saving your retry setting…', closing: 'Closing setup Wi-Fi…' };
        message(text[status.phase]);
      } else message('Choose your network, or enter its name manually.');
      schedule(running ? 800 : 2500);
    } catch (error) {
      if (error.status === 401) {
        code = ''; setBusy(false); $('gate').hidden = false; $('settings').hidden = true;
        message('Enter the correct setup code.', true); return;
      }
      if (connected && error.status === 404) { closed(); return; }
      message(connected ? 'Setup may have closed. Switch to your home Wi-Fi and use the address above.' : 'Reconnecting to the device… Stay on its setup Wi-Fi.', !connected);
      schedule(2000);
    }
  }
  async function operation(path, body) {
    clearTimeout(timer); setBusy(true);
    try {
      await api(path, body, true);
      if (path === 'retry' || path === 'join') retryDirty = false;
      await poll();
    }
    catch (error) { setBusy(false); message(error.message, true); if (error.status === 409) schedule(); }
  }
  function minutes() {
    const value = Number($('retry-minutes').value);
    if (!Number.isInteger(value) || value < 1 || value > 60) throw new Error('Retry time must be a whole number from 1 to 60 minutes.');
    return value;
  }
  function closed() {
    clearTimeout(timer); setBusy(false); $('finish').disabled = true;
    message('Setup closed. Switch your phone to your home Wi-Fi.');
  }
  async function begin() {
    try {
      // Authenticate before revealing networks or settings.
      const state = await api('status');
      $('retry-minutes').value = state.retry_minutes;
      $('gate').hidden = true; $('settings').hidden = false;
      if (state.address) await poll(); else await operation('scan');
    } catch (error) { message(error.message, true); }
  }
  $('unlock-form').addEventListener('submit', event => {
    event.preventDefault(); code = $('code').value; $('code').value = ''; begin();
  });
  $('manual').addEventListener('change', () => { $('manual-name').hidden = !$('manual').checked; });
  $('retry-minutes').addEventListener('input', () => { retryDirty = true; });
  $('show-password').addEventListener('change', () => { $('password').type = $('show-password').checked ? 'text' : 'password'; });
  $('rescan').addEventListener('click', () => { if (!busy) operation('scan'); });
  $('save-retry').addEventListener('click', () => {
    if (busy) return;
    try { operation('retry', { retry_minutes: minutes() }); } catch (error) { message(error.message, true); }
  });
  $('join-form').addEventListener('submit', event => {
    event.preventDefault(); if (busy) return;
    try {
      const ssid = $('manual').checked ? $('ssid').value : $('networks').value;
      const password = $('password').value;
      if (!ssid || bytes(ssid) > 32 || ssid.includes('\0')) throw new Error('Choose a network or enter a name of at most 32 bytes.');
      if (password && !((bytes(password) >= 8 && bytes(password) <= 63 && !password.includes('\0')) || /^[a-f0-9]{64}$/i.test(password))) throw new Error('Use 8–63 password bytes, or a 64-character hex key.');
      const request = { ssid, password, retry_minutes: minutes() };
      if (bytes(JSON.stringify(request)) > 512) throw new Error('Network settings are too large.');
      $('password').value = ''; $('show-password').checked = false; $('password').type = 'password';
      operation('join', request);
    } catch (error) { message(error.message, true); }
  });
  $('finish').addEventListener('click', async () => {
    if (busy) return;
    clearTimeout(timer); setBusy(true);
    try { await api('close', undefined, true); closed(); }
    catch (error) { setBusy(false); message(error.message, true); }
  });
  window.addEventListener('pagehide', () => { clearTimeout(timer); code = ''; $('password').value = ''; });
  api('meta').then(meta => {
    if (!meta.active) { message('Wi-Fi setup is not active.'); return; }
    if (meta.code_required) { $('gate').hidden = false; message('Enter the device setup code to continue.'); }
    else begin();
  }).catch(() => message('Cannot reach the device. Connect to its setup Wi-Fi, then reload this page.', true));
})();
