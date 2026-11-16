#!/usr/bin/env python3
"""Reticulum bridge for rchat.

Owns the Reticulum instance and the LXMF router, and speaks newline-delimited
JSON to the Rust daemon.

Why a separate process: the reference Reticulum stack is Python and carries a
lot of hard-won protocol behaviour (path discovery, announce handling, link
management, propagation). Reimplementing it would be months of work and a
compatibility risk. A supervised child process keeps that stack intact while
the daemon stays native and fast.

Transport to the daemon
-----------------------
stdio (default)
    The daemon starts this script as a child and reads stdout.

socket
    The daemon listens on loopback and this module dials in. Used on Android,
    where Chaquopy runs Python in the app process: there are no inherited pipes,
    so ``main(port, data_dir, display_name)`` connects instead. The protocol is
    the same line-delimited JSON in both directions.

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
import socket
import sys
import tempfile
import threading
import time
import traceback

# AutoInterface joins its discovery group by interface index, and Reticulum
# treats a failure here as fatal (RNS.panic() calls os._exit). Chaquopy's
# Python is built without socket.if_nametoindex even though Bionic supports it,
# and without the fcntl module, so borrow the libc call instead. Install this
# before RNS imports socket.
if not hasattr(socket, "if_nametoindex"):

    def _if_nametoindex(ifname):
        import ctypes

        name = str(ifname).encode("utf-8")
        # "libc.so" is the public NDK stub; None falls back to the global
        # symbol table if the stub cannot be opened.
        for library in ("libc.so", None):
            try:
                libc = ctypes.CDLL(library, use_errno=True)
            except OSError:
                continue
            native = getattr(libc, "if_nametoindex", None)
            if native is None:
                continue
            native.argtypes = [ctypes.c_char_p]
            native.restype = ctypes.c_uint
            index = int(native(name))
            if index:
                return index
            error = ctypes.get_errno()
            raise OSError(error, os.strerror(error))
        raise OSError("if_nametoindex is unavailable on this platform")

    socket.if_nametoindex = _if_nametoindex

# RNS installs SIGINT/SIGTERM handlers in Reticulum.__init__. CPython only allows
# that on the main thread, and inside the app the bridge runs on a worker thread
# (the Android main thread cannot block for the bridge's lifetime). Android never
# delivers those signals to the UI process, so ignore the install instead of
# letting it abort startup. On the main thread this is a pass-through.
import signal

_native_signal = signal.signal


def _signal(signum, handler):
    try:
        return _native_signal(signum, handler)
    except ValueError:
        return None


signal.signal = _signal

import LXMF
import RNS

_write_lock = threading.Lock()
# Set when the bridge was given a socket to dial instead of inheriting stdio.
_socket = None


def emit(payload):
    """Serialise one event line. Safe from any thread."""
    try:
        line = json.dumps(payload, separators=(",", ":"), default=str)
    except Exception:
        line = json.dumps({"ev": "error", "message": "bridge could not serialise an event"})
    with _write_lock:
        if _socket is None:
            sys.stdout.write(line + "\n")
            sys.stdout.flush()
            return
        try:
            _socket.sendall((line + "\n").encode("utf-8"))
        except OSError:
            # The engine is gone. Events are best effort; the daemon sees the
            # socket close and reports the transport offline.
            pass


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

ATTACHMENT_MAX_BYTES = 8 * 1024 * 1024
REASON_TOO_LARGE = "Attachment was too large to receive"
REASON_UNREADABLE = "Attachment could not be read"
IMAGE_MAGIC = (
    b"\x89PNG\r\n\x1a\n",
    b"\xff\xd8\xff",
    b"GIF87a",
    b"GIF89a",
)
IMAGE_EXTENSIONS = (".png", ".jpg", ".jpeg", ".gif", ".webp")
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


def open_socket(spec, timeout=10.0):
    """Dial the daemon's bridge listener, retrying while it comes up.

    The daemon binds before starting the interpreter, so the first attempt
    normally wins; the retry covers an app restart racing the engine.
    """
    host, port = split_hostport(str(spec))
    if port is None:
        raise ValueError("invalid bridge socket spec: %r" % (spec,))
    deadline = time.time() + timeout
    while True:
        try:
            sock = socket.create_connection((host, port), timeout=5)
            # Command lines have no upper bound we control, and the daemon only
            # writes complete lines; block until one arrives.
            sock.settimeout(None)
            return sock
        except OSError:
            if time.time() >= deadline:
                raise
            time.sleep(0.15)


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


# ---- attachments -----------------------------------------------------------
#
# Bytes never cross the JSON channel. The engine hands the bridge `{name, path}`
# on send and the bridge stages received payloads under a directory the engine
# collects from. Both directions use the standard LXMF image and
# file-attachments fields, so Sideband peers interoperate in both directions.


class AttachmentReadError(Exception):
    """A file the engine asked to send could not be read."""

    def __init__(self, name):
        super().__init__(name)
        self.name = name


def attachment_max_bytes():
    raw = os.environ.get("RCHAT_ATTACHMENT_MAX_BYTES")
    if raw:
        try:
            value = int(str(raw).strip())
            if value >= 0:
                return value
        except ValueError:
            pass
    return ATTACHMENT_MAX_BYTES


def staging_dir():
    """Derived on every call: ``main`` may override ``DATA_DIR``."""
    return os.path.join(DATA_DIR, "staging")


def _clear_staging():
    """Drop payloads a previous run staged but the engine never collected."""
    directory = staging_dir()
    try:
        names = os.listdir(directory)
    except OSError:
        return
    for name in names:
        try:
            os.remove(os.path.join(directory, name))
        except OSError:
            pass


def _basename(path):
    return os.path.basename(str(path or "").replace("\\", "/"))


def _looks_like_image(name, data):
    if data.startswith(IMAGE_MAGIC) or (data[8:12] == b"WEBP" and data[:4] == b"RIFF"):
        return True
    return str(name).lower().endswith(IMAGE_EXTENSIONS)


def attachment_fields(command):
    """Build the LXMF fields for a send command's `{name, path}` list.

    One image uses the standard image field; anything else (including several
    images) uses the standard file-attachments field. Raises
    :class:`AttachmentReadError` when a path cannot be read, so the caller can
    fail the send instead of quietly dropping the file.
    """
    specs = command.get("attachments") or []
    if not isinstance(specs, (list, tuple)) or not specs:
        return {}

    files = []
    for spec in specs:
        if not isinstance(spec, dict):
            continue
        name = _basename(spec.get("name")) or "attachment"
        path = spec.get("path")
        if not path:
            raise AttachmentReadError(name)
        try:
            with open(path, "rb") as handle:
                data = handle.read()
        except OSError:
            raise AttachmentReadError(name)
        files.append((name, data))

    if not files:
        return {}
    if len(files) == 1 and _looks_like_image(*files[0]):
        return {LXMF.FIELD_IMAGE: [files[0][0], files[0][1]]}
    return {LXMF.FIELD_FILE_ATTACHMENTS: [[name, data] for name, data in files]}


def _decode_name(value):
    if isinstance(value, str):
        return value
    if isinstance(value, (bytes, bytearray)):
        return bytes(value).decode("utf-8", "replace")
    return ""


def _as_pair(value):
    """A `(name, bytes)` tuple, or None if the payload is not one.

    Some clients send a filename as bytes rather than text; both are accepted.
    A payload that is not bytes is refused rather than guessed at.
    """
    if not isinstance(value, (list, tuple)) or len(value) != 2:
        return None
    payload = value[1]
    if isinstance(payload, bytearray):
        payload = bytes(payload)
    if not isinstance(payload, bytes):
        return None
    return _decode_name(value[0]), payload


def decode_attachment_fields(fields):
    """Return `[(name, bytes)]` from LXMF fields, in order. Never raises.

    The image field is read before the file-attachments field, so a peer that
    sets both shows its image first. Malformed entries are skipped so a bad
    payload cannot cost the message its text.
    """
    if not isinstance(fields, dict):
        return []
    entries = []
    try:
        pair = _as_pair(fields.get(LXMF.FIELD_IMAGE))
        if pair is not None:
            entries.append(pair)
    except Exception:
        pass
    try:
        files = fields.get(LXMF.FIELD_FILE_ATTACHMENTS)
        if isinstance(files, (list, tuple)):
            for item in files:
                pair = _as_pair(item)
                if pair is not None:
                    entries.append(pair)
    except Exception:
        pass
    return entries


def stage_attachments(fields):
    """Write received payloads into the staging directory.

    Returns `[{name, ordinal, size[, path][, reason]}]`. An entry with a reason
    has no path: the engine records the row without bytes so the bubble can say
    the attachment did not arrive. Empty when the message carried none.
    """
    entries = decode_attachment_fields(fields)
    if not entries:
        return []

    limit = attachment_max_bytes()
    directory = staging_dir()
    staged = []
    for index, (name, payload) in enumerate(entries):
        item = {
            "name": name or ("attachment-%d" % (index + 1)),
            "ordinal": index,
            "size": len(payload),
        }
        if len(payload) > limit:
            item["reason"] = REASON_TOO_LARGE
            staged.append(item)
            continue
        try:
            os.makedirs(directory, exist_ok=True)
            handle, path = tempfile.mkstemp(prefix="rx-", dir=directory)
            try:
                with os.fdopen(handle, "wb") as stream:
                    stream.write(payload)
            except Exception:
                try:
                    os.remove(path)
                except OSError:
                    pass
                raise
            item["path"] = path
        except OSError:
            item["reason"] = REASON_UNREADABLE
        staged.append(item)
    return staged


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
        payload = {
            "ev": "message",
            "id": remote_hash,
            "from": source,
            "body": body,
            "ts": int(float(timestamp) * 1000),
        }
        try:
            attachments = stage_attachments(getattr(message, "fields", None))
        except Exception:
            attachments = []
        if attachments:
            payload["attachments"] = attachments
        emit(payload)
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
        fields = attachment_fields(command)
    except AttachmentReadError as exc:
        emit(
            {
                "ev": "error",
                "message": "\"%s\" could not be read; attach it again" % exc.name,
            }
        )
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
            fields=fields,
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


def main(socket_spec=None, data_dir=None, display_name=None):
    """Start Reticulum and talk to the daemon until it goes away.

    With no arguments this is the stdio bridge the desktop daemon spawns.
    ``socket_spec`` (a port, or ``host:port``) dials the daemon's loopback
    listener instead. ``data_dir`` and ``display_name`` override the
    environment, which an embedded interpreter (Chaquopy) cannot always set.
    ``data_dir`` is the bridge's own directory — identity, configurations and
    LXMF storage — not the daemon's data directory.
    """
    global DATA_DIR, DISPLAY_NAME, CONFIG_DIR, _socket

    if data_dir:
        DATA_DIR = os.path.abspath(os.path.expanduser(str(data_dir)))
    if display_name:
        DISPLAY_NAME = str(display_name)
    # Re-derive: the module-level value was computed before the override.
    CONFIG_DIR = os.environ.get("RCHAT_RNS_CONFIGDIR") or DATA_DIR

    if socket_spec is not None:
        try:
            _socket = open_socket(socket_spec)
        except (OSError, ValueError) as exc:
            sys.stderr.write("rchat bridge could not reach the engine: %s\n" % exc)
            sys.stderr.flush()
            return 1

    emit({"ev": "state", "state": "starting", "name": DISPLAY_NAME})
    os.makedirs(DATA_DIR, exist_ok=True)
    # Whatever a previous run staged was never collected by the engine.
    _clear_staging()

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

    # Commands arrive from the daemon, one JSON object per line. Over a socket
    # the file object gives us the same line iterator stdio does.
    if _socket is not None:
        commands = _socket.makefile("r", encoding="utf-8", errors="replace")
    else:
        commands = sys.stdin
    for line in commands:
        line = line.strip()
        if line:
            handle_command(line)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
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
