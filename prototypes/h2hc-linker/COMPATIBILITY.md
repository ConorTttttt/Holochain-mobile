# Compatibility

This table tracks which versions of h2hc-linker are compatible with which versions of Holochain and the Holo Web Conductor extension.

| h2hc-linker Version | Holochain Version | HWC Version | Notes |
|---------------------|-------------------|-------------|-------|
| v0.1.0              | 0.6.1-rc.x | v0.1.0-rc.2 and earlier | Initial release |
| v0.1.1              | 0.6.1-rc.x       |      v0.1.0-rc.2       |    Includes PeerCount in ping/pong   |

## Release Process

- **Independent releases**: Most linker updates don't require an extension update. Tag and release each repo independently.
- **Lock-step releases**: When a protocol or API change affects both, tag both repos with the same version and update this table.
- **Triggering a release**: Push a tag matching `v*` (e.g., `git tag v0.1.0 && git push origin v0.1.0`).
