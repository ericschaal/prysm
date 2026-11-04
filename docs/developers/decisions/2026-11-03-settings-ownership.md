# Settings and application ownership

Date: 2026-11-03

CLI/environment own process options; browser JSON owns input, WLED/LEDs,
processing, profiles and selected source. Neither overrides the other.
Power intent is not saved; startup is idle. Missing settings open setup.

One controller serializes accepted mutations. The settings store validates,
checks revisions and writes atomically before updating memory. HTTP owns transport;
the runtime owns capture/lighting ordering. The browser keeps one draft and uses
complete PUTs with expected revisions; setup uses the same fields/save operation.
