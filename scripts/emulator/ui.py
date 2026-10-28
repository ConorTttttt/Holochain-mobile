#!/usr/bin/env python3
"""Explicitly emulator-targeted ADB UI/evidence helper; never clears app data."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import time
import xml.etree.ElementTree as ET

PACKAGE = 'dev.holochain.turbokart.mobile'

class Emulator:
    def __init__(self, serial):
        self.serial = serial
        self.adb = str(Path(os.environ.get('ANDROID_HOME', Path.home() / 'Android/Sdk')) / 'platform-tools/adb')
        if not serial.startswith('emulator-'):
            raise ValueError('This helper accepts emulator serials only')
        if self.run('shell', 'getprop', 'ro.kernel.qemu').strip() != '1':
            raise ValueError('Target did not identify as an emulator')
        if self.run('emu', 'avd', 'name').splitlines()[0].strip() != 'TurboKart_API35':
            raise ValueError('Target is not the dedicated TurboKart_API35 AVD')
        self.width, self.height = map(int, re.findall(r'(\d+)x(\d+)', self.run('shell','wm','size'))[-1])
    def run(self, *args, binary=False):
        result = subprocess.check_output([self.adb, '-s', self.serial, *args], timeout=45)
        return result if binary else result.decode()
    def tree(self):
        self.run('shell', 'uiautomator', 'dump', '/sdcard/turbokart-window.xml')
        return ET.fromstring(self.run('exec-out', 'cat', '/sdcard/turbokart-window.xml'))
    def tap_node(self, node):
        x1,y1,x2,y2 = map(int, re.findall(r'\d+', node.attrib['bounds']))
        self.run('shell', 'input', 'tap', str((x1+x2)//2), str((y1+y2)//2))
    def top(self):
        for _ in range(6): self.swipe(.27,.77,120)
    def bottom(self):
        for _ in range(7): self.swipe(.77,.27,120)
    def swipe(self, start, end, duration=250):
        self.run('shell','input','swipe',str(self.width//2),str(int(self.height*start)),str(self.width//2),str(int(self.height*end)),str(duration))
    def click(self, text):
        self.top()
        for _ in range(12):
            tree = self.tree()
            for node in tree.iter('node'):
                if node.attrib.get('text', '').casefold() == text.casefold() and node.attrib.get('enabled') == 'true':
                    x1,y1,x2,y2=map(int,re.findall(r'\d+',node.attrib['bounds']))
                    middle=(y1+y2)//2
                    # A partially clipped button can lie underneath Android's
                    # transparent status/navigation bars. Bring it into the
                    # middle first; tapping the bar would leave stale JSON.
                    if x2<=x1 or y2<=y1: continue
                    if middle < self.height*.1:
                        self.run('shell','input','swipe',str(self.width//2),str(int(self.height*.4)),str(self.width//2),str(int(self.height*.6)),'250')
                        break
                    if middle > self.height*.85:
                        self.run('shell','input','swipe',str(self.width//2),str(int(self.height*.6)),str(self.width//2),str(int(self.height*.4)),'250')
                        break
                    self.tap_node(node); return
            else:
                self.swipe(.77,.27)
                continue
            continue
        raise ValueError(f'Button missing or disabled: {text}')
    def response(self, timeout=90):
        self.bottom()
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            for value in self.texts():
                if value.startswith('{'):
                    reply = json.loads(value)
                    if not reply.get('ok'): raise RuntimeError(json.dumps(reply))
                    return reply
            time.sleep(1)
        raise TimeoutError('Native UI did not return JSON')
    def set_public_linker_key(self, key):
        self.set_public_field(1, key)
    def set_public_relay_url(self, url):
        if not url.startswith('https://'): raise ValueError('HTTPS relay required')
        self.set_public_field(2, url)
    def set_public_field(self, index, value):
        self.top()
        edits = [n for n in self.tree().iter('node') if n.attrib.get('class') == 'android.widget.EditText']
        if len(edits) <= index: raise ValueError('Open Multiplayer / Diagnostics first')
        self.tap_node(edits[index])
        self.run('shell', 'input', 'keyevent', 'KEYCODE_MOVE_END')
        self.run('shell', 'input', 'keyevent', *(['KEYCODE_DEL'] * 128))
        self.run('shell', 'input', 'text', value)
        self.run('shell', 'input', 'keyevent', 'KEYCODE_BACK')
    def screenshot(self, path):
        Path(path).write_bytes(self.run('exec-out', 'screencap', '-p', binary=True))
    def texts(self):
        return [n.attrib.get('text') for n in self.tree().iter('node') if n.attrib.get('text')]

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('action', choices=['launch','click','texts','screenshot'])
    parser.add_argument('value', nargs='?')
    args = parser.parse_args()
    emulator = Emulator(args.serial)
    if args.action == 'launch': print(emulator.run('shell','am','start','-n',f'{PACKAGE}/.MainActivity'))
    if args.action == 'click': emulator.click(args.value)
    if args.action == 'texts': print(json.dumps(emulator.texts(), indent=2))
    if args.action == 'screenshot': emulator.screenshot(args.value)
