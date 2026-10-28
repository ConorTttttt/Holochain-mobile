import test from 'node:test';
import assert from 'node:assert/strict';
import { RaceRpc, admittedControlApi } from './online-game.mjs';

test('race RPC correlates replies and exposes only the fixed race envelope',async()=>{
  const sent=[];const port={postMessage:s=>sent.push(JSON.parse(s)),close(){}};
  const rpc=new RaceRpc(port);const pending=rpc.request('status');
  assert.deepEqual(sent,[{id:1,op:'race',request:{op:'status'}}]);
  port.onmessage({data:JSON.stringify({id:99,ok:true,result:'wrong'})});
  assert.equal(rpc.pending.size,1);
  port.onmessage({data:JSON.stringify({id:1,ok:true,result:{state:'connected'}})});
  assert.deepEqual(await pending,{state:'connected'});assert.equal(rpc.pending.size,0);rpc.close();
});

test('normal multiplayer API preserves locked race envelope and pins every lobby request',async()=>{
  const operations=[];const record={setup:{lobby:'locked-lobby'}};
  const api=admittedControlApi({request:async op=>{operations.push(op);return {state:'connected'};}},record,'android-agent');
  assert.deepEqual(await api.raceGet('locked-lobby'),{race:record});
  await assert.rejects(api.raceGet('another-lobby'),/Different lobby/);
  await assert.rejects(api.sessionStart('another-lobby'),/Different lobby/);
  assert.equal((await api.netInit()).control_plane_ready,true);
  assert.equal(typeof api.onLobbyEvent(()=>{}),'function');
  await api.sessionStart('locked-lobby');await api.sessionReady();
  assert.deepEqual(operations,['status','ready']);
});

test('race RPC bounds requests and rejects every held promise on channel close',async()=>{
  let closed=false;
  const rpc=new RaceRpc({postMessage(){},close(){closed=true;}});
  const pending=Array.from({length:24},()=>rpc.request('poll').catch(e=>e.message));
  await assert.rejects(rpc.request('poll'),/backpressure/);
  assert.equal(rpc.pending.size,24);rpc.close();
  assert.ok((await Promise.all(pending)).every(e=>e==='Race bridge closed'));
  assert.equal(rpc.pending.size,0);assert.equal(closed,true);
});
