#!/usr/bin/env python3
"""Normal desktop/native lobby UI flow. Never clears data or restarts an app."""
import argparse
import json
from pathlib import Path
import subprocess
import time
from ui import Emulator, PACKAGE

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--serial',required=True)
parser.add_argument('--race',required=True,type=int)
parser.add_argument('--same-process-as',type=Path)
args=parser.parse_args()
assert args.race>0
project=Path(__file__).resolve().parents[2]
output=project/'docs/evidence/synchronized'
output.mkdir(parents=True,exist_ok=True)
label=f'race-{args.race}'
name=f'SyncRace-{args.race}'
node=Path.home()/'.nvm/versions/node/v24.21.0/bin/node'
def desktop(code):
    result=subprocess.run([str(node),str(project/'scripts/emulator/desktop.mjs')],input=code,text=True,capture_output=True,timeout=35,cwd=project)
    if result.returncode:raise RuntimeError(result.stderr)
    return json.loads(result.stdout)
def save(suffix,value):
    (output/f'{label}-{suffix}.json').write_text(json.dumps(value,indent=2)+'\n')

em=Emulator(args.serial)
if args.same_process_as:
    pid=em.run('shell','pidof',PACKAGE).strip()
    assert pid==args.same_process_as.read_text().strip(),'App process changed between races'
em.run('shell','am','start','-W','-n',f'{PACKAGE}/.MainActivity')
em.run('reverse','tcp:8090','tcp:8090')
em.click('Multiplayer / Diagnostics')
setup_actions={}
for suffix,button in [('holochain','Start / Resume Holochain'),('iroh','Start / Resume Iroh Endpoint (Test)')]:
    em.top()
    candidates=[n for n in em.tree().iter('node') if n.attrib.get('text','').casefold()==button.casefold()]
    assert len(candidates)==1,'Native setup control missing'
    if candidates[0].attrib['enabled']=='true':
        setup_actions[suffix]='resume-requested'
        em.click(button);reply=em.response();save(suffix,reply);print(suffix,json.dumps(reply),flush=True)
    else:setup_actions[suffix]='already-running-no-call'
save('setup-actions',setup_actions)
em.click('Synchronized race status')
status=em.response();save('before-status',status)
assert status['result']['realtime_active'] is False,'Previous session did not close'

created=desktop(f"""(async()=>{{
  __game.menu.showMultiplayer();const m=__game.menu.multiplayer,end=Date.now()+20000;
  while((!m.identity||m.loop.busy)&&Date.now()<end)await new Promise(r=>setTimeout(r,100));
  if(m.loop.busy)throw new Error('Original multiplayer UI remained busy');
  m.create();const f=document.querySelector('.mp-form');
  f.querySelector('[name=name]').value={json.dumps(name)};
  f.querySelector('[name=capacity]').value='2';f.querySelector('[name=class]').value='50cc';f.querySelector('[name=laps]').value='1';
  while(m.loop.busy&&Date.now()<end)await new Promise(r=>setTimeout(r,100));
  f.dispatchEvent(new Event('submit',{{bubbles:true,cancelable:true}}));
  while((m.mode!=='room'||m.writing)&&Date.now()<end)await new Promise(r=>setTimeout(r,100));
  if(m.mode!=='room')throw new Error(document.body.innerText);
  return{{lobby:m.lobbyId,text:document.body.innerText}};
}})()""")
save('created',created)
for suffix,button in [('discovery','Discover desktop lobbies'),('selected','View selected lobby'),('joined','Join as Zippy'),('presence','Publish signed Iroh presence')]:
    em.click(button);reply=em.response();save(suffix,reply)
    if suffix=='selected':assert name in json.dumps(reply),'Refusing an unrelated lobby'
    print(suffix,'passed',flush=True)

locked=desktop("""(async()=>{
  window.__normalRaceEvents=[];
  for(const type of ['race:countdown','race:go','race:lap','race:finish','race:end'])__game.bus.on(type,data=>__normalRaceEvents.push({type,wall:Date.now(),page:performance.timeOrigin+performance.now(),n:data?.n,lap:data?.lap,place:data?.place}));
  const m=__game.menu.multiplayer,end=Date.now()+20000;
  while(Date.now()<end){const b=[...document.querySelectorAll('button')].find(b=>b.textContent==='Start Race');if(b&&!b.disabled&&!m.loop.busy&&!m.writing){b.click();break;}await new Promise(r=>setTimeout(r,100));}
  while(m.mode!=='race'&&Date.now()<end)await new Promise(r=>setTimeout(r,100));
  if(m.mode!=='race')throw new Error(document.body.innerText);
  const record=m.raceRecord;[...document.querySelectorAll('button')].find(b=>b.textContent==='Enter race').click();return record;
})()""")
save('locked',locked)
em.click('Enter synchronized race')
time.sleep(1)
pid=em.run('shell','pidof',PACKAGE).strip();assert pid.isdigit()
if args.same_process_as:assert pid==args.same_process_as.read_text().strip()
(output/f'{label}-app-pid.txt').write_text(pid+'\n')
em.run('forward','tcp:9224',f'localabstract:webview_devtools_remote_{pid}')
print('Original normal online entry opened:',label,'app PID',pid,flush=True)
