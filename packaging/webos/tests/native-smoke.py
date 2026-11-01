#!/usr/bin/env python3
"""Read-only Luna probe. Writes only its temp directory and its own volatile manifest."""
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
    args = parser.parse_args()
    ssh = ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=8", args.host]

    def remote(command, data=None, timeout=15, check=True):
        return subprocess.run(ssh + [command], input=data, capture_output=True,
                              timeout=timeout, check=check)

    def write(path, data, mode="600"):
        # noclobber makes an existing path a hard failure, including dangling symlinks.
        remote(f"set -euC; umask 077; cat > {shlex.quote(path)}; chmod {mode} {shlex.quote(path)}", data)

    def request(payload, public=False):
        binary = "luna-send-pub" if public else "luna-send"
        command = f"timeout 6 {binary} -w 3000 -n 1 luna://{SERVICE}/status {shlex.quote(json.dumps(payload))}"
        # This TV's luna-send exits without a reply when stdin reaches EOF.
        with subprocess.Popen(ssh + [command], stdin=subprocess.PIPE,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE) as process:
            try:
                process.wait(timeout=10)
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

    directory = remote("mktemp -d /tmp/prysm-luna-probe.XXXXXX").stdout.decode().strip()
    if not directory.startswith("/tmp/prysm-luna-probe.") or "/" in directory[len("/tmp/"):]:
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
    pid = None
    try:
        write(binary, (ROOT / "target/armv7-unknown-linux-gnueabi/webos/prysm-webos-service").read_bytes(), "700")
        role = {"exeName": binary, "type": "regular", "allowedNames": [SERVICE],
                "permissions": [{"service": SERVICE, "inbound": ["*"],
                                 "outbound": ["com.webos.service.tvpower"]}]}
        write(directory + "/role.json", json.dumps(role).encode())
        # This group can read power state. The role restricts outbound service access.
        write(directory + "/client.json", json.dumps({SERVICE: ["activities.callbacks"]}).encode())
        write(directory + "/api.json", json.dumps({"public": [SERVICE + "/status"]}).encode())
        write(directory + "/service.service",
              f"[D-BUS Service]\nName={SERVICE}\nExec={binary} --run-for 30\nType=dynamic\n".encode())
        write(MANIFEST, manifest_data, "644")
        remote("ls-control scan-services")

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

        subscription = subprocess.Popen(ssh + [f"luna-send -i luna://{SERVICE}/status '{{\"subscribe\":true}}'"],
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        value = eventually(lambda v: v["subscribers"] == 1, "subscription registered")
        subscription.terminate()
        subscription.communicate(timeout=5)
        subscription = None
        eventually(lambda v: v["subscribers"] == 0, "subscription cancelled")
        print("PASS subscription registration and cancellation", flush=True)

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
               f"{shlex.quote(directory)}/api.json {shlex.quote(directory)}/service.service {shlex.quote(binary)}; "
               f"rmdir {shlex.quote(directory)}")
        print("Removed temporary probe registration and files.", flush=True)


if __name__ == "__main__":
    main()
