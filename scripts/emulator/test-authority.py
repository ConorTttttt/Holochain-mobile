#!/usr/bin/env python3
"""A normal headless desktop conductor for full-arc test-network convergence.
Creates its own Lair identity; never imports a phone or player key.
"""
import json
import os
from pathlib import Path
import secrets
import signal
import subprocess
import time
import yaml

group=Path('/proc/self/cgroup').read_text()
limit=subprocess.check_output(['systemctl','--user','show','turbo-kart-test-authority.scope','-p','MemoryMax','--value'],text=True).strip()
if '/turbo-kart-test-authority.scope' not in group or limit!='1073741824':
    raise RuntimeError('Run through scripts/emulator/start-authority-memory-safe.sh')

project=Path(__file__).resolve().parents[2]
base=Path(os.environ.get('TURBO_KART_DESKTOP_BASE','/mnt/holochain/offloaded/sf/Turbokart/turbo-kart/apps/turbo-kart-desktop'))
root=Path(f'/tmp/tk-combined-authority-{os.getuid()}')
marker=root/'.turbo-kart-combined-authority'
if root.exists() and not marker.exists(): raise RuntimeError('Refusing to reuse an unrelated authority directory')
root.mkdir(mode=0o700,exist_ok=True)
marker.touch(mode=0o600,exist_ok=True)
password=root/'passphrase'
if not password.exists():
    if (root/'keystore').exists(): raise RuntimeError('Missing authority passphrase; preserving existing identity')
    fd=os.open(password,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
    with os.fdopen(fd,'wb') as f: f.write(secrets.token_hex(32).encode())
secret=password.read_bytes()
lair=base/'resources/bins/lair-keystore-dev.holoch'
conductor=base/'resources/bins/holochain-v0.7.0-dev.holoch'
keys=root/'keystore'
keys.mkdir(mode=0o700,exist_ok=True)
if not (keys/'lair-keystore-config.yaml').exists():
    subprocess.run([str(lair),'-r',str(keys),'init','-p'],input=secret,check=True,capture_output=True)
url=subprocess.check_output([str(lair),'-r',str(keys),'url'],text=True).strip()
config=root/'conductor.yaml'
if not config.exists():
    value=yaml.safe_load((base/'resources/conductor-config.yaml').read_text())
    value['data_root_path']=str(root/'data')
    value['keystore']={'type':'lair_server','connection_url':url}
    value['admin_interfaces']=[{'driver':{'type':'websocket','port':40094,'allowed_origins':'kangaroo'}}]
    value['network']['bootstrap_url']='https://dev-test-bootstrap2.holochain.org/'
    value['network']['relay_url']='https://dev-test-bootstrap2.holochain.org/'
    value['network']['advanced']={'coreBootstrap':{'backoffMaxMs':10000}}
    config.write_text(yaml.safe_dump(value))
children=[]
def cleanup(*_):
    for child in reversed(children):
        if child.poll() is None: child.terminate()
    for child in reversed(children):
        try: child.wait(timeout=10)
        except subprocess.TimeoutExpired: child.kill()
def stop(*_):
    cleanup()
    raise SystemExit(0)
signal.signal(signal.SIGTERM,stop)
signal.signal(signal.SIGINT,stop)
def launch(args,log):
    stream=open(log,'ab')
    child=subprocess.Popen(args,stdin=subprocess.PIPE,stdout=stream,stderr=stream,env={**os.environ,'RUST_LOG':'warn'})
    children.append(child)
    child.stdin.write(secret);child.stdin.close()
    return child
try:
    launch([str(lair),'-r',str(keys),'server','-p'],'/tmp/turbokart-authority-lair.log')
    time.sleep(2)
    launch([str(conductor),'-c',str(config),'-p'],'/tmp/turbokart-authority-conductor.log')
    node=Path.home()/'.nvm/versions/node/v24.21.0/bin/node'
    for attempt in range(60):
        if any(c.poll() is not None for c in children): raise RuntimeError('Authority process exited; inspect its dedicated logs')
        result=subprocess.run([str(node),'--max-old-space-size=128',str(project/'scripts/emulator/install-authority.mjs')],cwd=project,capture_output=True,text=True,timeout=45)
        if result.returncode==0:
            print(result.stdout,flush=True);break
        time.sleep(1)
    else: raise RuntimeError('Authority admin did not become ready')
    while all(c.poll() is None for c in children): time.sleep(2)
finally:
    cleanup()
