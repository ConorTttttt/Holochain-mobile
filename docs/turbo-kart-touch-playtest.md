# Turbo Kart mobile touchscreen driving — first playable UI experiment

**Branch:** `feat/turbo-kart-touch-controls`

This change adds a **separate landscape Android launcher** called
**Turbo Kart Touch Race**. It loads the **existing Three.js desktop game**
in an isolated Android WebView and overlays three large multi-touch zones:

- **Left side (◀):** hold to steer left.
- **Right side (▶):** hold to steer right.
- **Bottom middle (GO ▲):** hold to accelerate (even while steering).

Releasing a finger releases only that control. If both left and right are
held, the steering input is neutral; GO remains active. Buttons appear only
in racing states (`intro`, `countdown`, `race`, `racing`), leaving the
title screen/character selection clickable. Switching away from the app or
ending a race releases controls. The underlying keyboard bindings are mapped
to **ArrowLeft / ArrowRight / ArrowUp**; if a particular game revision uses
different gameplay bindings, update `CONTROL_KEYS` in
`mobile/touch-controls/mobile-touch.mjs` after inspecting its keyboard
handlers.

## Important distinction

The game renderer is **not** in the GitHub Holochain-mobile repository.
The original web game was kept as a sibling `turbo-kart-rally-main`
directory and packaged into desktop `resources/ui`. This build stages those
**existing** UI files from the installed/unpacked desktop game, rather than
re-implementing the kart physics in Kotlin or replacing game assets with a
mock game. Neither production Holochain DNA is changed.

This is an isolated **single-player driving playtest**, not a proof of
Android multiplayer gameplay. The independent
`Turbo Kart H2HC Test` launcher with Holochain/Iroh diagnostics is preserved.
There is **no privileged Kotlin-to-JavaScript bridge**, so this game WebView
does not receive conductor credentials.

## Test and build

The pure JavaScript touch state tests can run without Android:

```sh
node --test mobile/touch-controls/mobile-touch.test.mjs
```

Find the existing unpacked game UI. This is usually:

```text
/mnt/holochain/offloaded/sf/Turbokart/turbo-kart/apps/turbo-kart-desktop/resources/ui
```

or:

```text
/opt/Turbo Kart Rally/resources/app.asar.unpacked/resources/ui
```

Check that it contains **`index.html` and `src/`**. If those files are
elsewhere, provide their directory using `TURBO_KART_GAME_UI_DIR`.
The script fails with an explicit message if the selected directory lacks
these files and never modifies the original game source.

```sh
cd /mnt/holochain/TurboMobile/Holochain-mobile
export PATH="$HOME/.local/opt/gradle-8.9/bin:$HOME/Android/Sdk/platform-tools:$PATH"
export ANDROID_HOME="$HOME/Android/Sdk"

# Optional override. Otherwise the build script auto-detects resources/ui.
# export TURBO_KART_GAME_UI_DIR="/full/path/to/unpacked/game/UI"

bash scripts/build-turbo-kart-apk.sh
adb devices -l
adb install -r android/turbo-kart-mobile/app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n dev.holochain.turbokart.mobile/.TouchGameActivity
```

The new activity is also a separate launcher icon labeled **Turbo Kart Touch
Race**. Your Holochain diagnostic icon remains available; the app package
and app-private agent/endpoint identities are unchanged with
`adb install -r`. **Do not uninstall or clear app data** during testing.

The build should say:

```text
Android touch game staged: ... -> .../app/src/main/assets/game
```

If it says `game UI not found`, locate the extracted desktop files and
build again using `TURBO_KART_GAME_UI_DIR`.

## Acceptance tests on Galaxy S23 (landscape)

1. Launch the Touch Race icon; verify the actual Turbo Kart title screen and
   character selection, not the Holochain JSON diagnostics.
2. Begin a **single-player** race using the game's own UI.
3. Hold **GO ▲** at the bottom; the kart should accelerate.
4. Without lifting GO, press **◀** or **▶** with a second finger; the kart
   should turn and keep accelerating.
5. Release steering; the kart stops turning but continues accelerating.
6. Release GO; acceleration stops. Background the app during a press, then
   return; no control should remain stuck.
7. Check performance, frame rate, whether assets render, and whether the
   touchscreen overlaps HUD or other game buttons.

The JavaScript pointer-state tests passed locally, but the Android APK
WebView renderer/gameplay integration is **not yet device-verified**.
Synthetic keyboard events are untrusted DOM events; we may need to connect
the overlay directly to the game's real input state if keyboard dispatch
doesn't move the kart. The test exposes
`window.__turboKartTouchState` as a fallback direct input source.

## Build considerations and security

- This uses WebView with a restricted HTTPS-like local asset origin,
  JavaScript enabled, no `addJavascriptInterface`, and file/content access
  disabled. Local `index.html` and models are served from APK assets.
- The game's existing WebGL rendering and physics can be performance-heavy
  on a phone; reducing resolution and quality is a subsequent milestone.
- The game is packaged only when its actual UI assets are available; no
  external image assets are invented.
- Touch/physics behavior on the real phone has not yet been proven.
- Real-time Iroh/MoQ multiplayer and race authentication are separate
  from single-player touch input, and are not modified here.
