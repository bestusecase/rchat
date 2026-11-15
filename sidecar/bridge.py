#!/usr/bin/env python3
"""Reticulum bridge for rchat.

Owns the Reticulum instance and the LXMF router, and speaks newline-delimited
JSON to the Rust daemon over stdio.

Why a separate process: the reference Reticulum stack is Python and carries a
lot of hard-won protocol behaviour (path discovery, announce handling, link
management, propagation). Reimplementing it would be months of work and a
compatibility risk. A supervised child process keeps that stack intact while
the daemon stays native and fast.

Modes
-----
embedded (default)
    A private Reticulum instance in the app's data directory. No dependency on
    the user's existing configuration, no port fights with other apps.

shared
    Attach to the user's running Reticulum instance (requires rnsd and
    `share_instance = Yes`). Set RCHAT_RNS_MODE=shared and optionally
    RCHAT_RNS_CONFIGDIR.

Interfaces are chosen with environment variables so no code changes are needed
to move between LAN, TCP relay and radio:

    RCHAT_RNS_AUTO=1|0               AutoInterface (LAN discovery)
    RCHAT_RNS_TCP_SERVER=PORT        listen for peers (127.0.0.1:PORT)
    RCHAT_RNS_TCP_CLIENT=HOST:PORT   connect to a peer or relay
    RCHAT_RNS_GROUP=name             AutoInterface group id
"""

import json
import os
import sys
import threading
import time
import traceback

import LXMF
import RNS

_print_lock = threading.Lock()


def emit(payload):
    """Serialise one event line. Safe from any thread."""
    try:
        line = json.dumps(payload, separators=(",", ":"), default=str)
    except Exception:
        line = json.dumps({"ev": "error", "message": "bridge could not serialise an event"})
    with _print_lock:
        sys.stdout.write(line + "\n")
        sys.stdout.flush()


def env_flag(name, default):
    raw = os.environ.get(name)
    if raw is None:
        return default
    return raw.strip().lower() in ("1", "yes", "true", "on")


DATA_DIR = os.environ.get("RCHAT_RNS_DIR") or os.path.expanduser(
    "~/.local/share/rchat/rns"
)
DISPLAY_NAME = os.environ.get("RCHAT_DISPLAY_NAME", "rchat")
MODE = os.environ.get("RCHAT_RNS_MODE", "embedded").strip().lower()
CONFIG_DIR = os.environ.get("RCHAT_RNS_CONFIGDIR") or DATA_DIR
TCP_SERVER = os.environ.get("RCHAT_RNS_TCP_SERVER")
TCP_CLIENT = os.environ.get("RCHAT_RNS_TCP_CLIENT")
GROUP = os.environ.get("RCHAT_RNS_GROUP", "reticulum")
AUTO_DEFAULT = "1" if not TCP_SERVER and not TCP_CLIENT else "0"
AUTO = env_flag("RCHAT_RNS_AUTO", AUTO_DEFAULT == "1")

ANNOUNCE_INTERVAL = 300
state = {"ready": False, "address": None}
local_destination = None
router = None


def split_hostport(spec, default_host="127.0.0.1"):
    if not spec:
        return None, None
    spec = spec.strip()
    if ":" in spec:
        host, _, port = spec.rpartition(":")
    else:
        host, port = default_host, spec
    try:
        return host, int(port)
    except ValueError:
        return default_host, None


def write_config(config_dir):
    os.makedirs(config_dir, exist_ok=True)

    # Preserve hand-edited configs. Regenerate only when interface settings are
    # supplied explicitly, or when a reset is requested. This is what lets a
    # user add an RNode or a custom interface and keep it across restarts.
    path = os.path.join(config_dir, "config")
    explicit = any(
        os.environ.get(key)
        for key in (
            "RCHAT_RNS_TCP_SERVER",
            "RCHAT_RNS_TCP_CLIENT",
            "RCHAT_RNS_AUTO",
            "RCHAT_RNS_GROUP",
        )
    )
    if os.path.exists(path) and not explicit and not env_flag("RCHAT_RNS_RESET_CONFIG", False):
        return

    lines = ["[reticulum]", "  enable_transport = No"]
    if MODE == "shared":
        lines += [
            "  share_instance = Yes",
            "  shared_instance_port = 37428",
            "  instance_control_port = 37429",
        ]
    else:
        lines += ["  share_instance = No"]
    lines += [
        "  panic_on_interface_error = No",
        "",
        "[logging]",
        "  loglevel = 3",
        "",
        "[interfaces]",
    ]

    if AUTO:
        lines += [
            "  [[AutoInterface]]",
            "    type = AutoInterface",
            "    interface_enabled = True",
            "    group_id = " + GROUP,
        ]

    if TCP_SERVER:
        host, port = split_hostport(TCP_SERVER, "127.0.0.1")
        if port:
            lines += [
                "  [[TCP Server]]",
                "    type = TCPServerInterface",
                "    interface_enabled = True",
                "    listen_ip = " + host,
                "    listen_port = " + str(port),
            ]

    if TCP_CLIENT:
        host, port = split_hostport(TCP_CLIENT)
        if port:
            lines += [
                "  [[TCP Client]]",
                "    type = TCPClientInterface",
                "    interface_enabled = True",
                "    target_host = " + host,
                "    target_port = " + str(port),
            ]

    with open(os.path.join(config_dir, "config"), "w", encoding="utf-8") as handle:
        handle.write("\n".join(lines) + "\n")

def load_identity(path):
    try:
        if os.path.exists(path):
            return RNS.Identity.from_file(path)
    except Exception as exc:  # corrupt or incompatible file
        emit({"ev": "log", "level": "warn", "message": "identity reload failed: %s" % exc})
    identity = RNS.Identity()
    identity.to_file(path)
    return identity


class AnnounceHandler:
    """Learns peer addresses and display names from LXMF announces."""

    aspect_filter = "lxmf.delivery"

    def received_announce(self, destination_hash, announced_identity, app_data):
        try:
            if isinstance(destination_hash, (bytes, bytearray)):
                address = destination_hash.hex()
            else:
                address = str(destination_hash)
            name = None
            try:
                name = LXMF.display_name_from_app_data(app_data)
            except Exception:
                name = None
            emit({"ev": "announce", "address": address, "name": name})
        except Exception:
            emit(
                {
                    "ev": "error",
                    "message": "announce handling failed: "
                    + traceback.format_exc(limit=2),
                }
            )


def on_delivery(message):
    try:
        content = message.content
        if isinstance(content, (bytes, bytearray)):
            body = bytes(content).decode("utf-8", "replace")
        else:
            body = str(content or "")

        source = message.source_hash
        if isinstance(source, (bytes, bytearray)):
            source = source.hex()
        else:
            source = str(source)

        remote_hash = message.hash
        if isinstance(remote_hash, (bytes, bytearray)):
            remote_hash = remote_hash.hex()
        else:
            remote_hash = str(remote_hash or "")

        timestamp = message.timestamp or time.time()
        emit(
            {
                "ev": "message",
                "id": remote_hash,
                "from": source,
                "body": body,
                "ts": int(float(timestamp) * 1000),
            }
        )
    except Exception:
        emit(
            {
                "ev": "error",
                "message": "delivery callback failed: " + traceback.format_exc(limit=2),
            }
        )


def announce_loop():
    while True:
        time.sleep(ANNOUNCE_INTERVAL)
        try:
            if local_destination is not None:
                router.announce(local_destination.hash)
        except Exception:
            pass


def do_announce():
    try:
        if router is not None and local_destination is not None:
            router.announce(local_destination.hash)
    except Exception:
        emit({"ev": "error", "message": "announce failed: " + traceback.format_exc(limit=2)})


def do_send(command):
    our_id = command.get("id") or ""
    target = (command.get("to") or "").strip().lower()
    body = command.get("body") or ""
    title = command.get("title") or ""

    def receipt(value):
        emit({"ev": "receipt", "id": our_id, "state": value})

    try:
        destination_hash = bytes.fromhex(target)
    except ValueError:
        emit({"ev": "error", "message": "invalid destination address: %s" % target})
        receipt("failed")
        return

    try:
        if not RNS.Transport.has_path(destination_hash):
            RNS.Transport.request_path(destination_hash)
            deadline = time.time() + 12
            while not RNS.Transport.has_path(destination_hash) and time.time() < deadline:
                time.sleep(0.15)

        remote_identity = RNS.Identity.recall(destination_hash)
        if remote_identity is None:
            emit(
                {
                    "ev": "error",
                    "message": "no identity known for %s; wait for an announce or "
                    "add the peer again" % target,
                }
            )
            receipt("failed")
            return

        destination = RNS.Destination(
            remote_identity,
            RNS.Destination.OUT,
            RNS.Destination.SINGLE,
            "lxmf",
            "delivery",
        )
        message = LXMF.LXMessage(
            destination,
            local_destination,
            content=body,
            title=title,
            desired_method=LXMF.LXMessage.DIRECT,
        )

        def delivered(*_args, message_id=our_id):
            emit({"ev": "receipt", "id": message_id, "state": "delivered"})

        def failed(*_args, message_id=our_id):
            emit({"ev": "receipt", "id": message_id, "state": "failed"})

        message.register_delivery_callback(delivered)
        message.register_failed_callback(failed)
        router.handle_outbound(message)
        receipt("sending")
    except Exception:
        emit({"ev": "error", "message": traceback.format_exc(limit=3)})
        receipt("failed")


def handle_command(line):
    try:
        command = json.loads(line)
    except Exception:
        return
    kind = command.get("cmd")
    if kind == "send":
        threading.Thread(target=do_send, args=(command,), daemon=True).start()
    elif kind == "rename":
        new_name = (command.get("name") or "").strip()
        if new_name and local_destination is not None:
            # LXMF reads the display name from the destination at announce time,
            # so mutating it here is enough. Re-registering would be rejected.
            local_destination.display_name = new_name
            global DISPLAY_NAME
            DISPLAY_NAME = new_name
            do_announce()
    elif kind == "announce":
        threading.Thread(target=do_announce, daemon=True).start()
    elif kind == "status":
        emit(
            {
                "ev": "state",
                "state": "ready" if state["ready"] else "starting",
                "address": state["address"],
                "name": DISPLAY_NAME,
            }
        )
    elif kind == "shutdown":
        raise SystemExit(0)


def main():
    emit({"ev": "state", "state": "starting", "name": DISPLAY_NAME})
    os.makedirs(DATA_DIR, exist_ok=True)

    if MODE != "shared":
        write_config(CONFIG_DIR)

    source_notes = []
    if AUTO:
        source_notes.append("auto")
    if TCP_SERVER:
        source_notes.append("tcp-server")
    if TCP_CLIENT:
        source_notes.append("tcp-client")
    emit(
        {
            "ev": "log",
            "level": "info",
            "message": "starting reticulum mode=%s config=%s interfaces=%s"
            % (MODE, CONFIG_DIR, ",".join(source_notes) or "none"),
        }
    )

    RNS.Reticulum(
        configdir=CONFIG_DIR,
        loglevel=3,
        require_shared_instance=(MODE == "shared"),
    )

    identity = load_identity(os.path.join(DATA_DIR, "identity"))
    global router, local_destination
    router = LXMF.LXMRouter(
        identity=identity,
        storagepath=os.path.join(DATA_DIR, "lxmf"),
    )
    local_destination = router.register_delivery_identity(
        identity, display_name=DISPLAY_NAME
    )
    router.register_delivery_callback(on_delivery)
    RNS.Transport.register_announce_handler(AnnounceHandler())

    address = local_destination.hash.hex()
    state["ready"] = True
    state["address"] = address
    emit(
        {
            "ev": "state",
            "state": "ready",
            "address": address,
            "name": DISPLAY_NAME,
        }
    )

    do_announce()
    threading.Thread(target=announce_loop, daemon=True).start()

    for line in sys.stdin:
        line = line.strip()
        if line:
            handle_command(line)


if __name__ == "__main__":
    try:
        main()
    except SystemExit:
        raise
    except Exception:
        emit({"ev": "error", "message": traceback.format_exc(limit=5)})
        emit(
            {
                "ev": "state",
                "state": "offline",
                "detail": "bridge failed to start",
            }
        )
        sys.exit(1)
