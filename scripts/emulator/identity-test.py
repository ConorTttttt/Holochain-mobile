#!/usr/bin/env python3
"""Verify public identities through the UI across activity/force-stop/reboot.
Never clears data or exports private identity files. Explicit emulator only.
"""
import argparse
import json
from pathlib import Path
import time
from ui import Emulator, PACKAGE

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--serial', required=True)
parser.add_argument('--output', type=Path, default=Path('docs/evidence/combined'))
args = parser.parse_args()
em = Emulator(args.serial)
baseline = {name: json.loads((args.output / f'{name}-initial.json').read_text())['result'][field]
            for name, field in [('holochain','agent'),('iroh','endpoint_id')]}

def snapshot(label):
    em.click('Multiplayer / Diagnostics')
    time.sleep(1)
    results = {}
    for name, button, field in [('holochain','Start / Resume Holochain','agent'),('iroh','Start / Resume Iroh Endpoint (Test)','endpoint_id')]:
        em.click(button)
        reply = em.response()
        assert reply['result'][field] == baseline[name], f'{name} changed after {label}'
        results[name] = reply
        print(f'PASS {label}: {name} {baseline[name]}', flush=True)
    (args.output / f'identity-{label}.json').write_text(json.dumps(results, indent=2)+'\n')

em.run('shell','am','start','--activity-clear-top','-n',f'{PACKAGE}/.MainActivity')
snapshot('activity-restart')
em.run('shell','am','force-stop',PACKAGE)
em.run('shell','am','start','-n',f'{PACKAGE}/.MainActivity')
snapshot('force-stop')
old_boot=em.run('shell','cat','/proc/sys/kernel/random/boot_id').strip()
em.run('reboot')
deadline = time.monotonic()+120
while time.monotonic()<deadline:
    try:
        if em.run('shell','cat','/proc/sys/kernel/random/boot_id').strip()!=old_boot and em.run('shell','getprop','sys.boot_completed').strip()=='1': break
    except Exception: pass
    time.sleep(2)
else: raise TimeoutError('Emulator did not reboot')
em.run('reverse','tcp:8090','tcp:8090')
em.run('shell','input','keyevent','KEYCODE_WAKEUP')
em.run('shell','wm','dismiss-keyguard')
em.run('shell','am','start','-W','-n',f'{PACKAGE}/.MainActivity')
time.sleep(2)
snapshot('emulator-reboot')
