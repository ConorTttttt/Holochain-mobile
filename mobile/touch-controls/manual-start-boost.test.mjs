import { test } from 'node:test';
import { strict as assert } from 'node:assert';
import { TouchDriveState, RaceStartGate, CONTROL_KEYS } from './mobile-touch.mjs';

const empty = {
  left: false, right: false, accelerate: false, lookBack: false,
};

test('no finger: countdown never automatically accelerates', () => {
  const gate = new RaceStartGate();
  const events = [];
  const input = new TouchDriveState((key, down) => events.push([key, down]));
  input.setActive(true, gate.update('countdown', 500), true);
  assert.deepEqual(input.snapshot(), empty);
  assert.deepEqual(events, []);
});

test('hold any finger during countdown: ArrowUp down; lift: ArrowUp up', () => {
  const events = [];
  const input = new TouchDriveState((key, down) => events.push([key, down]));
  input.setActive(true, false, true);
  input.pressBoost(10);
  assert.equal(input.snapshot().accelerate, true);
  assert.deepEqual(events, [[CONTROL_KEYS.accelerate, true]]);
  input.releaseBoost(10);
  assert.equal(input.snapshot().accelerate, false);
  assert.deepEqual(events, [
    [CONTROL_KEYS.accelerate, true],
    [CONTROL_KEYS.accelerate, false],
  ]);
});

test('short tap produces a short boost pulse; starting too early is player-controlled', () => {
  const events = [];
  const input = new TouchDriveState((key, down) => events.push([key, down]));
  input.setActive(true, false, true);
  input.pressBoost(1);
  input.releaseBoost(1);
  input.pressBoost(2);
  input.releaseBoost(2);
  assert.deepEqual(events, [
    ['ArrowUp', true], ['ArrowUp', false],
    ['ArrowUp', true], ['ArrowUp', false],
  ]);
});

test('manual boost holds with multiple fingers until the last one is released', () => {
  const events = [];
  const input = new TouchDriveState((key, down) => events.push([key, down]));
  input.setActive(true, false, true);
  input.pressBoost(11);
  input.pressBoost(12);
  input.pressBoost(12); // duplicate down event must not double-count
  input.releaseBoost(11);
  assert.equal(input.snapshot().accelerate, true);
  input.releaseBoost(12);
  assert.deepEqual(events, [['ArrowUp', true], ['ArrowUp', false]]);
});

test('holding countdown boost through GO keeps ArrowUp held without interruption', () => {
  const gate = new RaceStartGate({ countdownTransitionMs: 200 });
  const events = [];
  const input = new TouchDriveState((key, down) => events.push([key, down]));
  input.setActive(true, gate.update('countdown', 100), true);
  input.pressBoost(1);
  input.setActive(true, gate.update('race', 3100), true);
  input.setActive(true, gate.update('race', 3300), false);
  input.releaseBoost(1);
  assert.deepEqual(events, [['ArrowUp', true]], 'auto takes over while finger stays down');
  assert.equal(input.snapshot().accelerate, true);
});

test('releasing before GO stops throttle until normal auto start', () => {
  const gate = new RaceStartGate();
  const events = [];
  const input = new TouchDriveState((key, down) => events.push([key, down]));
  input.setActive(true, gate.update('countdown', 0), true);
  input.pressBoost(1);
  input.releaseBoost(1);
  input.setActive(true, gate.update('race', 3000), true);
  input.setActive(true, gate.update('race', 3200), false);
  assert.deepEqual(events, [
    ['ArrowUp', true], ['ArrowUp', false], ['ArrowUp', true],
  ]);
});

test('game versions without countdown allow manual boost during fallback window', () => {
  const gate = new RaceStartGate({ fallbackStartMs: 3600 });
  const input = new TouchDriveState();
  input.setActive(true, gate.update('race', 1000), true);
  input.pressBoost(4);
  assert.equal(input.snapshot().accelerate, true);
  input.releaseBoost(4);
  assert.equal(input.snapshot().accelerate, false);
  input.setActive(true, gate.update('race', 4600), false);
  assert.equal(input.snapshot().accelerate, true);
});

test('no manual acceleration during intro or menus', () => {
  const input = new TouchDriveState();
  input.setActive(true, false, false);
  input.pressBoost(1);
  assert.equal(input.snapshot().accelerate, false);
  input.setActive(false, false, false);
  input.pressBoost(2);
  assert.equal(input.snapshot().accelerate, false);
});

test('automatic driving ignores manual boost touches after the countdown', () => {
  const events = [];
  const input = new TouchDriveState((key, down) => events.push([key, down]));
  input.setActive(true, true, false);
  input.pressBoost(1);
  input.releaseBoost(1);
  assert.deepEqual(events, [['ArrowUp', true]]);
});

test('backgrounding cancels manual boost and clears held pointers', () => {
  const events = [];
  const input = new TouchDriveState((key, down) => events.push([key, down]));
  input.setActive(true, false, true);
  input.pressBoost(1);
  input.setActive(false, false, false);
  assert.equal(input.boostPointers.size, 0);
  assert.equal(input.snapshot().accelerate, false);
  assert.deepEqual(events, [['ArrowUp', true], ['ArrowUp', false]]);
});

test('steering, FIRE and LOOK BACK remain independent while manually boosting', () => {
  let shots = 0;
  const input = new TouchDriveState(() => {}, () => shots++);
  input.setActive(true, false, true);
  input.pressBoost(10);
  input.press(10, 'left');
  input.pressLook(11);
  input.fireWeapon();
  assert.deepEqual(input.snapshot(), {
    left: true, right: false, accelerate: true, lookBack: true,
  });
  assert.equal(shots, 1);
  input.releaseBoost(10);
  assert.deepEqual(input.snapshot(), {
    left: true, right: false, accelerate: false, lookBack: true,
  });
});

test('cancelled touch never leaves a boost key stuck', () => {
  const events = [];
  const input = new TouchDriveState((key, down) => events.push([key, down]));
  input.setActive(true, false, true);
  input.pressBoost(1);
  input.releaseBoost(1);
  input.releaseBoost(1); // pointercancel after pointerup is harmless
  assert.deepEqual(events, [['ArrowUp', true], ['ArrowUp', false]]);
});
