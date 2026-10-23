# Contributing to h2hc-linker

## Development Environment

This project requires [Nix](https://nixos.org/) for native dependencies. Always use `nix develop` when running cargo commands:

```bash
nix develop --command cargo build
nix develop --command cargo test
nix develop --command cargo clippy
```

Or enter the shell first:

```bash
nix develop
cargo build && cargo test
```

## Before Submitting a PR

1. Run `cargo fmt` to format code
2. Run `cargo clippy` and fix any warnings
3. Run `cargo test` and ensure all tests pass
4. Keep commits focused and descriptive

## Code Style

- Use strong typing: never use plain `String` for typed values (hashes, URLs, identifiers)
- Reuse types from holochain crates (`holo_hash`, `holochain_types`, `kitsune2_api`) rather than defining new structs
- Use `tracing` for logging, not `println!` or `dbg!`

## License

By contributing, you agree that your contributions will be licensed under the CAL-1.0 (Cryptographic Autonomy License).
