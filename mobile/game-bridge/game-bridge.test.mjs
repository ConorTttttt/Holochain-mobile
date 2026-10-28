import test from 'node:test';
import assert from 'node:assert/strict';
import { GameBridge } from './game-bridge-core.mjs';

function setup() {
  let time = 0;
  const requests = [];
  const remote = { character: { id: 'desktop' }, applyRemotePose(p) { this.pose = p; } };
  const player = { character: { id: 'zippy' }, x: 10 };
  const game = { state: 'racing', world: { player, karts: [player, remote], ais: [{ kart: remote }] }, startRace(s) { this.settings = s; } };
  class Buffer { push(s) { this.state = s; } sample() { return this.state; } }
  const port = { postMessage(raw) { requests.push(JSON.parse(raw)); }, close() {} };
  const bridge = new GameBridge({ port, game, snapshot: (kart, tick) => ({ pos: [kart.x, 0, 0], tick }), Buffer, characters: [{ id: 'zippy' }], now: () => time });
  const reply = result => bridge.receive(JSON.stringify({ id: requests.at(-1).id, ok: true, result }));
  return { bridge, requests, remote, game, reply, setTime: t => { time = t; } };
}
const status = { authenticated_moq_peer_session: true, local_agent: 'android', members: [{ agent: 'android', character_id: 'zippy' }, { agent: 'host', character_id: 'desktop' }], config: { track_id: 'palm-cove', laps: 3 } };
test('requires authenticated status and uses locked map and character', () => {
  const s = setup(); s.reply({ ...status, authenticated_moq_peer_session: false });
  s.bridge.update(.01); assert.equal(s.requests.length, 1); assert.ok(s.bridge.stats.error);
  const t = setup(); t.reply(status); assert.equal(t.game.settings.mapId, 'palm-cove');
});
test('actual physics at 20 Hz, no overlapping requests or hidden traffic', () => {
  const s = setup(); s.reply(status); s.bridge.update(.01);
  assert.deepEqual(s.requests[1].state.pos, [10, 0, 0]);
  s.setTime(100); s.bridge.update(.01); assert.equal(s.requests.length, 2);
  s.reply({ sent: true }); s.bridge.update(.01); assert.equal(s.requests.length, 3);
  s.reply({ sent: true }); s.setTime(120); s.bridge.update(.01); assert.equal(s.requests.length, 3);
  s.setTime(200); s.bridge.update(.01, false); assert.equal(s.requests.length, 3);
});
test('only admitted remote states reach interpolation and original remote renderer', () => {
  const s = setup(); s.reply(status); s.bridge.update(.01);
  const pose = { tick: 6, pos: [5,0,3] };
  s.reply({ remotes: [{ agent: 'unknown', state: pose, ageMs: 0 }, { agent: 'host', state: pose, ageMs: 0 }] });
  s.bridge.update(.01); assert.equal(s.bridge.stats.received, 1); assert.deepEqual(s.remote.pose, pose);
  assert.equal(s.remote.remote, true); assert.equal(s.game.world.ais.length, 0);
});
test('real physics continues beyond diagnostic ticks and retains the admitted class', () => {
  const s = setup();
  s.reply({ ...status, synthetic_kart_packets_sent: 100, config: { ...status.config, class:'150cc' } });
  s.bridge.update(.01);
  assert.equal(s.requests.at(-1).state.tick, 101);
  assert.equal(s.game.settings.difficulty, 'hard');
  const t = setup();
  t.reply({ ...status, synthetic_kart_packets_sent:100, last_game_tick:250 });
  t.bridge.update(.01);
  assert.equal(t.requests.at(-1).state.tick, 251);
});
