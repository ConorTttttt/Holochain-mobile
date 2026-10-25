# Turbo Kart Touch Preview — first playable Android milestone

**Status:** implemented on `feat/turbo-kart-touch-preview`, but **not yet
compiled, installed or touch-tested on the physical Galaxy S23**. Do not
mistake the previous successful Holochain/Iroh APK tests for a gameplay test.

## Goal and scope

Reuse the **exact web game packaged by Turbo Kart desktop** (Three.js,
existing kart physics, AI, menus, race state, track models) inside a secure
Android WebView. Inject a phone-only multitouch overlay; do **not** change
the original desktop game sources or DNA hashes.

This preview is **single-player only**. The existing multiplayer test
screen and Android Lair/Iroh identities remain unchanged. The 3D game
will not yet receive native authenticated Iroh/MoQ race packets.

Architecture:

```text
Android APK
  MainActivity (existing Holochain / Iroh debug UI)
    └── Play Touch Preview (single player)
         └── TouchGameActivity (landscape WebView)
              ├── original packaged Three.js game files
              └── touch-controls.js
                    pointer hold/release → KeyboardEvent codes
                    → unchanged InputController → unchanged kart physics
```

The game runs from
`https://appassets.androidplatform.net/assets/game/index.html`,
using AndroidX `WebViewAssetLoader`. No native `addJavascriptInterface`
bridge, Holochain token, admin socket or signing capability is exposed
to the gameplay page.

## On your HP Pro Mini

The exact desktop-packaged web UI normally lives at:

```text
/mnt/holochain/offloaded/sf/Turbokart/turbo-kart/apps/turbo-kart-desktop/resources/ui
```

It must contain `index.html` and `src/main.js`. This should be the
**already built desktop game**, including its vendored Three.js and all
models/maps. That avoids silently copying an older upstream copy that
doesn't match your current tracks and menu code.

First inspect the available files:

```sh
BASE=/mnt/holochain/offloaded/sf/Turbokart/turbo-kart/apps/turbo-kart-desktop
ls -lh "$BASE/resources/ui/index.html" "$BASE/resources/ui/src/main.js"
```

If missing, locate the existing desktop game assets or use its game's
source checkout (typically `turbo-kart-rally-main`) as
`TURBO_KART_UI_DIR`. The source checkout might use network imports for
Three.js; prefer the vendored desktop runtime. Never rebuild the DNAs
just to test rendering.

### Fetch and test

```sh
cd /mnt/holochain/TurboMobile/Holochain-mobile
git fetch origin feat/turbo-kart-touch-preview
git switch --track origin/feat/turbo-kart-touch-preview

# Fast, browser-independent tests for virtual key release/held state.
node --test tests/touch-controls.test.mjs
```

If the feature branch is already checked out, simply
`git merge --ff-only FETCH_HEAD` instead of `git switch --track`.

### Stage original game and build the debug APK

```sh
export PATH="$HOME/.local/opt/gradle-8.9/bin:$HOME/Android/Sdk/platform-tools:$PATH"
export ANDROID_HOME="$HOME/Android/Sdk"
export TURBO_KART_UI_DIR="$BASE/resources/ui"

bash scripts/build-turbo-kart-touch-apk.sh
```

This wrapper stages only `index.html`, `src/`, `assets/`,
`vendor/`, `LICENSE`, `NOTICE` and `icon.png` if present, plus
the touch-controller overlay, then runs the existing
`build-turbo-kart-apk.sh` native + Gradle pipeline.
No `.git/`, developer files, hApp code modifications or private keys
are included in the game assets.

The expected debug APK is:

```text
android/turbo-kart-mobile/app/build/outputs/apk/debug/app-debug.apk
```

### Install without deleting the existing Android identity

```sh
adb devices -l
adb install -r android/turbo-kart-mobile/app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n dev.holochain.turbokart.mobile/.MainActivity
```

**Do not uninstall** the app or run `pm clear`, since this can destroy
Android app-private Lair and Iroh identity data.
The original multiplayer controls remain in the starting Activity.
Tap **Play Touch Preview (single player)** to open the original game.

## Expected on-device behaviour

- Preview automatically rotates to landscape and hides system bars.
- Tap the game's original title and racer/class selection; start a
  single-player race through its own `RACE!` button.
- A phone-only touch overlay appears in the intro/countdown/race:
  **left**, **right**, **GO (accelerate)**, **BRAKE**, **DRIFT**, **ITEM**
  and **PAUSE**.
- Multiple fingers can hold steering + gas + drift simultaneously.
- Pointer cancellation, app switching, hiding the controls and other
  focus changes release held keys. Item sends a normal press edge.
- Touch controls hide in title, racer selection and paused menus so the
  original tappable UI remains accessible.
- Android Back returns to the existing APK control-plane screen.

The touch shim emits existing key codes (no forks of the game physics):

| Touch | Original key code |
| --- | --- |
| Left / right | `ArrowLeft` / `ArrowRight` |
| GO / brake | `ArrowUp` / `ArrowDown` |
| Drift | `Space` |
| Item | `KeyE` |
| Pause | `Escape` |

## Verification / limitations

**Success criterion for this milestone:** one complete **single-player**
race is controllable by touchscreen on the Galaxy S23, with frame rate
and menu layout adequate for initial testing. This has not been
demonstrated yet; send results from the device and we'll refine it.

This first WebView preview doesn't support live multiplayer racing,
native Iroh/MoQ packet injection, changing Holochain sessions from
the game, release signing, background racing or battery optimisation.
Graphics features (WebGL2, Three.js r170, post-processing and fonts)
may require performance tuning in Android WebView. Any games still
depending on external CDN imports also need internet connectivity.
