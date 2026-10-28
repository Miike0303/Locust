#!/usr/bin/env node
/**
 * Node 24+, no packages. Build first (this runner never builds):
 *   npm --prefix apps/desktop run build
 *   cargo build -p locust-cli
 *   node tools/capture_ui.mjs [output-directory]
 * CARGO_TARGET_DIR is required and is never changed. Optional CHROME_PATH and
 * LOCUST_CLI select existing executables. Restricted Windows sandboxes can use
 * LOCUST_CHROME_NO_SANDBOX=1 for these trusted local fixtures only.
 * See docs/UI-RESPONSIVE-CYCLE.md.
 */
import { spawn, execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createServer } from 'node:http';
import { createServer as createSocket } from 'node:net';
import { readFile, writeFile, mkdir, mkdtemp, rm, stat, realpath, copyFile, readdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const dist = path.join(root, 'apps/desktop/dist');
const chromePath = process.env.CHROME_PATH || 'C:/Program Files/Google/Chrome/Application/chrome.exe';
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const manifest = { version: 1, startedAt: new Date().toISOString(), captures: [], fixtures: [], requests: [], errors: [] };
let output, temporary, backend, chrome, staticServer, cdp, session, cleanupPromise;
let interrupted = false;

async function until(probe, description, timeout = 15000) {
  const end = Date.now() + timeout;
  let last;
  while (Date.now() < end) {
    if (interrupted) throw new Error('Capture interrupted');
    try { const value = await probe(); if (value) return value; } catch (error) { last = error; }
    await pause(80);
  }
  throw new Error(`Timed out: ${description}${last ? ` (${last.message})` : ''}`);
}

function start(executable, args, env = process.env) {
  assert(!interrupted, 'Capture interrupted before process startup');
  const child = spawn(executable, args, { cwd: root, env, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
  const owned = { child, pid: child.pid, executable, log: '', exited: false };
  for (const stream of [child.stdout, child.stderr]) stream.on('data', bytes => {
    owned.log = (owned.log + bytes.toString()).slice(-24000);
  });
  owned.done = new Promise(resolve => {
    child.once('error', error => { owned.error = error.message; owned.exited = true; resolve(); });
    child.once('close', (code, signal) => { owned.code = code; owned.signal = signal; owned.exited = true; resolve(); });
  });
  return owned;
}

async function freePort() {
  const socket = createSocket();
  await new Promise((resolve, reject) => { socket.once('error', reject); socket.listen(0, '127.0.0.1', resolve); });
  const port = socket.address().port;
  await new Promise((resolve, reject) => socket.close(error => error ? reject(error) : resolve()));
  return port;
}

class DevTools {
  constructor(url) {
    this.socket = new WebSocket(url);
    this.pending = new Map();
    this.listeners = new Map();
    this.sequence = 0;
    this.ready = new Promise((resolve, reject) => {
      this.socket.addEventListener('open', resolve, { once: true });
      this.socket.addEventListener('error', () => reject(new Error(`DevTools connection failed: ${url}`)), { once: true });
    });
    this.socket.addEventListener('message', event => {
      const message = JSON.parse(event.data);
      if (message.id) {
        const pending = this.pending.get(message.id);
        if (!pending) return;
        clearTimeout(pending.timer);
        this.pending.delete(message.id);
        if (message.error) pending.reject(new Error(`${pending.method}: ${message.error.message}`));
        else pending.resolve(message.result);
      } else {
        for (const listener of this.listeners.get(message.method) || []) {
          Promise.resolve().then(() => listener(message.params)).catch(error => manifest.errors.push(error.message));
        }
      }
    });
    this.socket.addEventListener('close', () => {
      for (const pending of this.pending.values()) {
        clearTimeout(pending.timer);
        pending.reject(new Error(`DevTools closed during ${pending.method}`));
      }
      this.pending.clear();
    });
  }
  on(method, listener) {
    this.listeners.set(method, [...(this.listeners.get(method) || []), listener]);
  }
  async call(method, params = {}, sessionId) {
    await this.ready;
    const id = ++this.sequence;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error(`DevTools timeout: ${method}`)); }, 15000);
      this.pending.set(id, { resolve, reject, timer, method });
      this.socket.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
    });
  }
}
const command = (method, params = {}) => cdp.call(method, params, session);
async function evaluate(expression) {
  const result = await command('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true, userGesture: true });
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description || result.exceptionDetails.text);
  return result.result.value;
}
const waitFor = (expression, description) => until(() => evaluate(expression), description);
const q = JSON.stringify;

async function focus(selector) {
  // Establish keyboard modality so :focus-visible is exercised, including buttons.
  await command('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Tab', code: 'Tab', windowsVirtualKeyCode: 9 });
  await command('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Tab', code: 'Tab', windowsVirtualKeyCode: 9 });
  await evaluate(`(() => { const el = document.querySelector(${q(selector)}); if (!el) throw Error('Missing focus target: ' + ${q(selector)}); el.scrollIntoView({block:'nearest'}); el.focus(); })()`);
}
async function click(selector) {
  await waitFor(`!!document.querySelector(${q(selector)})`, selector);
  await evaluate(`document.querySelector(${q(selector)}).click()`);
}
async function button(text, within = 'document') {
  const expression = `Array.from(${within}.querySelectorAll('button')).find(el => el.textContent.trim() === ${q(text)})`;
  await waitFor(`!!(${expression})`, `button ${text}`);
  await evaluate(`(${expression}).click()`);
}
async function fill(selector, value) {
  await evaluate(`(() => { const el = document.querySelector(${q(selector)}); if (!el) throw Error('Missing input: ' + ${q(selector)}); const proto = el.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : el.tagName === 'SELECT' ? HTMLSelectElement.prototype : HTMLInputElement.prototype; Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, ${q(String(value))}); el.dispatchEvent(new Event(el.tagName === 'SELECT' ? 'change' : 'input', {bubbles:true})); })()`);
}

async function api(route, method = 'GET', body) {
  assert(!interrupted, 'Capture interrupted before backend request');
  const response = await fetch(`${manifest.backendUrl}${route}`, {
    method, headers: { 'Content-Type': 'application/json' },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }), signal: AbortSignal.timeout(15000),
  });
  const text = await response.text();
  assert(response.ok, `${method} ${route}: ${response.status} ${text}`);
  return text ? JSON.parse(text) : null;
}

async function serveDist() {
  // Vite preview semantics: unmodified production assets + SPA history fallback.
  const types = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml', '.png': 'image/png', '.woff2': 'font/woff2', '.ico': 'image/x-icon' };
  staticServer = createServer(async (req, res) => {
    try {
      const pathname = decodeURIComponent(new URL(req.url, 'http://localhost').pathname);
      let file = path.resolve(dist, `.${pathname}`);
      assert(!path.relative(dist, file).startsWith('..'), 'Path outside dist');
      if (!(await stat(file).catch(() => null))?.isFile()) {
        if (path.extname(pathname)) { res.writeHead(404); res.end(); return; }
        file = path.join(dist, 'index.html');
      }
      res.writeHead(200, { 'Content-Type': types[path.extname(file)] || 'application/octet-stream', 'Cache-Control': 'no-store' });
      res.end(await readFile(file));
    } catch { res.writeHead(400); res.end(); }
  });
  await new Promise((resolve, reject) => { staticServer.once('error', reject); staticServer.listen(0, '127.0.0.1', resolve); });
  manifest.frontendUrl = `http://127.0.0.1:${staticServer.address().port}`;
}

async function fingerprint(directory) {
  const files = [];
  for (const entry of (await readdir(directory, { withFileTypes: true })).sort((a, b) => a.name.localeCompare(b.name))) {
    const file = path.join(directory, entry.name);
    if (entry.isDirectory()) files.push(...await fingerprint(file));
    else if (entry.isFile()) files.push({ path: path.relative(dist, file).replaceAll('\\', '/'), sha256: digest(await readFile(file)) });
  }
  return files;
}

// Generated synthetic fixture shapes from crates/cli/tests/ja_en_es_pipeline.rs
// (Engine::RpgMakerMv) and crates/formats/tests/rpgmaker_revision.rs. No games.
async function fixture(viewport, locale) {
  const caseName = `${locale}-${viewport.width}x${viewport.height}`;
  const game = path.join(temporary, caseName, 'Synthetic UI');
  await mkdir(path.join(game, 'data'), { recursive: true });
  const message = text => [{ code: 101, indent: 0, parameters: ['', 0, 0, 2] }, { code: 401, indent: 0, parameters: [text] }];
  const map = { displayName: '', events: [null, { id: 1, name: 'Synthetic event', pages: [{ list: [
    ...message('First small line\nSecond line'), ...message('Welcome traveler!'), ...message('Welcome traveler!'),
    ...message('This row will change'), ...message('This row will be removed'), { code: 0, indent: 0, parameters: [] },
  ] }] }] };
  await writeFile(path.join(game, 'data/Map001.json'), JSON.stringify(map));
  await writeFile(path.join(game, 'data/System.json'), JSON.stringify({ gameTitle: 'Synthetic UI', currencyUnit: 'G', terms: { basic: [], commands: ['Continue', 'Options'] } }));
  await writeFile(path.join(game, 'data/Actors.json'), JSON.stringify([null, { id: 1, name: 'Hero', profile: 'A synthetic profile' }]));
  const opened = await api('/api/project/open', 'POST', { path: game, format_id: 'rpgmaker-mv' });
  const { entries } = await api('/api/strings?limit=100');
  const dialogue = entries.find(row => row.source === 'First small line\nSecond line');
  assert(dialogue?.tags.includes('dialogue') && /#cmd_\d+#msg$/.test(dialogue.id), 'Extracted MV dialogue missing');
  let duplicates = 0;
  for (const [index, row] of entries.entries()) {
    if (row.source === 'Options') continue; // One blank row for preview's fill count.
    const translation = row.source === 'Welcome traveler!' ? (++duplicates === 1 ? 'Bienvenido, viajero.' : 'Hola, caminante.')
      : row.id === dialogue.id ? 'A short line\nA much longer translation for checking the width estimate'
        : `Traduccion de prueba ${index + 1}: texto guardado para revisar.`;
    await api(`/api/strings/${encodeURIComponent(row.id)}`, 'PATCH', { translation, status: index % 2 ? 'translated' : 'reviewed' });
  }
  assert.equal(duplicates, 2);
  const seeded = (await api('/api/strings?limit=100')).entries;
  const po = ['msgid ""\nmsgstr ""\n"Content-Type: text/plain; charset=UTF-8\\n"\n'];
  for (const [index, row] of seeded.entries()) po.push(`msgctxt ${q(row.id)}\nmsgid ${q(row.source)}\nmsgstr ${q(index === 0 && row.translation ? row.translation : `Nueva traduccion ${index + 1}: un texto de ejemplo que permite revisar los cambios antes de importar.`)}\n`);
  po.push('msgctxt "synthetic-unknown-id"\nmsgid "Unknown synthetic row"\nmsgstr "Fila desconocida"\n');
  const catalog = path.join(temporary, 'preview.po');
  await writeFile(catalog, po.join('\n'));
  const font = path.join(temporary, 'synthetic-ascii.ttf');
  await copyFile(path.join(root, 'crates/cli/tests/fixtures/synthetic-ascii.ttf'), font);
  const projectKey = JSON.stringify([opened.database_path, opened.project_path, opened.format_id]);
  manifest.fixtures.push({ case: caseName, project: opened, rows: seeded.length, catalogSha256: digest(await readFile(catalog)) });
  return { game, map, opened, seeded, dialogue, catalog, font, projectKey };
}

async function navigate(route) {
  await command('Page.navigate', { url: `${manifest.frontendUrl}${route}` });
  await waitFor(`document.readyState === 'complete' && !!document.querySelector('main')`, route);
}

async function capture(scenario, viewport, locale, evidence = {}) {
  await evaluate('document.fonts.ready.then(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))))');
  const diagnostics = await evaluate(`(() => {
    const rect = el => { const r = el.getBoundingClientRect(); return { x:r.x, y:r.y, width:r.width, height:r.height, right:r.right, bottom:r.bottom }; };
    const visible = el => { const r = el.getBoundingClientRect(); return r.width > 0 && r.height > 0 && r.bottom > 0 && r.y < innerHeight && getComputedStyle(el).visibility !== 'hidden'; };
    const active = document.activeElement, style = getComputedStyle(active);
    return {
      document: { width:innerWidth, height:innerHeight, scrollWidth:document.documentElement.scrollWidth, scrollHeight:document.documentElement.scrollHeight },
      focus: { tag:active.tagName, text:active.getAttribute('aria-label') || active.textContent.trim().slice(0,100), id:active.id, rect:rect(active), focusVisible:active.matches(':focus-visible'), outline:style.outline, boxShadow:style.boxShadow },
      dialogs: Array.from(document.querySelectorAll('[role=dialog]')).filter(visible).map(el => ({rect:rect(el), scrolls:Array.from(el.querySelectorAll('*')).filter(child => child.scrollHeight > child.clientHeight + 1 && /auto|scroll/.test(getComputedStyle(child).overflowY)).map(child => ({tag:child.tagName, rect:rect(child), scrollTop:child.scrollTop, clientHeight:child.clientHeight, scrollHeight:child.scrollHeight}))})),
      horizontalScroll: Array.from(document.querySelectorAll('main *, [role=dialog] *, [role=region]')).filter(el => visible(el) && el.clientWidth > 2 && el.scrollWidth > el.clientWidth + 2 && !['INPUT','TEXTAREA','SELECT'].includes(el.tagName)).map(el => ({tag:el.tagName, text:el.textContent.trim().slice(0,100), overflow:getComputedStyle(el).overflowX, rect:rect(el), clientWidth:el.clientWidth, scrollWidth:el.scrollWidth})).slice(0,30),
    };
  })()`);
  const file = `${locale}/${viewport.width}x${viewport.height}/${scenario}.png`;
  await mkdir(path.dirname(path.join(output, file)), { recursive: true });
  const { data } = await command('Page.captureScreenshot', { format: 'png', fromSurface: true, captureBeyondViewport: false });
  const bytes = Buffer.from(data, 'base64');
  assert.equal(bytes.readUInt32BE(16), viewport.width, 'PNG width');
  assert.equal(bytes.readUInt32BE(20), viewport.height, 'PNG height');
  await writeFile(path.join(output, file), bytes);
  const bounded = r => r.x >= -1 && r.y >= -1 && r.right <= viewport.width + 1 && r.bottom <= viewport.height + 1;
  const checks = { noDocumentOverflow: diagnostics.document.scrollWidth === viewport.width,
    boundedDialogs: diagnostics.dialogs.every(d => bounded(d.rect)),
    visibleKeyboardFocus: diagnostics.focus.focusVisible && bounded(diagnostics.focus.rect)
      && (diagnostics.focus.boxShadow !== 'none' || !diagnostics.focus.outline.includes(' none ')) };
  manifest.captures.push({ scenario, viewport, locale, gitHead: manifest.gitHead, timestamp: new Date().toISOString(), path: file, sha256: digest(bytes), evidence, checks, diagnostics });
  console.log(`Captured ${file}`);
}

async function scenarios(viewport, locale) {
  const f = await fixture(viewport, locale);
  const es = locale === 'es';
  const snap = (name, evidence) => capture(name, viewport, locale, evidence);
  const dialog = `document.querySelector('[role=dialog]')`;
  const { identifier } = await command('Page.addScriptToEvaluateOnNewDocument', { source: `localStorage.clear(); localStorage.setItem('locust.ui.language', ${q(locale)}); localStorage.setItem('locust.workflowGuide.dismissed','true'); localStorage.setItem(${q(`locust.textFit.v1:${f.projectKey}`)}, ${q(JSON.stringify({ fontPath: f.font, fontSize: '24', usableWidth: '900' }))});` });
  try {
    await navigate('/editor');
    await waitFor(`Array.from(document.querySelectorAll('tbody tr')).some(row => row.textContent.includes('First small line'))`, 'seeded editor');
    await button(es ? 'Flujo rápido' : 'Fast workflow');
    await waitFor(`!!document.querySelector('[role=dialog] [data-fast-workflow]')`, 'fast workflow stages');
    await click('[role=dialog] details > summary');
    await focus('[role=dialog] details > summary');
    await snap('fast-workflow', { stages: ['translate', 'pivot', 'translateTarget', 'validate', 'inject', 'pack'], providerPreset: 'google', compression: 'fast' });
    await button(es ? 'Cerrar' : 'Close', dialog);
    await evaluate(`Array.from(document.querySelectorAll('tbody tr')).find(row => row.textContent.includes('First small line')).click()`);
    await waitFor(`!!document.querySelector('[data-text-fit-preview]')`, 'Width estimate');
    await click('[data-text-fit-preview] button[aria-expanded]');
    const fitText = es ? 'avance estimado dentro del ancho elegido' : 'estimated advance within chosen width';
    await waitFor(`document.querySelector('[data-text-fit-preview]').textContent.includes(${q(fitText)})`, 'real within-width estimate');
    await focus('[data-text-fit-preview] input[id$="-width"]');
    await snap('editor-width-within', { entryId: f.dialogue.id, usableWidth: 900 });
    await fill('[data-text-fit-preview] input[id$="-width"]', '100');
    const exceeds = es ? 'el avance estimado supera el ancho elegido' : 'estimated advance exceeds chosen width';
    await waitFor(`document.querySelector('[data-text-fit-preview]').textContent.includes(${q(exceeds)})`, 'real exceeds-width estimate');
    await focus('[data-text-fit-preview] input[id$="-width"]');
    await snap('editor-width-exceeds', { entryId: f.dialogue.id, usableWidth: 100 });

    // A fresh editor closes the narrow viewport's modal DetailPanel first.
    await navigate('/editor');
    await click('button[title="Ctrl+E"]');
    await waitFor(`document.querySelector('[role=dialog] select[id$="-file"]')?.options.length > 1`, 'export facets');
    const storedFile = f.dialogue.file_path;
    await fill('[role=dialog] select[id$="-file"]', storedFile);
    // Keep exactly translated + reviewed, by interacting with the five checkboxes.
    await evaluate(`Array.from(document.querySelectorAll('[role=dialog] input[type=checkbox]')).forEach((el,index) => { if (![1,2].includes(index) && el.checked) el.click(); })`);
    const expectedCount = f.seeded.filter(row => row.file_path === storedFile && ['translated','reviewed'].includes(row.status)).length;
    await waitFor(`document.querySelector('[role=dialog] [role=status]').textContent.includes(${q(String(expectedCount))})`, 'live export count');
    await focus('[role=dialog] select[id$="-file"]');
    await snap('export-scope', { file: storedFile, statuses: ['translated', 'reviewed'], count: expectedCount });
    await evaluate(`Array.from(document.querySelectorAll('[role=dialog] input[type=checkbox]')).forEach((el,index) => { if ((index === 3) !== el.checked) el.click(); })`);
    await waitFor(`document.querySelector('[role=dialog] [role=status]').textContent.includes('0')`, 'zero matching strings');
    assert(await evaluate(`Array.from(${dialog}.querySelectorAll('button')).at(-1).disabled`), 'Zero-match export must be disabled');
    await focus('[role=dialog] input[type=checkbox]:checked');
    await snap('export-zero', { file: storedFile, statuses: ['approved'], count: 0 });

    await button(es ? 'Importar' : 'Import', dialog);
    const { root: documentRoot } = await command('DOM.getDocument');
    const { nodeId } = await command('DOM.querySelector', { nodeId: documentRoot.nodeId, selector: '[role=dialog] input[type=file]' });
    await command('DOM.setFileInputFiles', { nodeId, files: [f.catalog] });
    await waitFor(`${dialog}.textContent.includes('preview.po')`, 'browser catalog selection');
    await button(es ? 'Vista previa de importación' : 'Preview import', dialog);
    const summary = '[role=dialog] [role=status] .rounded';
    await waitFor(`!!document.querySelector(${q(summary)})`, 'keep-existing preview summary');
    await focus('[role=dialog] input[type=radio]:checked');
    await snap('import-keep', { keepExisting: true, summary: await evaluate(`document.querySelector(${q(summary)}).textContent`) });
    await click('[role=dialog] input[type=radio]:not(:checked)');
    await waitFor(`!!document.querySelector('[role=dialog] table tbody tr')`, 'replace preview table');
    await focus('[role=dialog] input[type=radio]:checked');
    await snap('import-replace', { keepExisting: false, summary: await evaluate(`document.querySelector(${q(summary)}).textContent`) });
    const region = '[role=dialog] [role=region]';
    await focus(region);
    await evaluate(`document.querySelector(${q(region)}).scrollTop = document.querySelector(${q(region)}).scrollHeight`);
    await snap('import-replace-table', { keepExisting: false, tableScroll: 'bottom' });

    const affected = f.seeded.find(row => row.translation && row.source !== 'Welcome traveler!');
    await api(`/api/strings/${encodeURIComponent(affected.id)}`, 'PATCH', { translation: 'Changed after the preview was computed.' });
    const beforeApply = (await api('/api/strings?limit=100')).entries;
    const requestOffset = manifest.requests.length;
    await button(es ? 'Aplicar importación' : 'Apply import', dialog);
    await waitFor(`!!document.querySelector('[role=dialog] [role=alert]')`, 'stale plan error');
    assert(manifest.requests.slice(requestOffset).some(request => request.url.includes('/api/import/po/apply') && request.status === 409), 'Expected genuine stale-plan HTTP 409');
    assert.deepEqual((await api('/api/strings?limit=100')).entries, beforeApply, 'Stale apply must not change rows');
    await focus('[role=dialog] [role=alert] button');
    await snap('import-stale', { httpStatus: 409, rowsUnchanged: true });

    await api('/api/validate', 'POST');
    const issues = await api('/api/validation/issues?kind=InconsistentTranslation&limit=100');
    assert(issues.total >= 2, 'InconsistentTranslation fixture missing');
    await navigate('/qa');
    const label = es ? 'Traducción incoherente' : 'Inconsistent translation';
    await waitFor(`Array.from(document.querySelectorAll('header button')).some(el => el.textContent.includes(${q(label)}))`, 'advisory chip');
    await evaluate(`Array.from(document.querySelectorAll('header button')).find(el => el.textContent.includes(${q(label)})).click()`);
    await waitFor(`document.querySelectorAll('tbody tr').length === ${issues.total}`, 'filtered QA issues');
    await focus('header button[aria-pressed=true]');
    await snap('qa-inconsistent', { kind: 'InconsistentTranslation', count: issues.total });

    // Reopening changed synthetic text creates the real, persistent merge warning.
    f.map.events[1].pages[0].list[7].parameters[0] = 'Changed synthetic source';
    f.map.events[1].pages[0].list.splice(8, 2);
    await writeFile(path.join(f.game, 'data/Map001.json'), JSON.stringify(f.map));
    await navigate('/');
    await waitFor(`Array.from(document.querySelectorAll('button')).some(el => el.textContent.trim() === ${q(es ? 'Abrir carpeta del juego' : 'Open game folder')})`, 'Home open-folder control');
    let promptHandled = false;
    const handlePrompt = async event => {
      if (event.type !== 'prompt') return;
      promptHandled = true;
      await command('Page.handleJavaScriptDialog', { accept: true, promptText: f.game });
    };
    cdp.on('Page.javascriptDialogOpening', handlePrompt);
    await button(es ? 'Abrir carpeta del juego' : 'Open game folder');
    await waitFor(`!!document.querySelector('[role=region] [role=status]')`, 'merge warning toast');
    assert(promptHandled, 'Expected browser folder prompt');
    const toastSelector = '[role=region] [role=status]';
    const message = await evaluate(`document.querySelector(${q(toastSelector)}).textContent`);
    assert(message.length > 100, 'Warning should wrap over several lines');
    // Warnings have no expiry. Wait beyond the normal success/info lifetime,
    // without hovering or focusing the toast, then prove it is still present.
    await pause(4200);
    assert.equal(await evaluate(`document.querySelector(${q(toastSelector)})?.textContent`), message);
    await focus(`${toastSelector} button`);
    await snap('warning-toast', { message, persistentAfterMs: 4200, trigger: 'real synthetic project merge' });
    await click(`${toastSelector} button`);
  } finally {
    await command('Page.removeScriptToEvaluateOnNewDocument', { identifier });
    cdp.listeners.delete('Page.javascriptDialogOpening');
  }
}

function alive(pid) {
  try { process.kill(pid, 0); return true; }
  catch (error) { if (error.code === 'ESRCH') return false; throw error; }
}
async function stop(owned) {
  if (!owned || owned.exited) return;
  if (process.platform === 'win32') {
    try { execFileSync('taskkill.exe', ['/PID', String(owned.pid), '/T', '/F'], { windowsHide: true, stdio: 'pipe', timeout: 10000 }); }
    catch (error) { if (!owned.exited) owned.stopError = error.message; }
  } else owned.child.kill('SIGTERM');
  await Promise.race([owned.done, pause(5000)]);
  if (!owned.exited) { owned.child.kill('SIGKILL'); await Promise.race([owned.done, pause(5000)]); }
  assert(owned.exited, `Owned process ${owned.pid} did not exit`);
}

function cleanup() {
  if (cleanupPromise) return cleanupPromise;
  cleanupPromise = (async () => {
    const errors = [], result = { processes: [], temporary, tempDeleted: !temporary };
    const attempt = async action => { try { await action(); } catch (error) { errors.push(error.message); } };
    let descendants = [];
    await attempt(async () => {
      if (!cdp || cdp.socket.readyState !== WebSocket.OPEN) return;
      // Browser-scoped process IDs avoid system-wide discovery/CIM permissions.
      descendants = (await cdp.call('SystemInfo.getProcessInfo')).processInfo.map(p => Number(p.id));
    });
    await attempt(async () => {
      if (cdp?.socket.readyState === WebSocket.OPEN) {
        await cdp.call('Browser.close');
        await Promise.race([chrome.done, pause(5000)]);
      }
    });
    cdp?.socket.close();
    await attempt(() => stop(chrome));
    await attempt(() => stop(backend));
    await attempt(async () => {
      if (!staticServer) return;
      staticServer.closeAllConnections();
      await new Promise((resolve, reject) => staticServer.close(error => error && error.code !== 'ERR_SERVER_NOT_RUNNING' ? reject(error) : resolve()));
    });
    await attempt(async () => {
      if (!descendants.length) return;
      // Allow child exit notifications to settle after Browser.close.
      await pause(300);
      const remaining = descendants.filter(alive);
      for (const pid of remaining) {
        if (process.platform === 'win32') execFileSync('taskkill.exe', ['/PID', String(pid), '/T', '/F'], { windowsHide: true, stdio: 'pipe', timeout: 10000 });
        else process.kill(pid, 'SIGKILL');
      }
      if (remaining.length) await pause(300);
      result.chromeProcessesObserved = descendants;
      result.chromeProcessesRemaining = descendants.filter(alive);
      assert.equal(result.chromeProcessesRemaining.length, 0, 'Owned Chrome descendants remain');
    });
    for (const [name, owned] of [['chrome', chrome], ['backend', backend]]) if (owned) {
      result.processes.push({ name, pid: owned.pid, exited: owned.exited, code: owned.code, signal: owned.signal, error: owned.error });
      if (output) await attempt(() => writeFile(path.join(output, `${name}.log`), owned.log));
    }
    await attempt(async () => {
      if (!temporary) return;
      const tempParent = await realpath(tmpdir()), target = await realpath(temporary);
      // Only delete this mkdtemp result, directly under the OS temp root.
      assert.equal(path.dirname(target), tempParent);
      assert(path.basename(target).startsWith('locust-ui-capture-'));
      await rm(target, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
      result.tempDeleted = !(await stat(target).catch(() => null));
      assert(result.tempDeleted, 'Owned temp directory remains');
    });
    manifest.cleanup = { ...result, errors, ok: errors.length === 0 && result.tempDeleted && result.processes.every(p => p.exited) };
    manifest.finishedAt = new Date().toISOString();
    if (output) await writeFile(path.join(output, 'manifest.json'), `${JSON.stringify(manifest, null, 2)}\n`);
    if (!manifest.cleanup.ok) process.exitCode = 1;
  })();
  return cleanupPromise;
}

async function main() {
  if (process.argv.includes('--help')) {
    console.log('Build first: npm --prefix apps/desktop run build; cargo build -p locust-cli\nUsage: node tools/capture_ui.mjs [output-directory]\nRequires Node 24+, CARGO_TARGET_DIR. Optional CHROME_PATH / LOCUST_CLI.');
    return;
  }
  assert(Number(process.versions.node.split('.')[0]) >= 24, 'Node 24+ required');
  assert(process.env.CARGO_TARGET_DIR, 'CARGO_TARGET_DIR must point to the existing build directory');
  assert(process.argv.length <= 3, 'Usage: node tools/capture_ui.mjs [output-directory]');
  manifest.gitHead = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8', windowsHide: true }).trim();
  manifest.gitDiffSha256 = digest(execFileSync('git', ['diff', '--', 'apps/desktop/src'], { cwd: root, windowsHide: true }));
  output = path.resolve(root, process.argv[2] || `.test-tmp/ui-captures/${manifest.gitHead}`);
  await mkdir(output, { recursive: true });
  const cli = process.env.LOCUST_CLI || path.resolve(process.env.CARGO_TARGET_DIR, 'debug', process.platform === 'win32' ? 'locust.exe' : 'locust');
  for (const file of [chromePath, cli, path.join(dist, 'index.html')]) assert((await stat(file)).isFile(), `Missing prerequisite: ${file}`);
  manifest.inputs = { runnerSha256: digest(await readFile(fileURLToPath(import.meta.url))), cli, cliSha256: digest(await readFile(cli)), assets: await fingerprint(dist) };
  temporary = await mkdtemp(path.join(tmpdir(), 'locust-ui-capture-'));
  const profile = path.join(temporary, 'data');
  await mkdir(profile);
  // Explicitly replace inherited backup/config overrides too. No real profile.
  const env = { ...process.env, LOCUST_DATA_DIR: profile, LOCUST_CONFIG: path.join(profile, 'config.json'), LOCUST_BACKUP_ROOT: path.join(profile, 'backups') };
  const port = await freePort();
  manifest.backendUrl = `http://127.0.0.1:${port}`;
  backend = start(cli, ['server', '--port', String(port)], env);
  await until(async () => {
    if (backend.exited) throw new Error(`Backend exited: ${backend.error || backend.log}`);
    return (await fetch(`${manifest.backendUrl}/health`, { signal: AbortSignal.timeout(500) })).ok;
  }, 'isolated real backend');
  await serveDist();
  chrome = start(chromePath, ['--headless=new', `--user-data-dir=${path.join(temporary, 'chrome')}`, '--remote-debugging-port=0', '--remote-debugging-address=127.0.0.1', '--no-first-run', '--no-default-browser-check', '--disable-background-networking', '--disable-component-update', '--disable-sync', '--disable-extensions', '--disable-crashpad-for-testing', '--disable-crash-reporter', '--disable-breakpad', '--noerrdialogs', '--password-store=basic', ...(process.env.LOCUST_CHROME_NO_SANDBOX === '1' ? ['--no-sandbox', '--disable-gpu'] : []), 'about:blank']);
  const websocket = await until(async () => {
    if (chrome.exited) throw new Error(`Chrome exited: ${chrome.error || chrome.log}`);
    const lines = (await readFile(path.join(temporary, 'chrome/DevToolsActivePort'), 'utf8')).trim().split(/\r?\n/);
    return `ws://127.0.0.1:${lines[0]}${lines[1]}`;
  }, 'Chrome DevToolsActivePort');
  cdp = new DevTools(websocket);
  manifest.browser = { executable: chromePath, noSandbox: process.env.LOCUST_CHROME_NO_SANDBOX === '1', ...await cdp.call('Browser.getVersion') };
  const { targetId } = await cdp.call('Target.createTarget', { url: 'about:blank' });
  ({ sessionId: session } = await cdp.call('Target.attachToTarget', { targetId, flatten: true }));
  await command('Page.enable');
  await command('Runtime.enable');
  await command('Network.enable');
  cdp.on('Runtime.exceptionThrown', event => manifest.errors.push(event.exceptionDetails.exception?.description || event.exceptionDetails.text));
  cdp.on('Network.responseReceived', event => {
    if (event.response.url.includes('/api/')) manifest.requests.push({ url: event.response.url, status: event.response.status });
  });
  // Production api.ts hardcodes localhost:7842. CDP changes only the destination;
  // every response comes from our real backend. Never contact an existing 7842.
  cdp.on('Fetch.requestPaused', async event => {
    const url = new URL(event.request.url);
    assert(url.hostname === 'localhost' && url.port === '7842');
    await command('Fetch.continueRequest', { requestId: event.requestId, url: `${manifest.backendUrl}${url.pathname}${url.search}` });
  });
  await command('Fetch.enable', { patterns: [{ urlPattern: 'http://localhost:7842/*', requestStage: 'Request' }] });
  for (const locale of ['en', 'es']) for (const viewport of [{ width: 1024, height: 700 }, { width: 1440, height: 900 }]) {
    console.log(`Starting ${locale} ${viewport.width}x${viewport.height}`);
    await command('Emulation.setDeviceMetricsOverride', { ...viewport, deviceScaleFactor: 1, mobile: false });
    await scenarios(viewport, locale);
  }
  assert.equal(manifest.captures.length, 44, 'Complete scenario × viewport × locale matrix');
  assert.equal(manifest.errors.length, 0, 'Unexpected browser/DevTools errors');
  assert(manifest.requests.every(request => request.status < 400 || (request.status === 409 && request.url.includes('/api/import/po/apply'))), 'Unexpected backend HTTP error');
  manifest.success = true;
}

for (const signal of ['SIGINT', 'SIGTERM']) process.once(signal, () => {
  interrupted = true;
  manifest.errors.push(`Interrupted by ${signal}`);
  process.exitCode = 130;
  // Let the active awaited operation settle before finally cleans up, so a
  // signal cannot race startup and leave a newly created process/temp tree.
});
try { await main(); }
catch (error) { manifest.success = false; manifest.errors.push(error.stack || error.message); console.error(error.stack || error); process.exitCode = 1; }
finally { await cleanup(); }
if (output) console.log(`Manifest: ${path.join(output, 'manifest.json')} (cleanup ${manifest.cleanup.ok ? 'OK' : 'FAILED'})`);
