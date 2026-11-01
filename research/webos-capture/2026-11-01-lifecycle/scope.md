# Case Scope

## meta
- case_id: vt-lifecycle
- created: 2026-11-01T11:03:54+01:00
- operator: local
- project_root: research/webos-capture/2026-11-01-lifecycle
- original_project_root: /tmp/prysm-reverse
- primary_skill: ghidra-reverse/SKILL.md
- primary_id: R22
- lead_role: lead (single agent)
- specialist_roles: [static and dynamic analysis performed by lead; no delegated agents]
- hint: Ghidra constructor cleanup and teardown analysis
- preset: none

## auth
- status: granted
- basis: own_system
- evidence_of_auth: User authorized local library analysis and isolated temporary TV probes in this chat
- MUST NOT proceed if status != granted

## in_scope
- assets:
  - /tmp/prysm-cleanup-evidence
  - 192.168.18.10
- surfaces: [authorized local ELF copies, isolated TV probe processes]
- activities: [static decompilation, ownership and synchronization tracing, bounded lifecycle probes]

## out_of_scope
- assets: []
- activities: [system partition writes, firmware changes, vendor binary replacement, reboot, power or input changes, installed Prysm changes, unrelated process attachment, arbitrary thread cancellation]

## network_profile
- mode: authorized_target_only
- notes: |
    offline | lab_only | authorized_target_only | unrestricted_lab
    Public documentation and the openlgtv SDK download were separately authorized by the user. Target instrumentation remains restricted to owned temporary probes.
    Presets: offline-sample | ctf-public | own-system

## deliverables
- report: true
- field_journal: true
- diagrams: true
- timeline: true

## constraints
- timebox: {}
- stealth: low
- data_handling: anonymize

## signoff
- ready_for_act: true
- checklist:
  - [x] auth.status = granted
  - [x] in_scope.assets non-empty OR offline sample path set
  - [x] network_profile.mode chosen
  - [x] out_of_scope reviewed
  - [x] roles assigned: single lead

## ops_refs
- skills/ops/scope-contract.md
- skills/ops/evidence-finding-path.md
- skills/ops/role-map.md
- skills/ops/timeline-workitem.md
- skills/ops/IDENTITY.md
