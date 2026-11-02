# Work items

| ID | title | role | targets | surface | status | evidence | notes |
|----|-------|------|---------|---------|--------|----------|-------|
| WI-001 | Scope and tool identity | lead | authorized samples | metadata | done | E-001 | User-approved setup |
| WI-002 | Constructor and teardown call paths | lead | three ELF libraries | static | done | E-002 E-003 E-004 | Ghidra and assembly |
| WI-003 | Persistent driver and restart retry | lead | isolated probe | public API | done | E-005 E-006 E-007 | Mitigation, not bounded cleanup |
| WI-004 | Frida Gum registry tracing | lead | isolated probe | hooks | done | E-008 E-009 E-010 E-012 | Broad-hook timeout remains unexplained |
| WI-005 | Cleanup and report | lead | case and TV temp artifacts | audit | done | E-011 | Installed service preserved |

## Coverage
- [x] Targeted lifecycle analysis complete for authorized samples
- [x] Candidates triaged; downstream stale-pointer use remains a static risk
- [x] Findings linked to evidence
- [x] Call paths documented
- [x] Timeline major phases recorded
- [x] Report follows docs-generator structure, flavor null
- [x] Anonymized journal retained locally
