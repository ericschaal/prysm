# Constructor registry leak and lost wakeup investigation

Authorized ARM32 embedded-Linux capture investigation. No target address, account, private path, captured pixels or binary sample is included here.

- E-008: Native Frida Gum listeners on register, unregister and free, followed by a read-only registry walk, distinguish a descriptor leak from a dangling global object pointer. Capture failed-constructor cleanup, not merely successful destruction.
- E-005: One object can survive repeated public init/stop/finalize sessions with stable short-run resource counts; this does not establish safe final destruction.
- E-007: Inject a small delay exactly before the native condition wait, then compare restart without retry and with one public lifecycle retry. A recovered start does not imply bounded shutdown.

Core finding F-001: registering an object before a throwing constructor step needs exception-safe unregister. Catching at the outer API cannot repair an object that was freed without removing global references.

Reusable path: decompile ownership and unwind paths → targeted native hooks → denied/success controls → inspect retained registry membership without dereferencing freed objects.

Tool lesson: stock Frida Linux ARM hard-float assets are not a verified ABI match for an ARM soft-float process. A platform SDK may supply compatible Gum archives. Check ELF ABI and dependencies, hide bundled runtime symbols, and assert a harmless self-hook first. Broad allocator hooks can perturb valid threaded teardown; keep that result separate and narrow the hook set before drawing conclusions.

Checked: no target IP, credentials, private paths, user identity, binary attachments or pixels. Journal retained locally rather than published to a shared skill repository.
