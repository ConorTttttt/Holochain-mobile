# Turbo Kart mobile touchscreen playtest

**Branch:** `feat/turbo-kart-touch-controls`

The app includes a separate Android landscape launcher named **Turbo Kart
Touch Race** that loads the existing Three.js Turbo Kart web game, not a
recreation of the kart physics. The Holochain/Iroh diagnostic application
remains independent and unchanged.

## Controls

- **Starting boost, your timing:** during the starting countdown,
  **touch and hold anywhere** on the screen to hold **ArrowUp**, then release
  when you want to release the accelerator. This lets you try Turbo Kart's
  existing manual start-boost mechanic rather than preventing it with a
  mandatory auto-throttle lock. Touching too early can still cause a
  spinout—timing remains in your hands.
- **Automatic driving after GO:** when the game exposes a separate
  `countdown` phase, auto-acceleration starts about **200 ms** after
  entering `race`/`racing`. If the game enters `race` directly without
  exposing a countdown, the previous **3.6-second fallback** still applies.
  You can manually hold accelerate during either pre-GO window. When the
  automatic driving gate opens, acceleration stays held even if you lift
  your finger, with **no keyup/keydown interruption**.
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
countdown. The screen's invisible pointer layer captures presses across
both steering halves **and the two buttons** for manual boost; this does
not change the buttons' normal actions. A countdown release stops manual
acceleration unless the auto-throttle gate has opened. There is **no
automatic throttle in the intro or countdown**, so starting boosts must
be initiated by your touch. Menus and results remain clickable.
On pause/background/focus loss, all held controls and pending FIRE pulses
release. Elapsed wall-clock time while hidden cannot unlock a countdown.
After GO, auto-acceleration resumes when gameplay regains focus.

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

1. Start a single-player race. With **no touch**, the kart does not
   accelerate during the intro/countdown, avoiding the accidental spinout.
2. During the countdown, **touch and hold anywhere** (left, right, FIRE,
   or LOOK BACK area): manual acceleration begins. Release before GO and
   it stops. Try different timing to trigger the game's normal starting
   boost, which may penalise holding too early.
3. Try **holding through GO**, then lifting. Automatic acceleration should
   take over without a pause or keyup event.
4. Start another race, **without touching** during the countdown.
   Auto-acceleration should still begin after GO (or following the
   3.6-second fallback if the game does not expose an explicit countdown).
5. Press and hold anywhere on the left half: steering left, with no visual
   highlight or text on the steering region.
6. Press and hold anywhere on the right half: steering right, again with
   no colour change.
7. Release steering: straight travel continues under auto acceleration.
8. Tap the **FIRE** button with an equipped weapon: it fires exactly once,
   subject to the game's normal rules. Tapping empty steering space does not
   fire **as a weapon**; it activates boost only during countdown.
9. Hold **LOOK BACK**: camera looks backward while the button is held;
   releasing it returns to the forward view.
10. While holding one steering half, use either action button with another
    finger. Both actions should work without interfering with steering.
11. Background/return to the app: no stuck boost, look-back or steering input.
12. Confirm the buttons do not obscure important HUD elements at landscape
    display size.

**Status:** The previous touch controls were user-confirmed working on the
Galaxy S23. The revised dedicated **FIRE**, **LOOK BACK**, invisible halves
and no pressed-state shading are committed; The new manual-start-boost implementation was exercised with 37
automated state/DOM checks (including multi-touch, button presses,
backgrounding, false-start timing and boost-to-auto throttle handoff).
Those checks do **not** confirm the exact start-boost timing in the original
3D game's physics; test that behaviour on the Galaxy S23. This new APK has
not yet been built or device-tested.

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
