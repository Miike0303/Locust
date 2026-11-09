#!/usr/bin/env node
/**
 * One production-browser release lifecycle; Node 24+, no packages.
 * Build first: npm --prefix apps/desktop run build
 * Run with an EXISTING CLI: LOCUST_CLI=/path/to/locust node tools/ui_release_lifecycle.mjs
 * Optional CHROME_PATH (Chrome executable on any OS), CARGO_TARGET_DIR (CLI fallback).
 * Chrome defaults to the standard installation on Windows/Linux/macOS. For trusted
 * loopback fixtures in restricted sandboxes only: LOCUST_CHROME_NO_SANDBOX=1.
 * If an AppContainer remaps temp to a location Chrome cannot write, set TEMP
 * and TMP to an explicitly writable OS temp directory before starting Node.
 * All generated files, downloads, profiles and logs live in an owned OS temp
 * directory, deleted on success AND failure. stdout is one bounded JSON result.
 * HTTP observations are read-only. Workflow mutations come from visible controls
 * using CDP mouse/keyboard input, never direct fetches or React/store calls.
 * A pre-document WebSocket constructor proxy changes ONLY the loopback URL;
 * CDP redirects HTTP similarly. Assets and all responses/frames are unmodified.
 */
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { cp, mkdir, mkdtemp, readFile, writeFile, readdir, realpath, rm, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { pause, digest, startProcess, chromeArguments, freePort, DevTools, serveProduction, fingerprint, alive, stop } from './ui_browser_helpers.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const dist = path.join(root, 'apps/desktop/dist');
const fixture = path.join(root, 'crates/formats/tests/fixtures/rpgmaker_mv');
const started = Date.now(), deadline = started + 210000;
const summary = { success: false, stages: [], websocket: {}, verification: {}, errors: [] };
const requests = [], sockets = new Map(), frames = [], downloads = new Map();
const requestInfo = new Map();
const bodyRoutes = new Set(['/api/translate/start', '/api/validate', '/api/inject', '/api/patch/pack', '/api/patch/apply', '/api/patch/rollback']);
let temporary, backend, chrome, staticServer, cdp, session, backendUrl, frontendUrl;
let interrupted = false, stage = 'prerequisites';
const q = JSON.stringify;
const dialog = 'document.querySelector("[role=dialog]")';
const control = selector => `document.querySelector(${q(selector)})`;

function checkDeadline() {
  assert(!interrupted, 'Interrupted');
  assert(Date.now() < deadline, 'Lifecycle exceeded its 210-second work deadline');
}
async function until(probe, description, timeout = 15000) {
  const end = Math.min(deadline, Date.now() + timeout);
  let last;
  while (Date.now() < end) {
    checkDeadline();
    if (summary.errors.length) throw new Error(summary.errors[0]);
    try { const result = await probe(); if (result) return result; } catch (error) { last = error; }
    await pause(80);
  }
  throw new Error(`Timed out: ${description}${last ? ` (${last.message})` : ''}`);
}
const command = (method, params = {}) => { checkDeadline(); return cdp.call(method, params, session); };
async function evaluate(expression) {
  const result = await command('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true, userGesture: true });
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description || result.exceptionDetails.text);
  return result.result.value;
}
const waitFor = (expression, description) => until(() => evaluate(expression), description);

// DOM reads only locate controls. Every activation is actual browser input and
// refuses disabled, hidden, covered or off-viewport controls.
async function clickElement(expression, description) {
  const point = await until(() => evaluate(`(() => {
    const el = (${expression}); if (!el || el.disabled) return null;
    el.scrollIntoView({block:'center', inline:'nearest'});
    const r = el.getBoundingClientRect(), style = getComputedStyle(el);
    if (!r.width || !r.height || style.visibility !== 'visible' || style.display === 'none') return null;
    const x = r.x + r.width/2, y = r.y + r.height/2;
    if (!el.contains(document.elementFromPoint(x,y))) return null;
    return {x,y};
  })()`), `visible enabled ${description}`);
  await command('Input.dispatchMouseEvent', { type: 'mouseMoved', ...point });
  await command('Input.dispatchMouseEvent', { type: 'mousePressed', button: 'left', clickCount: 1, ...point });
  await command('Input.dispatchMouseEvent', { type: 'mouseReleased', button: 'left', clickCount: 1, ...point });
}
const click = selector => clickElement(control(selector), selector);
const button = (label, within = 'document') => clickElement(`Array.from(${within}?.querySelectorAll('button') || []).find(el => el.textContent.trim() === ${q(label)})`, `button ${label}`);
async function key(key, code, windowsVirtualKeyCode, modifiers = 0) {
  await command('Input.dispatchKeyEvent', { type: 'keyDown', key, code, windowsVirtualKeyCode, modifiers });
  await command('Input.dispatchKeyEvent', { type: 'keyUp', key, code, windowsVirtualKeyCode, modifiers });
}
async function fill(selector, value) {
  await click(selector);
  await key('a', 'KeyA', 65, 2);
  await command('Input.insertText', { text: value });
  await waitFor(`${control(selector)}?.value === ${q(value)}`, `input accepted ${selector}`);
}
async function select(selector, value) {
  await waitFor(`Array.from(${control(selector)}?.options || []).some(o => o.value === ${q(value)})`, `option ${value}`);
  const index = await evaluate(`Array.from(${control(selector)}.options).findIndex(o => o.value === ${q(value)})`);
  await click(selector);
  await key('Home', 'Home', 36);
  for (let i = 0; i < index; i++) await key('ArrowDown', 'ArrowDown', 40);
  await key('Enter', 'Enter', 13);
  await waitFor(`${control(selector)}.value === ${q(value)}`, `selected ${value}`);
}
async function navigate(route) {
  await command('Page.navigate', { url: `${frontendUrl}${route}` });
  await waitFor(`document.readyState === 'complete'`, 'production page loaded');
}

// Deliberately no method/body parameters: observers cannot execute a stage.
async function readBackend(route) {
  checkDeadline();
  assert(route.startsWith('/api/strings') || route.startsWith('/api/validation/'), 'Read-only observer route required');
  const response = await fetch(`${backendUrl}${route}`, { signal: AbortSignal.timeout(10000) });
  assert(response.ok, `Backend read ${route}: ${response.status}`);
  return response.json();
}
const completedRequest = (route, offset = 0) => until(() => requests.slice(offset).find(r => r.path === route && r.body !== undefined), `UI response ${route}`);
async function observeJob(channel, type) {
  const result = await until(() => frames.find(f => f.channel === channel && f.data.type === type), `${channel} WebSocket ${type}`, 45000);
  assert.equal(new URL(result.url).origin, new URL(backendUrl.replace('http:', 'ws:')).origin);
  assert(sockets.get(result.requestId)?.handshake === 101, 'Real WebSocket handshake required');
  assert(!frames.some(f => f.channel === channel && ['failed', 'error', 'batch_failed'].includes(f.data.type)), 'Job stream reported a failure');
  summary.websocket[channel] = { jobId: new URL(result.url).pathname.split('/').at(-1), handshake: 101, terminal: result.data };
  return result.data;
}

async function main() {
  assert(Number(process.versions.node.split('.')[0]) >= 24, 'Node 24+ required');
  assert.equal(process.argv.length, 2, 'Usage: node tools/ui_release_lifecycle.mjs');
  const chromePath = process.env.CHROME_PATH || (process.platform === 'win32'
    ? 'C:/Program Files/Google/Chrome/Application/chrome.exe'
    : process.platform === 'darwin' ? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' : '/usr/bin/google-chrome');
  const cli = process.env.LOCUST_CLI || (process.env.CARGO_TARGET_DIR && path.resolve(process.env.CARGO_TARGET_DIR, 'debug', process.platform === 'win32' ? 'locust.exe' : 'locust'));
  assert(cli, 'Set LOCUST_CLI to a prebuilt CLI (or set CARGO_TARGET_DIR)');
  for (const file of [chromePath, cli, path.join(dist, 'index.html')]) assert((await stat(file)).isFile(), `Missing prerequisite: ${file}`);
  const assetsBefore = await fingerprint(dist), fixtureBefore = await fingerprint(fixture);
  summary.inputs = { cliSha256: digest(await readFile(cli)), assetsSha256: digest(JSON.stringify(assetsBefore)), fixtureSha256: digest(JSON.stringify(fixtureBefore)) };
  // Canonical form: hosted Windows reports an 8.3 short temp root (RUNNER~1).
  temporary = await realpath(await mkdtemp(path.join(tmpdir(), 'locust-ui-lifecycle-')));
  const source = path.join(temporary, 'Synthetic MV'), consumer = path.join(temporary, 'Pristine consumer');
  const output = path.join(temporary, 'Injected copies'), downloadDir = path.join(temporary, 'downloads');
  const profile = path.join(temporary, 'profile'), zip = path.join(temporary, 'synthetic-patch.zip');
  for (const directory of [profile, output, downloadDir]) await mkdir(directory);
  await cp(fixture, source, { recursive: true });
  // Opaque, nested nontext sentinels must survive copy, apply and rollback.
  await mkdir(path.join(source, 'audio/subdir'), { recursive: true });
  await writeFile(path.join(source, 'audio/subdir/sentinel.bin'), Buffer.from([0, 255, 1, 127, 13, 10]));
  await cp(source, consumer, { recursive: true });
  const pristine = await fingerprint(source);
  assert.deepEqual(await fingerprint(consumer), pristine);
  const port = await freePort();
  backendUrl = `http://127.0.0.1:${port}`;
  backend = startProcess(cli, ['server', '--port', String(port)], {
    ...process.env, LOCUST_DATA_DIR: profile, LOCUST_CONFIG: path.join(profile, 'config.json'), LOCUST_BACKUP_ROOT: path.join(profile, 'backups'),
  }, { cwd: temporary });
  await until(async () => {
    if (backend.exited) throw new Error(`Backend exited: ${backend.error || backend.log}`);
    return (await fetch(`${backendUrl}/health`, { signal: AbortSignal.timeout(500) })).ok;
  }, 'isolated backend');
  staticServer = await serveProduction(dist);
  frontendUrl = `http://127.0.0.1:${staticServer.address().port}`;
  chrome = startProcess(chromePath, chromeArguments({ profileDir: path.join(temporary, 'chrome'), noSandbox: process.env.LOCUST_CHROME_NO_SANDBOX === '1' }), process.env, { cwd: temporary });
  const websocket = await until(async () => {
    if (chrome.exited) throw new Error(`Chrome exited: ${chrome.error || chrome.log}`);
    const lines = (await readFile(path.join(temporary, 'chrome/DevToolsActivePort'), 'utf8')).trim().split(/\r?\n/);
    return `ws://127.0.0.1:${lines[0]}${lines[1]}`;
  }, 'Chrome DevToolsActivePort');
  cdp = new DevTools(websocket, error => summary.errors.push(error.message));
  const { targetId } = await cdp.call('Target.createTarget', { url: 'about:blank' });
  ({ sessionId: session } = await cdp.call('Target.attachToTarget', { targetId, flatten: true }));
  for (const domain of ['Page', 'Runtime', 'Network']) await command(`${domain}.enable`);
  await command('Emulation.setDeviceMetricsOverride', { width: 1440, height: 1000, deviceScaleFactor: 1, mobile: false });
  await cdp.call('Browser.setDownloadBehavior', { behavior: 'allowAndName', downloadPath: downloadDir, eventsEnabled: true });
  cdp.on('Browser.downloadWillBegin', e => downloads.set(e.guid, { name: e.suggestedFilename, file: e.guid }));
  cdp.on('Browser.downloadProgress', e => Object.assign(downloads.get(e.guid) || {}, { state: e.state }));
  cdp.on('Runtime.exceptionThrown', e => summary.errors.push(e.exceptionDetails.exception?.description || e.exceptionDetails.text));
  cdp.on('Runtime.consoleAPICalled', e => { if (e.type === 'error') summary.errors.push(e.args.map(a => a.value || a.description || '').join(' ')); });
  cdp.on('Network.loadingFailed', e => { if (!e.canceled) summary.errors.push(`Browser request failed: ${e.errorText}`); });
  cdp.on('Network.webSocketCreated', e => {
    const url = new URL(e.url);
    assert.equal(url.origin, backendUrl.replace('http:', 'ws:'), 'WebSocket escaped owned backend');
    const channel = url.pathname.includes('/translate/ws/') ? 'translate' : url.pathname.includes('/patch/ws/') ? 'patch' : null;
    assert(channel, `Unexpected WebSocket ${url.pathname}`);
    sockets.set(e.requestId, { url: e.url, channel });
  });
  cdp.on('Network.webSocketHandshakeResponseReceived', e => { const socket = sockets.get(e.requestId); if (socket) socket.handshake = e.response.status; });
  cdp.on('Network.webSocketFrameError', e => summary.errors.push(`WebSocket frame error: ${e.errorMessage}`));
  cdp.on('Network.webSocketFrameReceived', e => {
    const socket = sockets.get(e.requestId);
    assert(socket, 'Unobserved WebSocket');
    frames.push({ ...socket, requestId: e.requestId, data: JSON.parse(e.response.payloadData) });
  });
  cdp.on('Network.requestWillBeSent', e => requestInfo.set(e.requestId, { method: e.request.method, postData: e.request.postData }));
  cdp.on('Network.responseReceived', e => {
    const url = new URL(e.response.url);
    if (url.pathname.startsWith('/api/')) {
      // A fresh profile legitimately has no current project at first boot.
      const emptyProfile = stage === 'open' && url.pathname === '/api/project/current' && e.response.status === 404;
      assert(e.response.status < 400 || emptyProfile, `UI HTTP ${e.response.status} ${url.pathname}`);
      requests.push({ id: e.requestId, path: url.pathname, ...requestInfo.get(e.requestId), status: e.response.status });
    }
  });
  cdp.on('Network.loadingFinished', async e => {
    const request = requests.find(r => r.id === e.requestId);
    if (request?.method === 'POST' && bodyRoutes.has(request.path)) {
      const response = await command('Network.getResponseBody', { requestId: e.requestId });
      const text = response.base64Encoded ? Buffer.from(response.body, 'base64').toString('utf8') : response.body;
      request.body = text ? JSON.parse(text) : null;
    }
  });
  cdp.on('Fetch.requestPaused', async e => {
    const url = new URL(e.request.url);
    if (url.origin === 'http://localhost:7842') {
      await command('Fetch.continueRequest', { requestId: e.requestId, url: `${backendUrl}${url.pathname}${url.search}` });
    } else if ([frontendUrl, backendUrl].includes(url.origin)) {
      await command('Fetch.continueRequest', { requestId: e.requestId });
    } else {
      summary.errors.push(`Refused non-owned browser request: ${url.origin}`);
      await command('Fetch.failRequest', { requestId: e.requestId, errorReason: 'BlockedByClient' });
    }
  });
  await command('Fetch.enable', { patterns: [{ urlPattern: 'http://*/*', requestStage: 'Request' }, { urlPattern: 'https://*/*', requestStage: 'Request' }] });
  await command('Page.addScriptToEvaluateOnNewDocument', { source: `
    localStorage.setItem('locust.ui.language','en');
    localStorage.setItem('locust.workflowGuide.dismissed','true');
    const NativeWebSocket = window.WebSocket;
    window.WebSocket = new Proxy(NativeWebSocket, { construct(Target, args) {
      const url = new URL(args[0], location.href);
      if (url.origin !== 'ws://localhost:7842') throw Error('Unexpected WebSocket destination: ' + url.origin);
      args[0] = ${q(backendUrl.replace('http:', 'ws:'))} + url.pathname + url.search;
      return Reflect.construct(Target, args);
    }});
  ` });

  stage = 'open';
  await navigate('/');
  let folderPrompt = false;
  cdp.on('Page.javascriptDialogOpening', async e => {
    assert(e.type === 'prompt' && stage === 'open', `Unexpected ${e.type} dialog at ${stage}`);
    folderPrompt = true;
    await command('Page.handleJavaScriptDialog', { accept: true, promptText: source });
  });
  await button('Open game folder');
  await waitFor(`location.pathname === '/editor' && document.querySelectorAll('tbody tr').length > 0`, 'extracted project editor');
  assert(folderPrompt, 'Open must use the visible folder control');
  const initial = (await readBackend('/api/strings?limit=100')).entries;
  assert(initial.length > 0 && initial.every(e => !e.translation), 'Fixture must start untranslated');
  const entry = initial.find(e => e.source.includes('Hello, traveler!') && e.tags.includes('dialogue'));
  assert(entry, 'Fixture dialogue missing');
  summary.stages.push(stage);

  stage = 'translate';
  await button('Translate');
  await select('#translation-provider', 'mock');
  await select('#translation-source', 'en');
  await select('#translation-target', 'es');
  await button('Start Translation', dialog);
  const translated = await observeJob('translate', 'completed');
  const translationRequest = await completedRequest('/api/translate/start');
  assert.equal(JSON.parse(translationRequest.postData).provider_id, 'mock');
  assert.equal(translationRequest.body.job_id, summary.websocket.translate.jobId);
  assert.equal(translated.total_translated, initial.length, 'Mock must translate the entire project');
  assert.equal(translated.failed_ids?.length || 0, 0);
  await waitFor(`!!document.querySelector('[role=dialog] button[aria-label="Close translation dialog"]') && !document.querySelector('[role=dialog] #translation-provider')`, 'translation receipt');
  await click('[role=dialog] button[aria-label="Close translation dialog"]');
  await until(async () => (await readBackend('/api/strings?limit=100')).entries.every(e => e.translation), 'persisted mock translations');
  summary.stages.push(stage);

  stage = 'edit-save';
  const edited = 'Hola, viajero! Bienvenido al pueblo.';
  await clickElement(`Array.from(document.querySelectorAll('tbody tr')).find(el => el.textContent.includes('Hello, traveler!'))?.querySelector('td')`, 'fixture dialogue source cell');
  await fill('textarea[id$="-translation"]', edited);
  await key('Enter', 'Enter', 13, 2); // The editor's documented Ctrl+Enter save.
  await until(async () => (await readBackend(`/api/strings?limit=100`)).entries.find(e => e.id === entry.id)?.translation === edited, 'saved edit');
  await navigate('/editor');
  await clickElement(`Array.from(document.querySelectorAll('tbody tr')).find(el => el.textContent.includes('Hello, traveler!'))?.querySelector('td')`, 'saved dialogue source cell');
  await waitFor(`${control('textarea[id$="-translation"]')}?.value === ${q(edited)}`, 'edit survived reload');
  await click('button[aria-label="Close detail panel"]');
  summary.stages.push(stage);

  stage = 'validate';
  const validateOffset = requests.length;
  await button('Validate');
  const validation = (await completedRequest('/api/validate', validateOffset)).body;
  assert(validation.validation.total_checked > 0, 'Validation checked no strings');
  await waitFor(`!!${dialog}`, 'validation results');
  await click('[role=dialog] header button');
  summary.verification.validationChecked = validation.validation.total_checked;
  summary.stages.push(stage);

  stage = 'export';
  await click('button[title="Ctrl+E"]');
  await button('Download', dialog);
  const download = await until(() => [...downloads.values()].find(d => ['completed', 'canceled'].includes(d.state)), 'catalog download');
  assert.equal(download.state, 'completed', 'Chrome canceled the catalog download');
  assert.equal(download.name, 'translation_es.po');
  const catalog = await readFile(path.join(downloadDir, download.file), 'utf8');
  // Decode PO quoted fields, accepting both escaped and continued multiline text.
  const catalogEntries = catalog.replaceAll('\r\n', '\n').split(/\n\s*\n/).map(section => {
    const fields = {}; let field;
    for (const line of section.split('\n')) {
      const match = /^(msgctxt|msgid|msgstr) (".*")$/.exec(line);
      if (match) { field = match[1]; fields[field] = JSON.parse(match[2]); }
      else if (line.startsWith('"') && field) fields[field] += JSON.parse(line);
    }
    return fields;
  });
  assert(catalogEntries.some(e => e.msgid === entry.source && e.msgstr === edited), 'Downloaded PO lost the saved source/translation pair');
  summary.verification.export = { bytes: Buffer.byteLength(catalog), sha256: digest(catalog), editedText: edited };
  summary.stages.push(stage);

  stage = 'inject';
  await button('Inject');
  await select('#inject-mode', 'replace');
  await fill('[role=dialog] input[placeholder]', output);
  const injectOffset = requests.length;
  await button('Create translated copy', dialog);
  const injection = (await completedRequest('/api/inject', injectOffset)).body;
  const injectedRoot = injection.injected_roots?.es;
  assert(injectedRoot, 'Injection did not report a translated copy');
  const ownedRelative = path.relative(temporary, await realpath(injectedRoot));
  assert(ownedRelative && !ownedRelative.startsWith('..') && !path.isAbsolute(ownedRelative), 'Injection escaped owned temp');
  const injected = await fingerprint(injectedRoot);
  const payload = JSON.parse(await readFile(path.join(injectedRoot, 'data/Map001.json'), 'utf8'));
  const lines = payload.events[1].pages[0].list.filter(e => e.code === 401).map(e => e.parameters[0]);
  assert.equal(lines.join('\n'), edited, 'Injected MV dialogue must contain the saved edit');
  assert.notDeepEqual(injected.filter(f => f.path.startsWith('data/')), pristine.filter(f => f.path.startsWith('data/')));
  assert.equal(injected.find(f => f.path === 'audio/subdir/sentinel.bin')?.sha256, pristine.find(f => f.path === 'audio/subdir/sentinel.bin').sha256);
  summary.verification.injectedPayloadSha256 = injected.find(f => f.path === 'data/Map001.json').sha256;
  summary.stages.push(stage);

  stage = 'package';
  await button('Open Patch → Pack', dialog);
  await fill('[role=dialog] input[placeholder="Where to write the patch zip..."]', zip);
  await fill('[role=dialog] input[placeholder="Folder of an unpatched copy of the game…"]', source);
  const packOffset = requests.length;
  await button('Pack', `${dialog}.querySelector('footer')`);
  const packed = (await completedRequest('/api/patch/pack', packOffset)).body;
  assert(packed.files_packed > 0 && packed.size_bytes > 0, 'Empty patch package');
  assert.equal(packed.tier.toLowerCase(), 'strict', 'Pristine hashes must produce strict package');
  assert.equal(path.resolve(packed.output_path), zip);
  assert((await stat(zip)).size > 0);
  summary.verification.patch = { files: packed.files_packed, sha256: digest(await readFile(zip)), tier: packed.tier };
  summary.stages.push(stage);

  stage = 'apply';
  await button('Apply', `${dialog}.querySelector('header')`);
  await fill('[role=dialog] input[placeholder="Path to the game to patch..."]', consumer);
  await fill('[role=dialog] input[placeholder="locust-*-patch.zip..."]', zip);
  await button('Apply', `${dialog}.querySelector('footer')`);
  const applied = await observeJob('patch', 'done');
  assert.equal((await completedRequest('/api/patch/apply')).body.job_id, summary.websocket.patch.jobId);
  assert(applied.report && !applied.report.dry_run, 'Patch apply must modify the consumer');
  const appliedFiles = await fingerprint(consumer);
  // Original paths must all match the injected copy; added game payloads must
  // match too. Locust's own patch receipts/backups are checked after rollback.
  const expectedPayload = injected.filter(f => !f.path.split('/').some(part => part.startsWith('.locust')));
  const actualPayload = appliedFiles.filter(f => !f.path.split('/').some(part => part.startsWith('.locust')));
  assert.deepEqual(actualPayload, expectedPayload, 'Consumer payload differs from injected output');
  assert.notEqual(actualPayload.find(f => f.path === 'data/Map001.json').sha256, pristine.find(f => f.path === 'data/Map001.json').sha256);
  summary.stages.push(stage);

  stage = 'rollback-preview';
  const previewOffset = requests.length;
  await button('Preview rollback', dialog);
  const preview = (await completedRequest('/api/patch/rollback', previewOffset)).body;
  assert(preview.restored > 0, 'Rollback preview must restore files');
  assert.deepEqual(await fingerprint(consumer), appliedFiles, 'Rollback preview changed files');
  summary.stages.push(stage);

  stage = 'rollback';
  const rollbackOffset = requests.length;
  await button('Rollback', dialog);
  const rolledBack = (await completedRequest('/api/patch/rollback', rollbackOffset)).body;
  assert(rolledBack.restored > 0 && !rolledBack.aborted_edited?.length, 'Rollback did not restore the consumer');
  assert.deepEqual(await fingerprint(consumer), pristine, 'Rollback must restore exact original paths and SHA-256 hashes');
  assert.deepEqual(await fingerprint(source), pristine, 'Pristine source changed');
  assert.deepEqual(await fingerprint(fixture), fixtureBefore, 'Repository fixture changed');
  assert.deepEqual(await fingerprint(dist), assetsBefore, 'Production assets changed');
  summary.verification.rollback = { exactPathsAndHashes: true, files: pristine.length, inventorySha256: digest(JSON.stringify(pristine)), restored: rolledBack.restored };
  summary.stages.push(stage);
  summary.verification.browserRequests = requests.length;
  assert.equal(summary.errors.length, 0, 'Unexpected browser/transport errors');
  summary.success = true;
}

async function cleanup() {
  const errors = [], descendants = [];
  const attempt = async action => { try { await action(); } catch (error) { errors.push(error.message); } };
  await attempt(async () => {
    if (cdp?.socket.readyState !== WebSocket.OPEN) return;
    cdp.listeners.clear();
    descendants.push(...(await cdp.call('SystemInfo.getProcessInfo')).processInfo.map(p => Number(p.id)));
    await cdp.call('Browser.close');
    await Promise.race([chrome.done, pause(3000)]);
  });
  cdp?.socket.close();
  await attempt(() => stop(chrome));
  await attempt(() => stop(backend));
  await attempt(async () => {
    if (!staticServer) return;
    staticServer.closeAllConnections();
    await new Promise((resolve, reject) => staticServer.close(error => error ? reject(error) : resolve()));
  });
  await attempt(async () => {
    if (!descendants.length) return;
    await pause(300);
    // Windows can deny signal-0 during process teardown. Ask the OS for that
    // specific PID in that case, rather than assuming either alive or stopped.
    const processAlive = pid => {
      try { return alive(pid); }
      catch (error) {
        if (process.platform !== 'win32' || error.code !== 'EPERM') throw error;
        const listing = execFileSync('tasklist.exe', ['/FI', `PID eq ${pid}`, '/FO', 'CSV', '/NH'], { encoding: 'utf8', windowsHide: true, timeout: 5000 });
        return new RegExp(`^"[^"]+","${pid}",`, 'm').test(listing);
      }
    };
    for (const pid of descendants.filter(processAlive)) {
      if (process.platform === 'win32') execFileSync('taskkill.exe', ['/PID', String(pid), '/T', '/F'], { windowsHide: true, stdio: 'pipe', timeout: 5000 });
      else process.kill(pid, 'SIGKILL');
    }
    await pause(300);
    assert.equal(descendants.filter(processAlive).length, 0, 'Owned Chrome descendants remain');
  });
  await attempt(async () => {
    if (!temporary) return;
    const target = await realpath(temporary), parent = await realpath(tmpdir());
    assert.equal(path.dirname(target), parent, 'Cleanup must stay in the OS temp root');
    assert(path.basename(target).startsWith('locust-ui-lifecycle-'));
    await rm(target, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
    assert.equal(await stat(target).catch(() => null), null, 'Temp directory remains');
  });
  summary.cleanup = { processesStopped: [chrome, backend].filter(Boolean).every(p => p.exited), tempDeleted: !temporary || !(await stat(temporary).catch(() => null)), errors };
  if (errors.length || !summary.cleanup.tempDeleted || !summary.cleanup.processesStopped) summary.success = false;
}

if (process.argv.includes('--help')) {
  console.log('Build: npm --prefix apps/desktop run build\nRun: LOCUST_CLI=/existing/locust CHROME_PATH=/existing/chrome node tools/ui_release_lifecycle.mjs\nNode 24+. Optional LOCUST_CHROME_NO_SANDBOX=1 for trusted local fixtures in restricted sandboxes.');
} else {
  for (const signal of ['SIGINT', 'SIGTERM']) process.once(signal, () => { interrupted = true; summary.errors.push(`Interrupted by ${signal}`); });
  try { await main(); }
  catch (error) {
    summary.success = false;
    summary.failedStage = stage;
    summary.errors.push(error.message);
    // Bounded failure details survive deletion without leaving repo artifacts.
    summary.diagnostics = { backend: backend?.error || backend?.log.slice(-1500), chrome: chrome?.error || chrome?.log.slice(-1500) };
    summary.diagnostics.downloads = [...downloads.values()];
    if (temporary) summary.diagnostics.downloadFiles = await readdir(path.join(temporary, 'downloads')).catch(() => []);
    if (cdp?.socket.readyState === WebSocket.OPEN && Date.now() < deadline) {
      try { summary.diagnostics.ui = await evaluate('document.body.innerText.slice(-2500)'); } catch { /* Browser may already be gone. */ }
      if (stage === 'export' && [...downloads.values()].some(d => d.state === 'canceled')) {
        try {
          await command('Page.navigate', { url: 'chrome://downloads/' });
          await pause(500);
          summary.diagnostics.downloadManager = await evaluate(`Array.from(document.querySelector('downloads-manager')?.items_ || []).map(item => ({state: item.state, reason: item.lastReasonText, file: item.filePath})).slice(0,3)`);
        } catch (diagnosticError) { summary.diagnostics.downloadManagerError = diagnosticError.message; }
      }
    }
  } finally { await cleanup(); }
  if (summary.errors.length) summary.success = false;
  summary.durationMs = Date.now() - started;
  console.log(JSON.stringify(summary));
  if (!summary.success) process.exitCode = 1;
}
