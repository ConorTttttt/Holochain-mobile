# Turbo Kart mobile touchscreen driving — first playable UI experiment

**Branch:** `feat/turbo-kart-touch-controls`

This change adds a **separate landscape Android launcher** called
**Turbo Kart Touch Race**. It loads the **existing Three.js desktop game**
in an isolated Android WebView and overlays two transparent steering halves:

- **Automatic forward acceleration:** holds **ArrowUp** while a race is active.
- **Left half:** hold anywhere on the left to steer left (**ArrowLeft**).
- **Right half:** hold anywhere on the right to steer right (**ArrowRight**).
- **Quick tap on either half:** fire the equipped weapon using **E / KeyE**.
  Turbo Kart's desktop item integration test uses E; **Space is drift**, not fire.

A short tap (up to 230 ms, without swiping more than 22 px) fires on
release. A longer hold steers continuously without firing. You can tap with
a second finger while the first finger keeps steering. Sliding across
the middle switches steering; crossing does not fire. Holding both halves
cancels steering and acceleration continues. The GO button has been removed.

Menus and results remain unblocked: the overlay appears only in racing
states (`intro`, `countdown`, `race`, `racing`). Leaving a race, hiding or
backgrounding the app, or losing focus releases held acceleration/steering
and pending weapon-key pulses. Acceleration resumes when gameplay regains focus.

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
adb shell am start -n dev.holochain.turbokart.mobile.touch/dev.holochain.turbokart.mobile.TouchGameActivity
```

The new activity is also a separate launcher icon labeled **Turbo Kart Touch
Race**. For safety, this touch-playtest branch changes the debug build
application ID to `dev.holochain.turbokart.mobile.touch`. It installs
**alongside**, rather than over, your existing Holochain/Iroh test APK
(`dev.holochain.turbokart.mobile`); that app's keys, endpoint identity and
test data remain untouched. Do not uninstall or clear either app's data.

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
3. Wait for the race to begin **without touching the screen**; the kart
   should accelerate forwards automatically.
4. Hold the **left half**; the kart steers left while accelerating.
5. Release and hold the **right half**; the kart steers right.
6. Release steering; the kart straightens but keeps accelerating.
7. **Tap quickly** anywhere on either half with an equipped item:
   the item fires once. A longer steering press should not fire.
8. Hold to steer with one finger and tap to fire with the other.
9. Background the app while steering; no control should remain stuck.
10. Verify the title/selection screens work and item/touch hints do not obscure
    important HUD information. Recheck rendering and frame rate.

The previous GO-button touch implementation was confirmed working on the
Galaxy S23 by the user. This newer **auto-accelerate + half-screen steering +
tap-to-fire** revision has unit tests in
`mobile/touch-controls/mobile-touch.test.mjs`; run those and rebuild the APK
before claiming this behavior works on-device. Synthetic KeyboardEvents are
not trusted hardware events, but the previous revision already demonstrated
that the game's keyboard handler accepts steering and throttle. Verify E-key
item firing with an equipped weapon; if the game input mapping has changed,
inspect its actual item-key handler before adjusting the overlay.
`window.__turboKartTouchState` exposes the current touch snapshot for
diagnostics.

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
