#!/usr/bin/env python3
"""Isolated Luna/supervisor probe using temporary files and its own volatile manifest."""
import argparse
import json
from pathlib import Path
import shlex
import subprocess
import time

SERVICE = "io.prysm.capture.probe.service"
MANIFEST = "/var/luna-service2/manifests.d/io.prysm.capture.probe.json"
ROOT = Path(__file__).resolve().parents[3]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", required=True, help="SSH destination, e.g. root@192.168.18.10")
    parser.add_argument("--supervisor", action="store_true", help="exercise sandbox mutations with fake children")
    parser.add_argument("--capture", action="store_true", help="also acquire bounded 320x180 frames with the temporary worker role")
    args = parser.parse_args()
    args.supervisor = args.supervisor or args.capture
    ssh = ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=8", args.host]

    def remote(command, data=None, timeout=15, check=True):
        return subprocess.run(ssh + [command], input=data, capture_output=True,
                              timeout=timeout, check=check)

    def write(path, data, mode="600"):
        # noclobber makes an existing path a hard failure, including dangling symlinks.
        remote(f"set -euC; umask 077; cat > {shlex.quote(path)}; chmod {mode} {shlex.quote(path)}", data)

    def request(payload, public=False, method="status", app=None):
        binary = "luna-send-pub" if public else "luna-send"
        identity = " -a " + shlex.quote(app) if app else ""
        command = f"timeout 12 {binary}{identity} -w 10000 -n 1 luna://{SERVICE}/{method} {shlex.quote(json.dumps(payload))}"
        # This TV's luna-send exits without a reply when stdin reaches EOF.
        with subprocess.Popen(ssh + [command], stdin=subprocess.PIPE,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE) as process:
            try:
                process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
                raise
            output = process.stdout.read()
            error = process.stderr.read()
            if process.returncode != 0 or not output:
                raise RuntimeError(f"Luna request failed ({process.returncode}): {output!r} {error!r}")
            return json.loads(output)

    def eventually(check, label):
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            value = request({})
            if check(value):
                return value
            time.sleep(0.2)
        raise AssertionError(f"{label}: {value}")

    prefix = "prysm-supervisor." if args.supervisor else "prysm-luna-probe."
    directory = remote("mktemp -d /tmp/" + prefix + "XXXXXX").stdout.decode().strip()
    if not directory.startswith("/tmp/" + prefix) or "/" in directory[len("/tmp/"):]:
        raise RuntimeError("unexpected temporary path")
    binary = directory + "/prysm-webos-service"
    manifest = {
        "id": "io.prysm.capture.probe", "version": "1.0.0",
        "roleFiles": [directory + "/role.json"],
        "serviceFiles": [directory + "/service.service"],
        "clientPermissionFiles": [directory + "/client.json"],
        "apiPermissionFiles": [directory + "/api.json"],
    }
    manifest_data = (json.dumps(manifest) + "\n").encode()
    subscription = None
    receiver_pid = None
    pid = None
    try:
        if args.capture:
            # Refuse a test while an installed capturer is present; never stop it.
            running = remote("ps -ef").stdout.decode()
            assert "prysm-webos-capture" not in running, "capture worker already running"
            manifest["roleFiles"].append(directory + "/capture.role.json")
            manifest_data = (json.dumps(manifest) + "\n").encode()
            capture_role = json.loads((ROOT / "packaging/webos/service/capture.role.json").read_text())
            capture_role["exeName"] = directory + "/worker"
            write(directory + "/capture.role.json", json.dumps(capture_role).encode())
        write(binary, (ROOT / "target/armv7-unknown-linux-gnueabi/webos/prysm-webos-service").read_bytes(), "700")
        role = {"exeName": binary, "type": "regular", "allowedNames": [SERVICE],
                "permissions": [{"service": SERVICE, "inbound": ["*"],
                                 "outbound": ["com.webos.service.tvpower"]}]}
        write(directory + "/role.json", json.dumps(role).encode())
        # This group can read power state. The role restricts outbound service access.
        write(directory + "/client.json", json.dumps({SERVICE: ["activities.callbacks"]}).encode())
        methods = ["status", "start", "stop", "setConfig", "cleanup"] if args.supervisor else ["status"]
        write(directory + "/api.json", json.dumps({"public": [SERVICE + "/" + method for method in methods]}).encode())
        options = " --sandbox " + directory if args.supervisor else ""
        if args.capture:
            write(directory + "/worker", (ROOT / "target/armv7-unknown-linux-gnueabi/webos/prysm-webos-capture").read_bytes(), "700")
        elif args.supervisor:
            write(directory + "/worker", b"#!/bin/sh\ntrap '' TERM\nprintf '%s\\n' '{\"type\":\"connected\"}' >&\"$PRYSM_EVENTS_FD\"\nwhile :; do sleep 1; done\n", "700")
        if args.supervisor:
            write(directory + "/scan-services", b"#!/bin/sh\nexit 0\n", "700")
        write(directory + "/service.service",
              f"[D-BUS Service]\nName={SERVICE}\nExec={binary} --run-for 30{options}\nType=dynamic\n".encode())
        write(MANIFEST, manifest_data, "644")
        remote("ls-control scan-services")

        if args.supervisor:
            # Exercise the real Rust setup/activation path without installed paths.
            value = json.loads(remote(f"{binary} --setup --sandbox {directory}").stdout)
            assert value["returnValue"] is True, value
            print("PASS Rust sandbox setup and activation", flush=True)
        value = request({})  # Launch through Luna, rather than starting it over SSH.
        assert value["returnValue"] is True, value
        pid = int(value["pid"])
        value = eventually(lambda v: v["power"] is not None, "power observation")
        assert value["error"] is None, value
        print("PASS native Luna activation and power observation:", json.dumps(value), flush=True)
        value = request({}, public=True)
        assert value["returnValue"] is True, value
        print("PASS public-bus status request", flush=True)
        value = request({"subscribe": "yes"})
        assert value["returnValue"] is False, value
        print("PASS malformed request rejected", flush=True)
        value = request({"subscribe": True})
        assert value["subscribed"] is True, value
        print("PASS native subscription acknowledgement", flush=True)

        subscription = subprocess.Popen(ssh + [f"luna-send -i luna://{SERVICE}/status '{{\"subscribe\":true}}'"],
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        value = eventually(lambda v: v["subscribers"] == 1, "subscription registered")
        subscription.terminate()
        subscription.communicate(timeout=5)
        subscription = None
        eventually(lambda v: v["subscribers"] == 0, "subscription cancelled")
        print("PASS subscription registration and cancellation", flush=True)

        if args.supervisor:
            identity = "io.prysm.capture.probe.app"
            assert request({}, method="start")["returnValue"] is False
            config = {"receiver": "127.0.0.1:19400", "source": "both", "compression": "lz4", "autostart": False}
            if args.capture:
                receiver_source = (ROOT / "packaging/webos/tests/loopback-receiver.js").read_bytes()
                write(directory + "/receiver.js", receiver_source)
                receiver_pid = int(remote(f"timeout 18 node {directory}/receiver.js </dev/null >{directory}/receiver.log 2>&1 & echo $!").stdout)
                remote(f"timeout 4 sh -c 'while [ ! -f {directory}/receiver.port ]; do sleep 0.1; done'")
                port = int(remote(f"cat {directory}/receiver.port").stdout)
                config.update(receiver=f"127.0.0.1:{port}", compression="none")
            assert request(config, method="setConfig", app=identity)["returnValue"] is True
            assert request({}, method="start", app=identity)["returnValue"] is True
            eventually(lambda v: v["receiverConnected"], "worker handshake")
            if args.capture:
                remote(f"timeout 8 sh -c 'while [ ! -f {directory}/receiver.complete ]; do sleep 0.1; done'")
            started = time.monotonic()
            value = request({}, method="stop", app=identity)
            assert value["senderRunning"] is False and value["enabled"] is False, value
            if not args.capture:
                assert 4.5 <= time.monotonic() - started <= 9, "TERM-to-KILL deadline"
            value = request({}, method="cleanup", app=identity)
            assert value["returnValue"] is True and value["config"] == config, value
            assert remote("test ! -e " + directory + "/capture.manifest.json", check=False).returncode == 0
            label = "bounded native frames and worker cleanup" if args.capture else "fake worker handshake and TERM/KILL/reap"
            print("PASS sandbox identity, persistence, " + label, flush=True)

        remote(f"kill -TERM {pid}")
        # Wait for the exact process to exit before requesting a new activation.
        remote(f"timeout 6 sh -c 'while kill -0 {pid} 2>/dev/null; do sleep 0.1; done'")
        value = request({})
        previous = pid
        pid = int(value["pid"])
        assert pid != previous, value
        print("PASS SIGTERM shutdown and Luna reactivation", flush=True)
        memory = remote(f"sed -n '/^VmRSS:/p' /proc/{pid}/status").stdout.decode().strip()
        print("Native probe", memory, flush=True)
        remote(f"timeout 36 sh -c 'while kill -0 {pid} 2>/dev/null; do sleep 0.2; done'", timeout=40)
        print("PASS automatic bounded shutdown", flush=True)
    finally:
        if receiver_pid is not None:
            remote(f"kill -TERM {receiver_pid} 2>/dev/null || true", check=False)
        if subscription is not None:
            subscription.terminate()
            subscription.communicate(timeout=5)
        # Find our exact executable even if the first activation's reply failed.
        remote(f"for process in /proc/[0-9]*/exe; do "
               f"if [ \"$(readlink \"$process\")\" = {shlex.quote(binary)} ]; then "
               "pid=${process#/proc/}; pid=${pid%/exe}; kill -TERM \"$pid\"; fi; done", check=False)
        # Only remove the manifest when it is still exactly the file we installed.
        current = remote(f"cat {MANIFEST}", check=False)
        if current.returncode == 0 and current.stdout == manifest_data:
            remote(f"rm {MANIFEST}; ls-control scan-services")
        elif current.returncode == 0:
            raise RuntimeError("probe manifest changed; leaving files intact for inspection")
        remote(f"rm -f {shlex.quote(directory)}/role.json {shlex.quote(directory)}/client.json "
               f"{shlex.quote(directory)}/api.json {shlex.quote(directory)}/service.service {shlex.quote(binary)} "
               f"{shlex.quote(directory)}/worker {shlex.quote(directory)}/scan-services "
               f"{shlex.quote(directory)}/config.json {shlex.quote(directory)}/capture.manifest.json "
               f"{shlex.quote(directory)}/supervisor.manifest.json "
               f"{shlex.quote(directory)}/boot-hook {shlex.quote(directory)}/capture.role.json "
               f"{shlex.quote(directory)}/receiver.js {shlex.quote(directory)}/receiver.log "
               f"{shlex.quote(directory)}/receiver.port {shlex.quote(directory)}/receiver.complete; "
               f"rmdir {shlex.quote(directory)}")
        print("Removed temporary probe registration and files.", flush=True)


if __name__ == "__main__":
    main()
