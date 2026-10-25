// Standalone Node test for Android WebView's injected touch controls.
// Run: node --test tests/touch-controls.test.mjs
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { test } from 'node:test';
import vm from 'node:vm';

const sourcePath = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '../android/turbo-kart-mobile/touch-source/touch-controls.js'
);
const source = readFileSync(sourcePath, 'utf8');

class FakeElement {
  constructor(tag) {
    this.tag = tag;
    this.className = '';
    this.hidden = false;
    this.dataset = {};
    this.children = [];
    this.listeners = new Map();
    this.classes = new Set();
    this.classList = {
      add: (name) => this.classes.add(name),
      remove: (name) => this.classes.delete(name),
      contains: (name) => this.classes.has(name),
    };
  }
  append(...nodes) { this.children.push(...nodes); }
  appendChild(node) { this.children.push(node); return node; }
  remove() { this.removed = true; }
  setAttribute(k, v) { this[k] = v; }
  addEventListener(k, fn) {
    if (!this.listeners.has(k)) this.listeners.set(k, []);
    this.listeners.get(k).push(fn);
  }
  removeEventListener(k, fn) {
    this.listeners.set(k, (this.listeners.get(k) || []).filter(x => x !== fn));
  }
  trigger(k, attributes = {}) {
    const event = {
      type: k,
      pointerId: attributes.pointerId ?? 1,
      pointerType: attributes.pointerType ?? 'touch',
      isPrimary: attributes.isPrimary ?? true,
      preventDefault() {},
      stopPropagation() {},
    };
    for (const fn of this.listeners.get(k) || []) fn(event);
  }
  setPointerCapture() {}
}

function setup(state = 'racing') {
  const emitted = [];
  const body = new FakeElement('body');
  const head = new FakeElement('head');
  body.dataset.state = state;
  const document = {
    head, body, hidden: false,
    createElement: tag => new FakeElement(tag),
    listeners: new Map(),
    addEventListener(name, fn) {
      if (!this.listeners.has(name)) this.listeners.set(name, []);
      this.listeners.get(name).push(fn);
    },
    removeEventListener(name, fn) {
      this.listeners.set(name, (this.listeners.get(name) || []).filter(x => x !== fn));
    },
    trigger(name) { for (const fn of this.listeners.get(name) || []) fn(); },
  };
  const window = {
    __game: { state },
    listeners: new Map(),
    setInterval(fn) { this.tick = fn; return 1; },
    clearInterval() { this.tick = null; },
    addEventListener(name, fn) {
      if (!this.listeners.has(name)) this.listeners.set(name, []);
      this.listeners.get(name).push(fn);
    },
    removeEventListener(name, fn) {
      this.listeners.set(name, (this.listeners.get(name) || []).filter(x => x !== fn));
    },
    trigger(name) { for (const fn of this.listeners.get(name) || []) fn(); },
    dispatchEvent(e) { emitted.push({ type: e.type, code: e.code, key: e.key }); },
  };
  class KeyboardEvent {
    constructor(type, values) { this.type = type; Object.assign(this, values); }
  }
  vm.runInNewContext(source, { window, document, KeyboardEvent });
  const overlay = body.children.find(x => x.id === 'tk-touch-controls');
  const find = action => {
    const queue = [...overlay.children];
    while (queue.length) {
      const current = queue.shift();
      if (current.dataset.action === action) return current;
      queue.push(...current.children);
    }
    throw Error('Missing touch control: ' + action);
  };
  return { window, document, overlay, find, emitted };
}

test('two-finger steering and acceleration use existing keyboard codes independently', () => {
  const game = setup();
  const left = game.find('left');
  const go = game.find('accelerate');
  assert.equal(game.overlay.hidden, false);
  left.trigger('pointerdown', { pointerId: 1 });
  go.trigger('pointerdown', { pointerId: 2, isPrimary: false });
  assert.deepEqual([...game.window.TurboKartTouch.heldActions].sort(), ['ArrowLeft', 'ArrowUp']);
  left.trigger('pointerup', { pointerId: 1 });
  assert.deepEqual([...game.window.TurboKartTouch.heldActions], ['ArrowUp']);
  go.trigger('pointercancel', { pointerId: 2 });
  assert.deepEqual([...game.window.TurboKartTouch.heldActions], []);
  assert.deepEqual(game.emitted, [
    { type: 'keydown', code: 'ArrowLeft', key: 'ArrowLeft' },
    { type: 'keydown', code: 'ArrowUp', key: 'ArrowUp' },
    { type: 'keyup', code: 'ArrowLeft', key: 'ArrowLeft' },
    { type: 'keyup', code: 'ArrowUp', key: 'ArrowUp' },
  ]);
});

test('item is a short keyboard edge and never sticks after focus loss', () => {
  const game = setup();
  game.find('item').trigger('pointerdown', { pointerId: 11 });
  game.find('item').trigger('pointerup', { pointerId: 11 });
  game.find('drift').trigger('pointerdown', { pointerId: 12 });
  game.window.trigger('blur');
  assert.deepEqual(game.emitted.map(x => x.type + ':' + x.code), [
    'keydown:KeyE', 'keyup:KeyE', 'keydown:Space', 'keyup:Space',
  ]);
  assert.equal(game.window.TurboKartTouch.heldActions.length, 0);
});

test('controls hide outside a race and release throttle when state changes', () => {
  const game = setup('title');
  assert.equal(game.overlay.hidden, true);
  game.window.__game.state = 'countdown';
  game.window.tick();
  assert.equal(game.overlay.hidden, false);
  game.find('accelerate').trigger('pointerdown', { pointerId: 7 });
  game.window.__game.state = 'paused';
  game.window.tick();
  assert.equal(game.overlay.hidden, true);
  assert.deepEqual(game.emitted.map(x => x.type + ':' + x.code), [
    'keydown:ArrowUp', 'keyup:ArrowUp',
  ]);
});

test('disposal releases held keys and allows a fresh touch overlay', () => {
  const game = setup();
  game.find('right').trigger('pointerdown', { pointerId: 3 });
  game.window.TurboKartTouch.dispose();
  assert.equal(game.window.TurboKartTouch, undefined);
  assert.equal(game.window.tick, null);
  assert.equal(game.overlay.removed, true);
  assert.equal(game.emitted.at(-1).type, 'keyup');
});
