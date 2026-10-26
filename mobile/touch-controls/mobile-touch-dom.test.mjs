import { test } from 'node:test';
import { strict as assert } from 'node:assert';
import { attachTurboKartTouchControls } from './mobile-touch.mjs';

// Minimal DOM test harness: no Android SDK or browser dependency is needed
// to verify touch-zone placement, pointer dispatch and teardown.
class MockElement {
  constructor(tagName) {
    this.tagName = tagName;
    this.id = '';
    this.textContent = '';
    this.className = '';
    this.children = [];
    this.events = new Map();
    this.attributes = {};
    this.dataset = {};
    const classes = new Set();
    this.classList = {
      contains: name => classes.has(name),
      add: name => classes.add(name),
      remove: name => classes.delete(name),
      toggle: (name, enabled) => (enabled ? classes.add(name) : classes.delete(name)),
    };
  }
  append(node) { this.children.push(node); }
  remove() { this.removed = true; }
  setAttribute(key, val) { this.attributes[key] = val; }
  setPointerCapture() {}
  addEventListener(name, fn) {
    if (!this.events.has(name)) this.events.set(name, []);
    this.events.get(name).push(fn);
  }
  removeEventListener(name, fn) {
    this.events.set(name, (this.events.get(name) ?? []).filter(x => x !== fn));
  }
  fire(name, props = {}) {
    const e = {
      type: name, pointerId: 1, clientX: 100, clientY: 80,
      timeStamp: 100, detail: 1, code: '',
      preventDefault() {}, stopPropagation() {},
      ...props,
    };
    for (const fn of this.events.get(name) ?? []) fn(e);
  }
}

function harness() {
  const doc = {
    head: new MockElement('head'),
    body: new MockElement('body'),
    visibilityState: 'visible',
    inputEvents: [],
    events: new Map(),
    createElement(name) { return new MockElement(name); },
    dispatchEvent(e) { this.inputEvents.push([e.type, e.key, e.code]); },
    addEventListener: MockElement.prototype.addEventListener,
    removeEventListener: MockElement.prototype.removeEventListener,
  };
  doc.body.dataset.state = 'countdown';

  const timeouts = new Map();
  const intervals = new Map();
  let id = 0;
  const win = {
    innerWidth: 1000,
    clock: 0,
    performance: { now() { return win.clock; } },
    events: new Map(),
    addEventListener: MockElement.prototype.addEventListener,
    removeEventListener: MockElement.prototype.removeEventListener,
    setTimeout(fn) { const timer = ++id; timeouts.set(timer, fn); return timer; },
    clearTimeout(timer) { timeouts.delete(timer); },
    setInterval(fn) { const timer = ++id; intervals.set(timer, fn); return timer; },
    clearInterval(timer) { intervals.delete(timer); },
    KeyboardEvent: class {
      constructor(type, opts) { this.type = type; Object.assign(this, opts); }
    },
  };

  const detach = attachTurboKartTouchControls(win, doc);
  const overlay = doc.body.children.find(node => node.id === 'tk-touch-controls');
  const left = overlay.children.find(node => node.id === 'tk-touch-left');
  const right = overlay.children.find(node => node.id === 'tk-touch-right');
  const actions = overlay.children.find(node => node.id === 'tk-touch-actions');
  const fire = actions.children.find(node => node.id === 'tk-touch-fire');
  const look = actions.children.find(node => node.id === 'tk-touch-look');
  const advance = (phase, time) => {
    doc.body.dataset.state = phase;
    win.clock = time;
    for (const fn of [...intervals.values()]) fn();
  };
  return { win, doc, overlay, left, right, actions, fire, look, timeouts, intervals, detach, advance };
}

test('steering halves are transparent with no visible labels or pressed-state colours', () => {
  const h = harness();
  try {
    assert.equal(h.left.textContent, '');
    assert.equal(h.right.textContent, '');
    assert.equal(h.overlay.classList.contains('tk-active'), true);
    assert.equal(h.doc.head.children[0].textContent.includes('width:50%'), true);
    assert.equal(h.doc.head.children[0].textContent.includes('background:transparent'), true);
    assert.equal(h.doc.head.children[0].textContent.includes('tk-held'), false);
    assert.equal(h.doc.head.children[0].textContent.includes('tk-hint'), false);
    assert.equal(h.fire.textContent, 'FIRE');
    assert.equal(h.look.textContent, 'LOOK BACK');
  } finally { h.detach(); }
});

test('auto-accelerates, steers, fires, looks back, and never fires on steering taps', () => {
  const h = harness();
  const keys = () => h.doc.inputEvents.map(([type, key]) => `${type}:${key}`);
  try {
    assert.deepEqual(keys(), [], 'countdown must not send ArrowUp');
    h.advance('race', 3100);
    assert.deepEqual(keys(), [], 'GO transition grace');
    h.advance('race', 3300);
    assert.deepEqual(keys(), ['keydown:ArrowUp']);
    h.left.fire('pointerdown', { pointerId: 1, clientX: 80 });
    assert.equal(keys().at(-1), 'keydown:ArrowLeft');

    h.fire.fire('pointerdown', { pointerId: 2 });
    assert.equal(keys().at(-1), 'keydown:e');
    h.fire.fire('click', { detail: 1 });
    assert.equal(keys().filter(key => key === 'keydown:e').length, 1);

    h.look.fire('pointerdown', { pointerId: 3 });
    assert.equal(keys().at(-1), 'keydown:c');
    h.look.fire('pointerup', { pointerId: 3 });
    assert.equal(keys().at(-1), 'keyup:c');

    h.left.fire('pointerup', { pointerId: 1 });
    assert.equal(keys().at(-1), 'keyup:ArrowLeft');
    for (const fn of [...h.timeouts.values()]) fn();
    assert.equal(keys().at(-1), 'keyup:e');

    h.right.fire('pointerdown', { pointerId: 4, clientX: 920 });
    h.right.fire('pointerup', { pointerId: 4, clientX: 920 });
    assert.equal(keys().filter(key => key === 'keydown:e').length, 1,
      'steering tap must never fire');
    assert.equal(h.win.__turboKartTouchState.accelerate, true);
  } finally {
    h.detach();
  }
  assert.equal(keys().at(-1), 'keyup:ArrowUp');
  assert.equal(h.intervals.size, 0);
});

test('look back and accelerate release on pause, fire button is inert outside races', () => {
  const h = harness();
  const keys = () => h.doc.inputEvents.map(([type, key]) => `${type}:${key}`);
  try {
    h.advance('race', 3100);
    h.advance('race', 3300);
    h.look.fire('pointerdown', { pointerId: 3 });
    assert.equal(keys().at(-1), 'keydown:c');
    for (const fn of h.win.events.get('blur') ?? []) fn();
    assert.equal(keys().includes('keyup:c'), true);
    assert.equal(keys().includes('keyup:ArrowUp'), true);
    const count = keys().length;
    h.fire.fire('pointerdown');
    assert.equal(keys().length, count);
  } finally { h.detach(); }
});
