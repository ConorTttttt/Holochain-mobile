// Android's capability transport for the ORIGINAL desktop Multiplayer.enterRace
// and startNetRace path. All game rules stay in the packaged original modules.
export class RaceRpc {
  constructor(port) {
    this.port=port;this.id=0;this.pending=new Map();
    port.onmessage=event=>{
      let reply;try{reply=JSON.parse(event.data);}catch{return;}
      const held=this.pending.get(reply.id);if(!held)return;
      this.pending.delete(reply.id);clearTimeout(held.timer);
      reply.ok?held.resolve(reply.result):held.reject(new Error(reply.error||reply.errorCode||'Native race request failed'));
    };
  }
  request(op,fields={}) {
    if(this.pending.size>=24)return Promise.reject(new Error('Race bridge backpressure'));
    const id=++this.id;
    return new Promise((resolve,reject)=>{
      const timer=setTimeout(()=>{this.pending.delete(id);reject(new Error(`Native ${op} timed out`));},8000);
      this.pending.set(id,{resolve,reject,timer});
      this.port.postMessage(JSON.stringify({id,op:'race',request:{op,...fields}}));
    });
  }
  close(){for(const p of this.pending.values()){clearTimeout(p.timer);p.reject(new Error('Race bridge closed'));}this.pending.clear();this.port.close();}
}

const encode=bytes=>btoa(String.fromCharCode(...bytes));
const decode=value=>Uint8Array.from(atob(value),c=>c.charCodeAt(0));

export function admittedControlApi(rpc,record,local){
  const pinned=id=>{if(id!==record.setup.lobby)throw new Error('Different lobby refused');};
  return {
    netInit:async()=>({agent:local,game_id:'turbo-kart',control_plane_ready:true,realtime_active:true}),
    // Admission is immutable; no live lobby writes/events are exposed to JS.
    onLobbyEvent:()=>()=>{},
    raceGet:async id=>{pinned(id);return {race:record};},
    sessionStart:async id=>{pinned(id);return rpc.request('status');},
    sessionStatus:()=>rpc.request('status'),
    sessionReady:()=>rpc.request('ready'),
  };
}

export async function enterOnlineGame(nativePort,game) {
  const rpc=new RaceRpc(nativePort);
  const bootstrap=await rpc.request('bootstrap');
  const record=bootstrap.record;
  const transport={closed:false,polling:false,timer:null,channel:null};
  const stats={delivered:0,queued:0,error:null,entered:false,stopped:false};
  window.__turboKartOnline=stats;
  const closeChannel=()=>{transport.closed=true;clearTimeout(transport.timer);transport.channel?.port1.close();transport.channel=null;};
  async function poll(){
    if(transport.closed||transport.polling)return;
    transport.polling=true;
    try{const result=await rpc.request('poll');for(const packet of result.packets){transport.channel?.port1.postMessage(decode(packet));stats.delivered++;}}
    catch(error){if(!transport.closed)stats.error=String(error);}
    finally{transport.polling=false;if(!transport.closed)transport.timer=setTimeout(poll,50);}
  }
  const api={
    ...admittedControlApi(rpc,record,bootstrap.status.local),
    sessionStop:async()=>{closeChannel();const result=await rpc.request('stop');stats.stopped=true;return result;},
    openTestChannel:()=>{
      closeChannel();transport.closed=false;
      const channel=new MessageChannel();transport.channel=channel;
      channel.port1.onmessage=async event=>{
        const data=event.data;
        const bytes=data instanceof ArrayBuffer?new Uint8Array(data):ArrayBuffer.isView(data)?new Uint8Array(data.buffer,data.byteOffset,data.byteLength):null;
        if(!bytes||bytes.length<56||bytes.length>3072){stats.error='Invalid original binary frame';return;}
        // Native has a bounded receive queue; the original IPC credit is local.
        if(bytes[4]===4)return;
        if(bytes[4]!==1){stats.error='Unsupported original binary operation';return;}
        try{const result=await rpc.request('packet',{packet:encode(bytes)});if(!transport.closed)channel.port1.postMessage(decode(result.reply));stats.queued++;}
        catch(error){stats.error=String(error);}
      };
      channel.port1.start();void poll();
      window.postMessage('kart-net:test-channel',location.origin,[channel.port2]);
    },
  };
  // This is the same entry method and callback used by the normal desktop UI.
  // It waits for authenticated peers, loads the original online world and calls
  // sessionReady only after that scene (including AI/progress/items) is ready.
  const multiplayer=game.menu.multiplayer;
  game.menu.hideAll();multiplayer.element.classList.add('active');
  multiplayer.api=api;multiplayer.identity={agent:bootstrap.status.local};
  multiplayer.lobbyId=record.setup.lobby;
  multiplayer.race(record);
  await multiplayer.enterRace();
  if(!game.world?.net&&multiplayer.entryMessage)throw new Error(multiplayer.entryMessage);
  stats.entered=true;
  const exitTimer=setInterval(()=>{
    if(stats.stopped&&game.state==='title'&&!game.world?.net){
      clearInterval(exitTimer);
      nativePort.postMessage(JSON.stringify({id:++rpc.id,op:'exit'}));
    }
  },100);
  return {api,rpc,stats,closeChannel,leave:async()=>{
    if(game.world?.net)await game.leaveOnlineRace(false);else await api.sessionStop();
    nativePort.postMessage(JSON.stringify({id:++rpc.id,op:'exit'}));
  }};
}
