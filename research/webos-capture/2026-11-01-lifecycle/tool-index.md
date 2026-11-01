# Case-local verified tool index

The shared skills/tool-index.md is incomplete while its refresh is running.
Paths below were resolved with `command -v`, readlink and the installed launchers,
not guessed. No global installation or MCP registration occurred. Subsequently, with user authorization, the openlgtv SDK archive was extracted under /tmp and its static Frida Gum libraries linked into a temporary probe (E-012).

| Tool | Absolute path | Version | Verification |
|---|---|---|---|
| Ghidra | /opt/homebrew/Cellar/ghidra/12.2/libexec/support/analyzeHeadless | 12.2 | Ghidra/application.properties; Homebrew ghidraRun wrapper |
| JDK for Ghidra | /opt/homebrew/opt/openjdk@25/libexec/openjdk.jdk/Contents/Home | 25 (package path; runtime log to verify) | Homebrew ghidraRun wrapper |
| Frida | /Users/eschaal/.local/bin/frida | 17.25.0 | frida --version |
| Frida Python | /Users/eschaal/Library/Application Support/pipx/venvs/frida-tools/bin/python | pipx environment | frida launcher symlink |
| radare2 | /opt/homebrew/bin/r2 | 6.2.4 | r2 -v; shared index |
| rabin2 | /opt/homebrew/bin/rabin2 | 6.2.4 | rabin2 -v; shared index |

Frida Gum: `/tmp/prysm-reverse/sdk/arm-lgtv-linux-gnueabi_sdk-buildroot/arm-lgtv-linux-gnueabi/sysroot/usr/lib/libfrida-gum-1.0.a`; source commit `0c8df6742d2c7a08c32fc3c3b691779b062fb34a`, pkg-config version 0.0.0. The installed Mac Frida CLI 17.25.0 was not used to attach to the TV.
