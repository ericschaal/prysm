# Timeline (append-only)

## 2026-11-01T11:03:54+01:00 | lead | init
- action: case-init
- command_or_ref: skills/scripts/case-init.sh
- result_summary: case directory created; scope ready_for_act=true
- artifacts: [scope.md, workitems.md]
- evidence_ids: []
- decision_delta: [case_initialized]
- carry_forward_refs: [scope.md]
- next: open PRIMARY SKILL.md and ACT within scope

## 2026-11-01T10:23:29.819748+00:00 | lead | static retrospective record
- action: Headless Ghidra import/export
- command_or_ref: evidence records and scripts
- result_summary: Reviewed constructor unwind, callback semantics, buffer cleanup and wakeup paths
- artifacts: [evidence, report]
- evidence_ids: [E-001, E-002, E-003, E-004]
- decision_delta: [recorded findings]
- carry_forward_refs: [scope.md]
- next: handoff

## 2026-11-01T10:23:29.819748+00:00 | lead | reuse retrospective record
- action: Temporary TV reuse controls and delayed wait
- command_or_ref: evidence records and scripts
- result_summary: Normal reuse succeeds; forced restart lost wake; one lifecycle retry succeeds
- artifacts: [evidence, report]
- evidence_ids: [E-005, E-006, E-007]
- decision_delta: [recorded findings]
- carry_forward_refs: [scope.md]
- next: handoff

## 2026-11-01T10:23:29.819748+00:00 | lead | gum retrospective record
- action: Verified SDK; built and ran native Gum probes
- command_or_ref: evidence records and scripts
- result_summary: Confirmed freed pointers retained on failed construction; successful targeted control empties registry; broad allocator-hook timeout inconclusive
- artifacts: [evidence, report]
- evidence_ids: [E-008, E-009, E-010, E-012]
- decision_delta: [recorded findings]
- carry_forward_refs: [scope.md]
- next: handoff

## 2026-11-01T10:23:29.819748+00:00 | lead | finish retrospective record
- action: Target cleanup and report
- command_or_ref: evidence records and scripts
- result_summary: No temporary probe artifacts; supervisor still present; no production lifecycle changes
- artifacts: [evidence, report]
- evidence_ids: [E-011]
- decision_delta: [recorded findings]
- carry_forward_refs: [scope.md]
- next: handoff

## 2026-11-01 | lead | repository archive
- action: User requested research folder in the Prysm repository
- result_summary: Archived reports, sources, runtime logs and metadata; vendor binaries, disassembly, decompilations and SDK remain external. Updated diagnosis links. Original temporary files retained as backup.
- evidence_ids: [E-001, E-002, E-003, E-004, E-005, E-006, E-007, E-008, E-009, E-010, E-011, E-012]
- next: No live TV work required for this move
