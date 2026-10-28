#!/usr/bin/env python3
"""Reject an APK missing game assets, either JNI library, or the original app ID."""
import argparse
from pathlib import Path
import subprocess
import zipfile
import os


def verify(apk, abis, sdk=None):
    sdk = Path(sdk or os.environ.get('ANDROID_HOME', Path.home() / 'Android/Sdk'))
    with zipfile.ZipFile(apk) as archive:
        names = set(archive.namelist())
        required = {'assets/game/index.html', 'assets/game/mobile-touch.mjs',
                    'assets/game/mobile-game.mjs', 'assets/game/game-bridge-core.mjs',
                    'assets/game/local-fonts.css', 'assets/game/mobile-fonts/LilitaOne-Regular.ttf',
                    'assets/game/mobile-fonts/Nunito.ttf',
                    'assets/game/src/main.js', 'assets/game/src/kart.js', 'assets/game/src/input.js',
                    'assets/game/src/race.js', 'assets/game/src/netplay.js',
                    'assets/game/src/maps/index.js', 'assets/game/assets/karts/zippy.glb',
                    'assets/gaming_platform.happ', 'assets/turbo_kart.happ', 'assets/expected-dna-hashes.json'}
        source = Path(__file__).resolve().parents[1] / 'android/turbo-kart-mobile/app/src/main/assets/game'
        required |= {'assets/game/' + p.relative_to(source).as_posix() for p in source.rglob('*') if p.is_file()}
        for abi in abis:
            required |= {f'lib/{abi}/libturbo_kart_{lib}.so' for lib in ('android_native', 'realtime_jni')}
        missing = required - names
        if missing:
            raise ValueError(f'Missing APK runtime files: {sorted(missing)}')
        if not any(n.startswith('assets/game/vendor/') for n in names) or not any(n.startswith('assets/game/src/') for n in names):
            raise ValueError('Original game src/vendor missing')
        for relative in ('index.html', 'mobile-touch.mjs', 'mobile-game.mjs', 'game-bridge-core.mjs', 'src/main.js', 'src/kart.js', 'src/netplay.js'):
            if archive.read('assets/game/' + relative) != (source / relative).read_bytes():
                raise ValueError(f'Stale packaged gameplay code: {relative}')
        # Inspect ELF e_machine inside the packaged files, not the build directory.
        for abi in abis:
            for lib in ('android_native', 'realtime_jni'):
                data = archive.read(f'lib/{abi}/libturbo_kart_{lib}.so')
                expected = {'arm64-v8a': 183, 'x86_64': 62}[abi]
                if data[:4] != b'\x7fELF' or int.from_bytes(data[18:20], 'little') != expected:
                    raise ValueError(f'Wrong packaged ELF architecture: {abi}/{lib}')
    aapt = sdk / 'build-tools/35.0.0/aapt'
    manifest = subprocess.check_output([str(aapt), 'dump', 'badging', str(apk)], text=True)
    if "package: name='dev.holochain.turbokart.mobile'" not in manifest:
        raise ValueError('APK application ID differs from dev.holochain.turbokart.mobile')
    if "launchable-activity: name='dev.holochain.turbokart.mobile.MainActivity'" not in manifest:
        raise ValueError('MainActivity is not the launcher')
    print(f'APK verified: original package, MainActivity, complete staged assets, JNI ABIs {abis}')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('apk', type=Path)
    parser.add_argument('--abis', default='arm64-v8a,x86_64')
    args = parser.parse_args()
    verify(args.apk, args.abis.split(','))
