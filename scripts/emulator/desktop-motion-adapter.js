// Development motion test, evaluated in the unchanged original desktop page.
// No shared start, scoring, progress or item authority is provided by this test.
(async () => {
  const status = await kartNet.sessionStatus();
  if (status.state !== 'connected' || !status.peers.length || status.peers.some(p => !p.connected)) throw new Error('Authenticated desktop session required');
  if (!window.__emulatorPort || !window.__emulatorRace) throw new Error('Open the original binary channel and lock admission first');
  const previousMotion = window.__desktopMotion;
  const tickBase = previousMotion?.stats.sentSamples.at(-1)?.tick || 0;
  // Reuse the already admitted binary channel when resetting this test's
  // original race scene; do not close the session or replay its earlier ticks.
  if(previousMotion) { previousMotion.adapter.port=null; previousMotion.stop(); }
  const { NetRace, kartSnapshot } = await import('./src/netplay.js');
  const { CHARACTERS } = await import('./src/config.js');
  const config = window.__emulatorRace.setup.config;
  const local = status.members.find(m => m.agent === status.local);
  const characterIndex = CHARACTERS.findIndex(c => c.id === local.character_id);
  if (characterIndex < 0) throw new Error('Unknown admitted character');
  __game.menu.multiplayer.close();
  const previous = __game.world;
  __game.startRace({ characterIndex, mapId:config.track_id, laps:config.laps, difficulty:({'50cc':'easy','100cc':'normal','150cc':'hard'})[config.class], cupId:null });
  const deadline = performance.now() + 15000;
  while (__game.world === previous || __game.state !== 'intro') {
    if (performance.now() > deadline) throw new Error('Original race did not load');
    await new Promise(r => setTimeout(r,50));
  }
  const world = __game.world;
  const adapter = new NetRace({ port:window.__emulatorPort, status });
  const karts = new Map();
  for (const member of adapter.members.filter(m => !m.local)) {
    const kart = world.karts.find(k => k !== world.player && k.character.id === member.character_id);
    if (!kart) throw new Error('Admitted remote kart absent');
    kart.remote = true; kart.netAgent = member.agent;
    world.ais = world.ais.filter(ai => ai.kart !== kart);
    karts.set(member.index,kart);
  }
  const label = document.createElement('div');
  label.textContent = 'DEVELOPMENT MOTION TEST · shared timing and scoring unavailable';
  Object.assign(label.style,{position:'fixed',top:0,left:0,zIndex:10000,color:'white',background:'#000c',font:'12px sans-serif',pointerEvents:'none'});
  document.body.append(label);
  let active = true, last = performance.now(), lastSend = -Infinity;
  const start = last;
  const stats = { received:0, rendered:0, receivedSamples:[], sentSamples:[], error:null };
  const bounded = (array,value) => { array.push(value); if(array.length>60)array.shift(); };
  adapter.onKart = (index,state) => { stats.received++; bounded(stats.receivedSamples,{index,tick:state.tick,pos:state.pos}); };
  function frame(now) {
    if(!active)return;
    const dt=Math.min(.05,(now-last)/1000); last=now;
    try {
      if(__game.world===world && ['intro','countdown','racing'].includes(__game.state)) {
        for(const pose of adapter.poses(now,dt)) {
          const kart=karts.get(pose.index);
          if(kart){kart.applyRemotePose(pose,dt);stats.rendered++;}
        }
        if(now-lastSend>=50) {
          const sample=kartSnapshot(world.player,tickBase+Math.max(1,Math.floor((now-start)*60/1000)));
          adapter.publish(sample);lastSend=now;bounded(stats.sentSamples,{tick:sample.tick,pos:sample.pos});
        }
      }
    } catch(error) { stats.error=String(error);active=false; }
    requestAnimationFrame(frame);
  }
  window.__desktopMotion={adapter,stats,karts,world,stop(){active=false;adapter.stop();label.remove();}};
  requestAnimationFrame(frame);
  return {race_id:status.race_id,remoteKarts:karts.size,developmentOnly:true};
})()
