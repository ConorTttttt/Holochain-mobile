# Turbo Kart mobile Holochain smoke test

This is the **first integration milestone**, not an installable Android game. It
reuses the existing H2HC/Wasmi conductor, installs the two **unchanged** Turbo Kart
hApps with **one agent key**, compares their DNA hashes with the desktop
`expected-dna-hashes.json`, and offers two read-only lobby commands.

Source: `prototypes/h2hc-conductor/src/bin/turbo-kart-smoke.rs`.
Nothing in the existing `h2hc-phone` benchmark, Turbo Kart desktop app or DNAs
is changed.

## Prepare on the Linux build machine

Obtain both hApp bundles and the expected-hashes JSON **from the same Turbo Kart
desktop build**. The desktop build script writes:

- `turbo-kart/apps/turbo-kart-desktop/.happs/gaming_directory/gaming_platform.happ`
- `turbo-kart/apps/turbo-kart-desktop/.happs/turbo_kart/turbo_kart.happ`
- `turbo-kart/apps/turbo-kart-desktop/expected-dna-hashes.json`

If the bundles do not already exist, consult the Turbo Kart desktop build
instructions (`yarn pack:happs`, requiring its sibling game source tree).
Do **not** casually rebuild or change the expected hashes: a different DNA hash
represents a different Holochain network.

First build the mobile project's pinned dependencies:

```sh
./scripts/vendor-holochain.sh
cargo build --release -p h2hc-conductor --bin turbo-kart-smoke --no-default-features --features wasmi
```

The H2HC linker must be running and attached to suitable full-arc authorities.
This standalone command does not start the linker or test the desktop
integration by itself.

Run the smoke harness on a **fresh disposable data directory**:

```sh
target/release/turbo-kart-smoke \
  --linker-url ws://127.0.0.1:8090/conductor/v1 \
  --linker-key '<base64-linker-public-key>' \
  --directory-happ /absolute/path/to/gaming_platform.happ \
  --kart-happ /absolute/path/to/turbo_kart.happ \
  --expected-hashes /absolute/path/to/expected-dna-hashes.json \
  --data-dir /tmp/turbo-kart-mobile-smoke-new
```

Use the linker's **actual** address and public key. After the `ready` JSON
message, type one JSON command per line:

```json
{"cmd":"install"}
{"cmd":"list_open_lobbies","limit":10}
{"cmd":"quit"}
```

For a known lobby ID, use
`{"cmd":"get_lobby","id":"<lobby-action-hash>"}`.

An `install` result with `both_enabled: true`, the same agent for both
apps, and matching DNA hashes is the first passing criterion. Seeing a
desktop-created lobby in `list_open_lobbies` is a **separate networking test**
and has not yet been demonstrated.

## Android

`scripts/build-android.sh` builds the original benchmark binary only. The
new smoke binary has **not** yet been cross-compiled or tested on Android.
When ready, add it to the Android build script and run it through ADB with
a reachable H2HC linker. On-device USB testing requires `adb reverse` for
a localhost linker, or a reachable network address.

## Security and limitations

- This harness uses Holochain's **in-memory test keystore**, so its keys do
  **not** survive restarts. Run on a fresh profile every time.
- It must **not** be used for production accounts or valuable player data.
- It is a command-line prototype; it is not an Android APK or iOS app.
- No cross-version (mobile Holochain 0.7.1 vs desktop 0.7.0) networking
  interoperability or desktop lobby test is claimed yet.
- It does not start races or implement Iroh/MoQ realtime sessions.
