// Shared production-browser plumbing. No packages, fixtures or response stubs.
import { spawn, execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createServer } from 'node:http';
import { createServer as createSocket } from 'node:net';
import { readFile, stat, readdir } from 'node:fs/promises';
import path from 'node:path';
import assert from 'node:assert/strict';

export const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
export const digest = bytes => createHash('sha256').update(bytes).digest('hex');

export function startProcess(executable, args, env = process.env, { cwd, interrupted = false } = {}) {
  assert(!interrupted, 'Capture interrupted before process startup');
  const child = spawn(executable, args, { cwd, env, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
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

// One Chrome argument list for every production-browser runner.
export function chromeArguments({ profileDir, platform = process.platform, noSandbox = false }) {
  // Linux omits the crash-reporter switches and avoids /dev/shm: hosted Ubuntu Chrome killed its
  // network service with "FD ownership violation" (Actions run 41552463447). Hypothesis, unverified locally.
  const crash = platform === 'linux'
    ? ['--disable-dev-shm-usage']
    : ['--disable-crashpad-for-testing', '--disable-crash-reporter', '--disable-breakpad'];
  return ['--headless=new', `--user-data-dir=${profileDir}`,
    '--remote-debugging-port=0', '--remote-debugging-address=127.0.0.1', '--no-first-run', '--no-default-browser-check',
    '--disable-background-networking', '--disable-component-update', '--disable-sync', '--disable-extensions',
    ...crash, '--noerrdialogs', '--password-store=basic',
    ...(noSandbox ? ['--no-sandbox', '--disable-gpu'] : []), 'about:blank'];
}

export async function freePort() {
  const socket = createSocket();
  await new Promise((resolve, reject) => { socket.once('error', reject); socket.listen(0, '127.0.0.1', resolve); });
  const port = socket.address().port;
  await new Promise((resolve, reject) => socket.close(error => error ? reject(error) : resolve()));
  return port;
}

export class DevTools {
  constructor(url, onError) {
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
          Promise.resolve().then(() => listener(message.params)).catch(error => onError(error));
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
export async function serveProduction(dist) {
  // Vite preview semantics: unmodified production assets + SPA history fallback.
  const types = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml', '.png': 'image/png', '.woff2': 'font/woff2', '.ico': 'image/x-icon' };
  const staticServer = createServer(async (req, res) => {
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
  return staticServer;
}

export async function fingerprint(directory, base = directory) {
  const files = [];
  for (const entry of (await readdir(directory, { withFileTypes: true })).sort((a, b) => a.name.localeCompare(b.name))) {
    const file = path.join(directory, entry.name);
    if (entry.isDirectory()) files.push(...await fingerprint(file, base));
    else if (entry.isFile()) files.push({ path: path.relative(base, file).replaceAll('\\', '/'), sha256: digest(await readFile(file)) });
  }
  return files;
}

export function alive(pid) {
  try { process.kill(pid, 0); return true; }
  catch (error) { if (error.code === 'ESRCH') return false; throw error; }
}
export async function stop(owned) {
  if (!owned || owned.exited) return;
  if (process.platform === 'win32') {
    try { execFileSync('taskkill.exe', ['/PID', String(owned.pid), '/T', '/F'], { windowsHide: true, stdio: 'pipe', timeout: 10000 }); }
    catch (error) { if (!owned.exited) owned.stopError = error.message; }
  } else owned.child.kill('SIGTERM');
  await Promise.race([owned.done, pause(5000)]);
  if (!owned.exited) { owned.child.kill('SIGKILL'); await Promise.race([owned.done, pause(5000)]); }
  assert(owned.exited, `Owned process ${owned.pid} did not exit`);
}

