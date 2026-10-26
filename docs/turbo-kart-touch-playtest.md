# Turbo Kart mobile touchscreen playtest

**Branch:** `feat/turbo-kart-touch-controls`

The app includes a separate Android landscape launcher named **Turbo Kart
Touch Race** that loads the existing Three.js Turbo Kart web game, not a
recreation of the kart physics. The Holochain/Iroh diagnostic application
remains independent and unchanged.

## Controls

- **Safe race start:** never holds **ArrowUp** in `intro` or `countdown`.
  When the game exposes a separate `countdown` phase, it auto-accelerates
  about **200 ms** after the transition to `race`/`racing`. If the game
  enters `race` without exposing a countdown, it waits **3.6 seconds**
  before accelerating. This deliberately trades a slower initial launch
  for avoiding an early-throttle spinout; no GO button is needed.
- **Left 50% of screen:** hold to steer left (**ArrowLeft**).
- **Right 50% of screen:** hold to steer right (**ArrowRight**).
- **FIRE button:** sends a single approximately 145 ms press of **E**
  (`KeyE`) to use the equipped weapon; normal weapon/item rules still apply.
- **LOOK BACK button:** **hold** to send **C** (`KeyC`) and look behind,
  then release to restore the normal view.

Both steering halves are **fully transparent**: no "Press here to turn"
labels, coloured press highlights, or Auto GO text. They are always
50%-width throughout gameplay, except for the visible buttons over them.
The **FIRE** and **LOOK BACK** buttons are visually static while pressed and
are positioned together at the **bottom centre** of the landscape screen.
Their touches are handled separately, so touching a button does not steer.

A quick tap anywhere in a steering half **only steers momentarily**; it does
not fire a weapon anymore. The weapon fires only through the FIRE button.
Multitouch enables steering while firing or looking behind. Two opposing
steering touches cancel steering but never cancel auto acceleration.
Repeated look-back pointers hold the rear view until the last is released.

The controls appear only during gameplay states (`intro`, `countdown`,
`race`, `racing`). Steering and FIRE/LOOK BACK remain available during
countdown, but automatic throttle is **locked**. Menus and results remain
clickable. On pause/background/focus loss, acceleration, steering, Look
Back and pending FIRE pulses release. The throttle cannot be unlocked by
elapsed wall-clock time while a countdown is hidden. Once GO has already
been reached, focusing an ongoing race resumes auto acceleration.

Turbo Kart's published desktop input documentation maps look behind to
**C**; its item integration test presses **E** to fire. **Space** is drift,
not weapon firing.

## Build and install alongside Holochain/Iroh APK

From the separate touch project worktree:

```sh
cd /mnt/holochain/TurboMobile/Holochain-mobile-touch
git fetch origin feat/turbo-kart-touch-controls
git merge --ff-only FETCH_HEAD

node --test mobile/touch-controls/*.test.mjs

export ANDROID_HOME="$HOME/Android/Sdk"
export PATH="$HOME/.local/opt/gradle-8.9/bin:$HOME/Android/Sdk/platform-tools:$PATH"

# Stage original game UI and build the separate touchscreen debug APK.
bash scripts/build-turbo-kart-apk.sh

adb install -r android/turbo-kart-mobile/app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n \
  dev.holochain.turbokart.mobile.touch/dev.holochain.turbokart.mobile.TouchGameActivity
```

The build script automatically looks for the original unpacked game in
`/mnt/holochain/offloaded/sf/Turbokart/turbo-kart/apps/turbo-kart-desktop/resources/ui`,
`/opt/Turbo Kart Rally/resources/app.asar.unpacked/resources/ui`, or the
original `turbo-kart-rally-main` sibling directory. The source directory
must contain both `index.html` and `src/`. If autodetection fails, set
`TURBO_KART_GAME_UI_DIR` to the correct game UI directory.

The staging script adds only the touch JS file and its module import to
the generated APK copy of the real game. It does **not** change desktop game
files or either production Holochain DNA.

The touchscreen APK has its **own debug application ID**
(`dev.holochain.turbokart.mobile.touch`) and installs alongside the
existing diagnostic APK (`dev.holochain.turbokart.mobile`). Its
app-private data is separate. **Do not uninstall or clear the diagnostic
APK's data while testing.**

## Galaxy S23 acceptance checks

1. Start a single-player race. **During the countdown the kart must NOT
   accelerate or spin out.** Confirm no movement until GO. After GO, it
   accelerates automatically without touching. If no countdown state is
   exposed by the game, it waits roughly 3.6 seconds before accelerating.
2. Press and hold anywhere on the left half: steering left, with no visual
   highlight or text on the steering region.
3. Press and hold anywhere on the right half: steering right, again with
   no colour change.
4. Release steering: straight travel continues under auto acceleration.
5. Tap the **FIRE** button with an equipped weapon: it fires exactly once,
   subject to the game's normal rules. Tapping empty steering space does not
   fire.
6. Hold **LOOK BACK**: camera looks backward while the button is held;
   releasing it returns to the forward view.
7. While holding one steering half, use either action button with another
   finger. Both actions should work without interfering with steering.
8. Background/return to the app: no stuck look-back or steering input.
9. Confirm the buttons do not obscure important HUD elements at landscape
   display size.

**Status:** The previous touch controls were user-confirmed working on the
Galaxy S23. The revised dedicated **FIRE**, **LOOK BACK**, invisible halves
and no pressed-state shading are committed; 11 state tests, three mock-DOM interaction tests and seven dedicated
race-start-gate tests passed (21 total). These tests do not replace
on-device verification. The new APK
has **not yet been built or device-tested**, so confirm on the phone.

## Implementation notes and limitations

- The overlay is injected as `mobile-touch.mjs`, independent of the original
  game source, and maps to the original keyboard control inputs.
- FIRE is a single bounded keypress, not a repeating press; multiple rapid
  touch presses during one active key pulse are coalesced.
- Synthetic keyboard events are browser-generated (not trusted hardware
  input), but the earlier touch playtest confirmed control compatibility.
- The game WebView has no privileged JavaScript bridge or conductor secrets.
- Native racing performance and real Iroh/MoQ race authentication are
  separate features and are not modified in this change.
