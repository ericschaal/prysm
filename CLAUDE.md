# Working on Prysm

Prysm is a Rust ambient-lighting workspace for Linux/macOS with a supervised LG
webOS capture sender. Use the maintained documentation rather than duplicating it here.

- [Developer setup and checks](docs/developers/README.md)
- [Architecture and workspace map](docs/developers/architecture.md)
- [Technical decisions](docs/developers/decisions/README.md)
- [Contribution and documentation guidelines](docs/developers/guidelines.md)
- [Error handling](docs/developers/error-handling.md)
- [HTTP/configuration contracts](docs/developers/control-api.md)
- [Frontend and generated API](docs/developers/frontend.md)
- [TV lifecycle and permissions](docs/developers/webos.md)
- [Debugging](docs/developers/debugging.md)

Keep `prysm-core` dependency-free. Preserve typed validation, terminal error causes,
ordered lifecycle commands, bounded frame delivery, stop/drain/join ordering and
WLED failure compensation. Native TV tests require the target SDK/hardware and
must retain isolation, timeouts and cleanup. Builds do not deploy.

For runtime instructions use [Run Prysm](docs/users/README.md).
For historical evidence use [Dive deeper](research/README.md); local `work/`
artifacts are not public documentation or runtime dependencies.
