// Capability-only bridge. Traffic is bounded to one outstanding 20 Hz request.
export class GameBridge {
  constructor({ port, game, snapshot, Buffer, characters, now = () => performance.now() }) {
    Object.assign(this, { port, game, snapshot, Buffer, characters, now });
    this.pending = false;
    this.sequence = 0;
    this.status = null;
    this.world = null;
    this.remotes = new Map();
    this.stats = { sent: 0, received: 0, rendered: 0, error: null };
    this.lastSend = -Infinity;
    this.tickBase = 0;
    this.startedAt = now();
    port.onmessage = event => this.receive(event.data);
    this.request({ op: 'status' });
  }
  request(value) {
    if (this.pending) return false;
    this.pending = ++this.sequence;
    this.port.postMessage(JSON.stringify({ ...value, id: this.pending }));
    return true;
  }
  receive(raw) {
    let response;
    try { response = JSON.parse(raw); } catch { this.stats.error = 'Invalid native response'; this.pending = false; return; }
    if (response.id !== this.pending) return;
    this.pending = false;
    if (response.errorCode === 'paused') return;
    if (!response.ok) { this.stats.error = response.error; return; }
    const value = response.result;
    if (!this.status) {
      if (!value.authenticated_moq_peer_session) { this.stats.error = 'Peers have not authenticated'; return; }
      this.status = value;
      // A preceding diagnostic uses this same track. Never replay its ticks
      // when switching to actual physics or re-entering the game view.
      this.tickBase = Math.max(value.last_game_tick || 0, value.synthetic_kart_packets_sent || 0);
      const local = value.members.find(m => m.agent === value.local_agent);
      const characterIndex = this.characters.findIndex(c => c.id === local?.character_id);
      if (characterIndex < 0) { this.stats.error = 'Unknown admitted character'; this.status = null; return; }
      const difficulty = { '50cc':'easy', '100cc':'normal', '150cc':'hard' }[value.config.class] || 'normal';
      this.game.startRace({ characterIndex, difficulty, mapId: value.config.track_id, laps: value.config.laps, cupId: null });
      return;
    }
    if (value.sent) this.stats.sent++;
    for (const frame of value.remotes || []) {
      const remote = this.remotes.get(frame.agent);
      if (!remote || frame.ageMs > 2000) continue;
      remote.buffer.push(frame.state, this.now() - Math.max(0, frame.ageMs));
      this.stats.received++;
    }
  }
  bindWorld(world) {
    this.world = world;
    this.remotes.clear();
    for (const member of this.status.members) {
      if (member.agent === this.status.local_agent) continue;
      const kart = world.karts.find(k => k !== world.player && k.character.id === member.character_id);
      if (!kart) { this.stats.error = 'Admitted remote character missing from game'; continue; }
      kart.remote = true;
      kart.netAgent = member.agent;
      world.ais = world.ais.filter(ai => ai.kart !== kart);
      this.remotes.set(member.agent, { kart, buffer: new this.Buffer() });
    }
  }
  update(dt, visible = true) {
    const world = this.game.world;
    if (!visible || !this.status || !world?.player || !['intro', 'countdown', 'racing'].includes(this.game.state)) return;
    if (world !== this.world) this.bindWorld(world);
    const now = this.now();
    for (const { kart, buffer } of this.remotes.values()) {
      const pose = buffer.sample(now);
      if (pose) { kart.applyRemotePose(pose, dt); this.stats.rendered++; }
    }
    if (now - this.lastSend < 50 || this.pending || this.stats.error) return;
    // WP3 ticks remain the original 60 Hz simulation clock; publication is 20 Hz.
    const tick = this.tickBase + Math.max(1, Math.floor((now - this.startedAt) * 60 / 1000));
    if (tick > 0xffffffff) { this.stats.error = 'WP3 tick exhausted; reconnect race'; return; }
    if (this.request({ op: 'frame', state: this.snapshot(world.player, tick) })) this.lastSend = now;
  }
  close() { this.port.onmessage = null; this.port.close(); this.status = null; this.remotes.clear(); }
}
