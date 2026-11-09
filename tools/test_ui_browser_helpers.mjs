// Unit tests for the shared production-browser helpers. Run: node --test tools/test_ui_browser_helpers.mjs
import assert from 'node:assert/strict';
import test from 'node:test';
import { chromeArguments } from './ui_browser_helpers.mjs';

const head = ['--headless=new', '--user-data-dir=P', '--remote-debugging-port=0', '--remote-debugging-address=127.0.0.1',
  '--no-first-run', '--no-default-browser-check', '--disable-background-networking', '--disable-component-update',
  '--disable-sync', '--disable-extensions'];
const crash = ['--disable-crashpad-for-testing', '--disable-crash-reporter', '--disable-breakpad'];
const tail = ['--noerrdialogs', '--password-store=basic'];

test('windows and macOS keep the established argument list', () => {
  for (const platform of ['win32', 'darwin']) {
    assert.deepEqual(chromeArguments({ profileDir: 'P', platform }), [...head, ...crash, ...tail, 'about:blank']);
    assert.deepEqual(chromeArguments({ profileDir: 'P', platform, noSandbox: true }),
      [...head, ...crash, ...tail, '--no-sandbox', '--disable-gpu', 'about:blank']);
  }
});

test('linux drops the crash-reporter switches and avoids /dev/shm', () => {
  const args = chromeArguments({ profileDir: 'P', platform: 'linux', noSandbox: true });
  for (const flag of crash) assert(!args.includes(flag), flag);
  assert.deepEqual(args, [...head, '--disable-dev-shm-usage', ...tail, '--no-sandbox', '--disable-gpu', 'about:blank']);
});
