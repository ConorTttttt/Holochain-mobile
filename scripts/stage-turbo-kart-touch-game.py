#!/usr/bin/env python3
"""Stage the existing desktop web game as APK assets and inject mobile touch controls.

Does not change the original game source, Holochain DNAs, or desktop app.
"""
from __future__ import annotations

import argparse
import json
import re
from pathlib import Path
import shutil
import sys


def stage(source: Path, destination: Path, touch_source: Path) -> None:
    src = source.resolve(strict=True)
    if not (src / 'index.html').is_file() or not (src / 'src').is_dir():
        raise ValueError(f'Game UI must contain index.html and src/: {src}')
    dest = destination.resolve()
    if dest == src or src in dest.parents or dest in src.parents:
        raise ValueError('Destination must not overlap the original game source')
    if not touch_source.is_file():
        raise ValueError(f'Touch module missing: {touch_source}')

    # Clean only the generated game APK asset tree.
    if dest.exists():
        shutil.rmtree(dest)
    # Only copy runtime assets: never embed .git, private data, node_modules
    # or development logs from the game's original source directory.
    dest.mkdir(parents=True)
    allowed = ("index.html", "src", "assets", "vendor", "icon.png", "LICENSE", "NOTICE", "licenses")
    for name in allowed:
        original = src / name
        target = dest / name
        if not original.exists():
            continue
        if original.is_dir():
            shutil.copytree(original, target, symlinks=False)
        else:
            shutil.copy2(original, target)

    fonts = Path(__file__).resolve().parent.parent / 'mobile' / 'game-assets' / 'fonts'
    manifest = json.loads((fonts / 'sources.json').read_text())
    import hashlib
    for name, info in manifest.items():
        if hashlib.sha256((fonts / name).read_bytes()).hexdigest() != info['sha256']:
            raise ValueError(f'Bundled font checksum mismatch: {name}')
    shutil.copytree(fonts, dest / 'mobile-fonts')
    (dest / 'local-fonts.css').write_text(
        "@font-face{font-family:'Lilita One';font-style:normal;font-weight:400;font-display:swap;src:url('./mobile-fonts/LilitaOne-Regular.ttf') format('truetype')}\n"
        "@font-face{font-family:'Nunito';font-style:normal;font-weight:200 1000;font-display:swap;src:url('./mobile-fonts/Nunito.ttf') format('truetype')}\n", encoding='utf-8')
    page = (dest / 'index.html').read_text(encoding='utf-8')
    page = re.sub(r'\s*<link[^>]+(?:fonts.googleapis.com|fonts.gstatic.com)[^>]*>', '', page)
    page = page.replace('</head>', '<link rel="stylesheet" href="./local-fonts.css">\n</head>')
    # Check static entrypoints and import-map targets before shipping an APK.
    references = re.findall(r'(?:src|href)=["\']([^"\']+)["\']', page)
    for block in re.findall(r'<script\s+type="importmap"[^>]*>(.*?)</script>', page, re.S):
        references += list(json.loads(block).get('imports', {}).values())
    for reference in references:
        if reference.startswith(('https:', 'http:', 'data:', '#', '//')):
            continue
        relative = reference.split('?', 1)[0].split('#', 1)[0]
        if not (dest / relative).exists():
            raise ValueError(f'Required game entrypoint missing: {reference}')
    if page.count('</body>') != 1:
        raise ValueError('Game index.html must contain exactly one closing </body>')
    if 'tk-touch-controls' in page or 'mobile-touch.mjs' in page:
        raise ValueError('Game page already contains injected mobile touch controls')

    if '<meta name="viewport"' not in page and "<head>" in page:
        page = page.replace(
            '<head>',
            '<head><meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">',
            1,
        )

    injected = (
        '\n<!-- Turbo Kart mobile: auto-accelerate; invisible left/right steering; separate Fire (E) and Look Back (C) buttons. -->\n'
        '<script type="module" src="./mobile-touch.mjs"></script>\n'
    )
    page = page.replace('</body>', injected + '</body>')
    (dest / 'index.html').write_text(page, encoding='utf-8')
    shutil.copy2(touch_source, dest / 'mobile-touch.mjs')
    bridge = Path(__file__).resolve().parent.parent / 'mobile' / 'game-bridge'
    for name in ('mobile-game.mjs', 'game-bridge-core.mjs', 'online-game.mjs'):
        shutil.copy2(bridge / name, dest / name)
    page = (dest / 'index.html').read_text(encoding='utf-8')
    page = page.replace('</body>', '<script type="module" src="./mobile-game.mjs"></script>\n</body>')
    (dest / 'index.html').write_text(page, encoding='utf-8')
    print(f'Android touch game staged: {src} -> {dest}')
    print('Original game files and both Holochain DNAs were not modified.')


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--source', type=Path, required=True)
    p.add_argument('--dest', type=Path, required=True)
    p.add_argument('--touch-script', type=Path, default=Path(__file__).resolve().parent.parent / 'mobile' / 'touch-controls' / 'mobile-touch.mjs')
    args = p.parse_args()
    try:
        stage(args.source, args.dest, args.touch_script)
    except (OSError, ValueError) as exc:
        sys.exit(f'[touch game staging] {exc}')


if __name__ == '__main__':
    main()
