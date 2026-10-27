import { test } from 'node:test';
import { strict as assert } from 'node:assert';
import { RaceStartGate, TouchDriveState, CONTROL_KEYS } from './mobile-touch.mjs';

test('countdown and intro never trigger auto acceleration', () => {
  const gate = new RaceStartGate();
  const keys = [];
  const state = new TouchDriveState((key, down) => keys.push([key, down]));
  for (const [phase, now] of [
    ['intro', 100], ['intro', 3200], ['countdown', 4100],
    ['countdown', 6100], ['countdown', 8000],
  ]) {
    state.setActive(true, gate.update(phase, now));
    assert.equal(state.snapshot().accelerate, false, `should not accelerate in ${phase}`);
  }
  assert.deepEqual(keys, [], 'no ArrowUp keydown before GO');
});

test('explicit countdown to race transition enables throttle shortly after GO', () => {
  const gate = new RaceStartGate({ countdownTransitionMs: 200 });
  const keys = [];
  const state = new TouchDriveState((key, down) => keys.push([key, down]));
  state.setActive(true, gate.update('countdown', 100));
  state.setActive(true, gate.update('countdown', 3100));
  state.setActive(true, gate.update('race', 3200));
  assert.equal(state.snapshot().accelerate, false);
  state.setActive(true, gate.update('race', 3399));
  assert.equal(state.snapshot().accelerate, false);
  state.setActive(true, gate.update('race', 3400));
  assert.equal(state.snapshot().accelerate, true);
  assert.deepEqual(keys, [[CONTROL_KEYS.accelerate, true]]);
  state.setActive(true, gate.update('racing', 9000));
  assert.equal(keys.length, 1, 'continuous throttle, no repeat keydown');
});

test('race with an implicit countdown waits out three seconds before accelerating', () => {
  const gate = new RaceStartGate({ fallbackStartMs: 3600 });
  const state = new TouchDriveState();
  state.setActive(true, gate.update('race', 1000));
  assert.equal(state.snapshot().accelerate, false);
  state.setActive(true, gate.update('race', 4500));
  assert.equal(state.snapshot().accelerate, false);
  state.setActive(true, gate.update('race', 4600));
  assert.equal(state.snapshot().accelerate, true);
});

test('a new race starts with throttle off after the previous race finishes', () => {
  const gate = new RaceStartGate();
  gate.update('countdown', 0);
  assert.equal(gate.update('race', 100), false);
  assert.equal(gate.update('race', 300), true);
  assert.equal(gate.update('results', 400), false);
  assert.equal(gate.update('race', 500), false, 'must not inherit GO from last race');
  assert.equal(gate.update('race', 4100), true);
});

test('backgrounded countdown cannot unlock while paused', () => {
  const gate = new RaceStartGate({ fallbackStartMs: 3600 });
  assert.equal(gate.update('race', 0, true), false);
  assert.equal(gate.update('race', 50000, false), false);
  assert.equal(gate.update('race', 51000, true), false);
  assert.equal(gate.update('race', 54600, true), true);
});

test('pause while already racing releases acceleration; focus resumes without false start', () => {
  const gate = new RaceStartGate();
  const keys = [];
  const state = new TouchDriveState((key, down) => keys.push([key, down]));
  gate.update('countdown', 0);
  state.setActive(true, gate.update('race', 100));
  state.setActive(true, gate.update('race', 300));
  assert.equal(state.snapshot().accelerate, true);
  state.setActive(false, gate.update('race', 400, false));
  assert.equal(state.snapshot().accelerate, false);
  state.setActive(true, gate.update('race', 600, true));
  assert.equal(state.snapshot().accelerate, true);
  assert.deepEqual(
    keys,
    [[CONTROL_KEYS.accelerate, true], [CONTROL_KEYS.accelerate, false], [CONTROL_KEYS.accelerate, true]],
  );
});

test('steering, fire, and look back remain usable before automatic GO', () => {
  const gate = new RaceStartGate();
  const keys = [];
  let fired = 0;
  const state = new TouchDriveState((key, down) => keys.push([key, down]), () => fired++);
  state.setActive(true, gate.update('countdown', 100));
  state.press(1, 'left');
  state.pressLook(2);
  state.fireWeapon();
  assert.equal(state.snapshot().accelerate, false);
  assert.equal(state.snapshot().left, true);
  assert.equal(state.snapshot().lookBack, true);
  assert.equal(fired, 1);
  assert.equal(keys.some(([code]) => code === CONTROL_KEYS.accelerate), false);
});
