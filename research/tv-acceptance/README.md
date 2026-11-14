# TV acceptance records

Status: no run recorded yet.

One file per run of
[`tv-acceptance.sh`](../../packaging/webos/tests/tv-acceptance.sh),
named `DATE-FLAVOUR.md` (`2026-11-20-full.md`). Each starts with the status,
package, commit, TV and limits, then holds the output of the on-TV smoke test
at its three levels and of the install, and the answer and note for each
manual check. A run that fails stops at that step and says so in its status.

A record describes one package on one TV at one time; see
[Hardware checks](../../docs/developers/debugging.md#hardware-checks) for how
to run one and [the TV guide](../../docs/developers/webos.md) for what is
being tested.
