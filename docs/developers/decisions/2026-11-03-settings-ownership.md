# Settings and application ownership

Date: 2026-11-03

CLI/environment own process options; browser JSON owns input, WLED/LEDs,
processing, profiles and selected source. Neither overrides the other.
Power intent is not saved; startup is idle. Missing settings open setup.

One controller serializes accepted mutations. The settings store validates,
checks revisions and writes atomically before updating memory. HTTP owns transport;
the runtime owns capture/lighting ordering. The browser keeps one draft and uses
complete PUTs with expected revisions; setup uses the same fields/save operation.

## Update for Prysm 1.x (2026-11-12)

Still holds: command-line options and the settings file never override each
other, startup is idle, missing settings open setup, and saves are complete
documents checked against a revision.

Changed in 1.x ([workspace](../workspace.md)): the selected mode (Screen sync or
device effects) is no longer saved; `--start` requests lighting at launch.
The settings path is a required argument and there are no `PRYSM_*`
environment variables ([args.rs](../../../hosts/common/src/args.rs)).
The engine replaces the controller; loaded settings that no longer validate
are kept and reported instead of stopping startup.
