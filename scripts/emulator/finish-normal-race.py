#!/usr/bin/env python3
"""Leave a completed race through both original result menus; retain processes."""
import argparse
import gzip
import json
from pathlib import Path
import subprocess
import time
from ui import Emulator, PACKAGE

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--serial', required=True)
parser.add_argument('--race', type=int, required=True)
args = parser.parse_args()
assert args.race > 0
project = Path(__file__).resolve().parents[2]
output = project / 'docs/evidence/synchronized'
label = f'race-{args.race}'
node = Path.home() / '.nvm/versions/node/v24.21.0/bin/node'
em = Emulator(args.serial)
marker = f'BEGIN_{label.upper()}_MENU_RETURN'
em.run('shell', 'log', '-t', 'TurboKartLifecycle', marker)
code = """(()=>{const b=[...document.querySelectorAll('button')].find(b=>b.textContent==='MAIN MENU');
if(!b)throw new Error('Original results MAIN MENU absent');b.click();return true;})()"""
for tool, extra in [('cdp.mjs', ['http://127.0.0.1:9224']), ('desktop.mjs', [])]:
    subprocess.run([str(node), str(project / 'scripts/emulator' / tool), *extra],
                   input=code, text=True, capture_output=True, check=True, timeout=35)
time.sleep(2)
pid = em.run('shell', 'pidof', PACKAGE).strip()
assert pid == (output / f'{label}-app-pid.txt').read_text().strip()
em.click('Multiplayer / Diagnostics')
em.click('Synchronized race status')
status = em.response()
assert status['ok'] and status['result']['realtime_active'] is False
logs = em.run('logcat', '-d', '-v', 'threadtime')
with gzip.open(output / f'{label}-menu-logcat.txt.gz', 'wt') as captured:
    captured.write(logs)
recent = logs.split(marker)[-1]
errors = [line for line in recent.splitlines() if any(text in line for text in
          ('aw_browser_terminator', 'Active renderer gone', 'FATAL EXCEPTION'))]
assert not errors, errors
result = {'passed': True, 'app_pid': pid, 'original_menu_buttons': True,
          'session': status, 'renderer_errors_after_marker': errors, 'same_process': True}
(output / f'{label}-menu-validation.json').write_text(json.dumps(result, indent=2) + '\n')
(output / f'{label}-native-menu.png').write_bytes(subprocess.check_output(
    [em.adb, '-s', args.serial, 'exec-out', 'screencap', '-p'], timeout=30))
print(json.dumps(result))
