// Validate captured real motion, admitted source identity and original cameras.
// A fresh race need not contain the separate 100-frame synthetic diagnostic.
import assert from 'node:assert/strict';
import fs from 'node:fs';
export function validateMotionEvidence(result) {
  const checks=[];
  const check=(name,condition)=>{assert.ok(condition,name);checks.push(name);};
  const distance=(a,b)=>Math.hypot(...a.map((v,i)=>v-b[i]));
  const session=result.desktopSession;
  check('desktop session authenticated',session.state==='connected'&&session.peers.length>0&&session.peers.every(p=>p.connected)&&session.setup.failures.length===0);
  for(const side of ['android','desktop']){
    const before=result.initial[side],after=result.moved[side],rendered=result.rendered[side];
    check(`${side}: original local physics moved`,distance(before.local,after.local)>1);
    check(`${side}: received remote pose moved`,distance(before.remote.pos,after.remote.pos)>1);
    check(`${side}: original remote mesh in scene`,rendered.remote.visible&&rendered.remote.inScene&&rendered.remote.meshes>0);
    check(`${side}: opponent inside original camera`,rendered.remote.projected.every(n=>Number.isFinite(n)&&Math.abs(n)<=1));
    check(`${side}: game error registry empty`,after.errors.length===0&&rendered.errors.length===0);
  }
  check('Android opponent is admitted desktop agent',result.rendered.android.remote.agent===session.local);
  check('desktop opponent is admitted Android agent',session.peers.some(p=>p.agent===result.rendered.desktop.remote.agent));
  const a=result.initial.android.bridge,b=result.moved.android.bridge;
  check('Android actual physics publication advanced',b.sent>a.sent);
  check('Android decoded and interpolated desktop motion',b.received>a.received&&b.rendered>a.rendered&&b.error===null);
  const d=result.initial.desktop.motion,e=result.moved.desktop.motion;
  check('desktop decoded and interpolated Android motion',e.received>d.received&&e.rendered>d.rendered&&e.error===null);
  check('desktop Rust received the renderer-attributed frames',session.kart.received>0&&session.kart.received>=result.rendered.desktop.motion.received);
  check('desktop Rust published original WP3 frames',session.kart.published>0);
  check('desktop Rust reports no malformed or stale WP3',session.kart.malformed===0&&session.kart.stale===0);
  return {passed:true,checks:checks.length,results:checks,developmentOnly:true,sharedRaceTimingAndScoring:false};
}
if(process.argv[1]&&import.meta.url===new URL(process.argv[1],'file:').href){
  const input=process.argv[2];
  const result=validateMotionEvidence(JSON.parse(fs.readFileSync(input,'utf8')));
  result.recording=input;
  result.note='Revalidation of captured live data after correcting the unrelated 100-frame diagnostic threshold; original harness result retained.';
  if(process.argv[3])fs.writeFileSync(process.argv[3],JSON.stringify(result,null,2)+'\n');
  console.log(`PASS ${result.checks} recorded real-motion, admission and camera assertions`);
}
