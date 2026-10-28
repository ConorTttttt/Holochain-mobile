// Install the unchanged desktop hApps into a fresh, headless test conductor.
import fs from 'node:fs';
import path from 'node:path';
const base=process.env.TURBO_KART_DESKTOP_BASE || '/mnt/holochain/offloaded/sf/Turbokart/turbo-kart/apps/turbo-kart-desktop';
const {AdminWebsocket,encodeHashToBase64}=await import(path.join(base,'node_modules/@holochain/client/lib/index.js'));
const admin=await AdminWebsocket.connect({url:new URL('ws://127.0.0.1:40094'),wsClientOptions:{origin:'kangaroo'}});
const previous=await admin.listApps({});
const ids=['gaming-platform-authority','turbo-kart-authority'];
const existing=previous.filter(a=>ids.includes(a.installed_app_id));
if(new Set(existing.map(a=>encodeHashToBase64(a.agent_pub_key))).size>1)throw new Error('Authority apps have inconsistent identities');
const agent=existing[0]?.agent_pub_key || await admin.generateAgentPubKey();
for(const [id,bundle] of [[ids[0],'gaming_platform.happ'],[ids[1],'turbo_kart.happ']]){
  if(!previous.some(a=>a.installed_app_id===id))await admin.installApp({agent_key:agent,installed_app_id:id,source:{type:'path',value:path.join(base,'.happs',bundle)}});
  await admin.enableApp({installed_app_id:id});
}
const expected=JSON.parse(fs.readFileSync(path.join(base,'expected-dna-hashes.json')));
const apps=(await admin.listApps({})).filter(a=>ids.includes(a.installed_app_id));
const hashes={};
for(const app of apps){
  if(app.status.type!=='enabled')throw new Error('Authority app not enabled');
  for(const [role,cells] of Object.entries(app.cell_info)){
    const cell=cells.find(c=>c.type==='provisioned')?.value;
    const hash=cell&&encodeHashToBase64(cell.cell_id[0]);
    if(hash!==expected[role])throw new Error(`Authority DNA mismatch for ${role}`);
    hashes[role]=hash;
  }
}
const result={agent:encodeHashToBase64(agent),apps:apps.map(a=>a.installed_app_id),dna_hashes:hashes,admin_port:40094};
fs.writeFileSync('docs/evidence/combined/headless-authority.json',JSON.stringify(result,null,2)+'\n');
console.log(JSON.stringify(result,null,2));process.exit(0);
