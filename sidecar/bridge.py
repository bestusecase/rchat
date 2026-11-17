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

Interfaces
----------
``rchat.db`` owns interface configuration; ``rns/config`` is a generated
artifact. When the engine has taken ownership this process waits briefly for a
``configure`` command before starting Reticulum, and applies further changes
through RNS's interface management API so nothing needs an engine restart.

Environment interfaces are only a seed for a config file that does not exist
yet, and only when the engine is told this is a development run
(``RCHAT_RNS_ENV_SEED=1``). Production always waits for the engine's rows:

    RCHAT_RNS_AUTO=1|0               AutoInterface (LAN discovery, on by default)
    RCHAT_RNS_TCP_SERVER=PORT        listen for peers (127.0.0.1:PORT)
    RCHAT_RNS_TCP_CLIENT=HOST:PORT   connect to a peer or relay
    RCHAT_RNS_GROUP=name             AutoInterface group id

Observability
-------------
A sampling thread emits a ``mesh`` event every couple of seconds, but only when
the mesh fingerprint changed, plus immediately after an announce or an interface
change. The engine caches it and serves smaller, queryable views to clients.

Management requests (``configure``, ``import_config``, ``describe``,
``list_devices``) carry a ``token`` and are answered with a ``bridge_reply``
event carrying the same token.
"""

import collections
import json
import os
import queue
import re
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

# Reticulum treats an interface it cannot create as fatal and calls os._exit
# (RNS.panic) from inside _synthesize_interface, even when
# panic_on_interface_error is off. That would let one bad form submission kill
# the whole bridge. Raising instead lets the caller roll back and report the
# configuration error, which is what the apply-then-persist contract needs.
def _panic_as_error():
    raise RuntimeError("Reticulum refused the interface configuration")


RNS.panic = _panic_as_error

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
# Local discovery is on unless a development script turns it off. Adding a TCP
# interface never disables it: the two are independent paths.
AUTO = env_flag("RCHAT_RNS_AUTO", True)
# The engine owns rns/config: wait for its configure before starting Reticulum.
MANAGED = env_flag("RCHAT_RNS_MANAGED", False)

# Attachments ride inside the LXMF message, so this ceiling is what the bridge
# will stage on receipt. It matches the engine's inbound limit; the engine
# applies the policy, and the bridge only refuses to write megabytes to disk.
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

# RNS reports an interface mode as a bit constant; clients see the config word.
INTERFACE_MODES = {
    0x01: "full",
    0x02: "pointtopoint",
    0x03: "access_point",
    0x04: "roaming",
    0x05: "boundary",
    0x06: "gateway",
    0x07: "internal",
}

ANNOUNCE_INTERVAL = 300
MESH_SAMPLE_INTERVAL = 2.0
MESH_PATH_CAP = 2000
OTHER_SERVICES_LIMIT = 200
CONFIGURE_GRACE_SECONDS = 3.0

state = {"ready": False, "address": None, "started_at": None}
local_destination = None
router = None
reticulum = None

# Interface rows last accepted by the engine.
_applied = {}

# Command lines arrive on a reader thread so the main thread can wait for a
# configure before starting Reticulum without blocking on stdin forever.
_commands = queue.Queue()
_commands_closed = threading.Event()

# Non-LXMF destinations heard recently, in memory only.
_other_services = collections.deque(maxlen=OTHER_SERVICES_LIMIT)
_other_services_lock = threading.Lock()

# Aspects learned from announces, keyed by destination hex. RNS only caches the
# aspects of *local* destinations, so a remote announce's aspect has to be
# matched against its identity; this keeps the answer for the path table.
_learned_aspects = {}
_learned_aspects_lock = threading.Lock()
LEARNED_ASPECTS_LIMIT = 5000
LXMF_ASPECTS = ("lxmf.delivery", "lxmf.propagation")

# When each interface was first seen down, so an outage has an age.
_interface_down_since = {}

_mesh_wake = threading.Event()
_mesh_fingerprint = None
_mesh_thread = None
_announce_thread = None


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


# ---- config rendering and import ------------------------------------------

INTERFACE_TYPES = {
    "AutoInterface",
    "TCPClientInterface",
    "TCPServerInterface",
    "BackboneInterface",
    "BackboneClientInterface",
    "UDPInterface",
    "PipeInterface",
    "I2PInterface",
    "SerialInterface",
    "KISSInterface",
    "AX25KISSInterface",
    "RNodeInterface",
    "RNodeMultiInterface",
    "WeaveInterface",
}

_TOP_SECTION = re.compile(r"^\[([^\[\]].*)\]\s*$")
_SUB_SECTION = re.compile(r"^\s*\[\[(.+)\]\]\s*$")
_KEY_VALUE = re.compile(r"^\s*([A-Za-z0-9_]+)\s*=\s*(.*?)\s*$")
_INT = re.compile(r"^-?\d+$")
_FLOAT = re.compile(r"^-?\d+\.\d+$")
_TRUTHY = {"true", "yes", "on", "1"}
_FALSY = {"false", "no", "off", "0"}


def _coerce(text):
    lowered = text.strip().lower()
    if lowered in _TRUTHY:
        return True
    if lowered in _FALSY:
        return False
    if _INT.match(text.strip()):
        try:
            return int(text.strip())
        except ValueError:
            return text
    if _FLOAT.match(text.strip()):
        try:
            return float(text.strip())
        except ValueError:
            return text
    if len(text) >= 2 and text[0] == text[-1] and text[0] in ("'", '"'):
        return text[1:-1]
    return text


def _render_value(value):
    if isinstance(value, bool):
        return "True" if value else "False"
    return str(value)


def render_config(interfaces, extra):
    """Render rns/config from the managed rows plus preserved sections.

    Flat sections are re-emitted verbatim after the managed content. Unknown
    interface blocks are preserved under ``[interfaces]`` so an RNS release that
    adds a field never loses data.
    """
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

    for row in interfaces:
        name = str(row.get("name") or "").strip()
        kind = str(row.get("kind") or row.get("type") or "").strip()
        if not name or not kind:
            continue
        lines.append("  [[%s]]" % name)
        lines.append("    type = %s" % kind)
        lines.append("    interface_enabled = %s" % (
            "True" if row.get("enabled", True) else "False"
        ))
        config = row.get("config") or {}
        if isinstance(config, dict):
            for key in config:
                if key in ("type", "name", "interface_enabled", "enabled"):
                    continue
                lines.append("    %s = %s" % (key, _render_value(config[key])))
        lines.append("")

    for section, raw in extra:
        if str(section).startswith("interfaces/"):
            block = str(raw).strip("\n")
            if block:
                lines.append(block)
                lines.append("")

    for section, raw in extra:
        if str(section).startswith("interfaces/"):
            continue
        block = str(raw).strip("\n")
        if block:
            lines.append(block)
            lines.append("")

    return "\n".join(lines).rstrip("\n") + "\n"


def read_config(path):
    try:
        with open(path, "r", encoding="utf-8") as handle:
            return handle.read()
    except OSError:
        return None


def write_config_text(config_dir, text):
    """Atomically replace the config, mode 0600, never outside config_dir."""
    os.makedirs(config_dir, exist_ok=True)
    path = os.path.join(config_dir, "config")
    tmp = path + ".rchat-tmp"
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    try:
        payload = text.encode("utf-8")
        written = 0
        while written < len(payload):
            written += os.write(fd, payload[written:])
        os.fsync(fd)
    finally:
        os.close(fd)
    os.replace(tmp, path)


def parse_config_text(text):
    """Split a hand-written config into managed interface rows and raw blocks.

    Line-based on purpose: ConfigObj does not round-trip comments or unknown
    keys, and the preserved blocks must stay byte-identical.
    """
    lines = text.splitlines()
    chunks = []
    current = None
    for line in lines:
        match = _TOP_SECTION.match(line)
        if match:
            current = {"name": match.group(1).strip(), "raw": [line]}
            chunks.append(current)
        elif current is not None:
            current["raw"].append(line)
        # Content before the first section is not meaningful in rns/config.

    interfaces = []
    extra = []
    for chunk in chunks:
        name = chunk["name"]
        raw = "\n".join(chunk["raw"]).rstrip("\n") + "\n"
        if name in ("reticulum", "logging"):
            continue
        if name != "interfaces":
            extra.append((name, raw))
            continue

        block = None
        trailing = []
        for line in chunk["raw"][1:]:
            sub = _SUB_SECTION.match(line)
            if sub:
                if block is not None:
                    _finish_interface_block(block, interfaces, extra)
                block = {"name": sub.group(1).strip(), "raw": [line], "values": []}
                trailing = []
                continue
            if block is None:
                if line.strip():
                    trailing.append(line)
            else:
                block["raw"].append(line)
                match = _KEY_VALUE.match(line)
                if match:
                    block["values"].append((match.group(1), match.group(2)))
        if block is not None:
            _finish_interface_block(block, interfaces, extra)
        if trailing:
            extra.append(("interfaces", "\n".join(trailing).rstrip("\n") + "\n"))

    return interfaces, extra


def _finish_interface_block(block, interfaces, extra):
    values = dict(block["values"])
    kind = values.get("type")
    if kind in INTERFACE_TYPES:
        config = {}
        for key, value in block["values"]:
            if key in ("type", "name", "interface_enabled", "enabled"):
                continue
            config[key] = _coerce(value)
        enabled = str(values.get("interface_enabled", "True")).strip().lower() in _TRUTHY
        interfaces.append(
            {
                "name": block["name"],
                "kind": kind,
                "config": config,
                "enabled": enabled,
                "origin": "imported",
            }
        )
    else:
        raw = "\n".join(block["raw"]).rstrip("\n") + "\n"
        extra.append(("interfaces/" + block["name"], raw))


def seed_interfaces():
    rows = []
    if AUTO:
        rows.append(
            {
                "name": "AutoInterface",
                "kind": "AutoInterface",
                "config": {"group_id": GROUP},
                "enabled": True,
            }
        )
    if TCP_SERVER:
        host, port = split_hostport(TCP_SERVER, "127.0.0.1")
        if port:
            rows.append(
                {
                    "name": "TCP Server",
                    "kind": "TCPServerInterface",
                    "config": {"listen_ip": host, "listen_port": port},
                    "enabled": True,
                }
            )
    if TCP_CLIENT:
        host, port = split_hostport(TCP_CLIENT)
        if port:
            rows.append(
                {
                    "name": "TCP Client",
                    "kind": "TCPClientInterface",
                    "config": {"target_host": host, "target_port": port},
                    "enabled": True,
                }
            )
    return rows


def write_config(config_dir):
    """Seed a config only when there is none (or a reset was requested).

    Once rchat owns the file, the engine renders it through ``configure``.
    """
    os.makedirs(config_dir, exist_ok=True)
    path = os.path.join(config_dir, "config")
    if os.path.exists(path) and not env_flag("RCHAT_RNS_RESET_CONFIG", False):
        return
    write_config_text(config_dir, render_config(seed_interfaces(), []))


# ---- management replies ----------------------------------------------------


def reply(token, ok, data=None, error=None, extra=None):
    payload = {"ev": "bridge_reply", "token": token, "ok": bool(ok)}
    if data is not None:
        payload["data"] = data
    if error is not None:
        payload["error"] = str(error)
    if extra:
        payload.update(extra)
    emit(payload)


def _canonical(row):
    return json.dumps(
        {
            "name": row.get("name"),
            "kind": row.get("kind"),
            "config": row.get("config") or {},
            "enabled": bool(row.get("enabled", True)),
        },
        sort_keys=True,
        default=str,
    )


def _applied_map(interfaces):
    """What is actually live: a disabled row was never attached, so it must
    never be a rollback target either."""
    return {
        str(row.get("name")): _canonical(row)
        for row in interfaces
        if row.get("enabled", True) and row.get("name") and row.get("kind")
    }


def _live_names():
    return {str(interface.name) for interface in RNS.Transport.interfaces}


class _RnsLogCapture:
    """Collect what RNS logs while an interface operation runs.

    RNS reports an interface failure as a boolean and writes the reason to its
    log. Relaying that text verbatim is the difference between "check the
    address" and knowing what the address did wrong.
    """

    def __init__(self):
        self.messages = []
        self._previous = None

    def __enter__(self):
        self._previous = RNS.log
        capture = self

        def listener(message, level=3, *_args, **_kwargs):
            try:
                if int(level) <= RNS.LOG_ERROR:
                    capture.messages.append(str(message))
            except Exception:
                pass
            previous = capture._previous
            if previous is not None:
                return previous(message, level, *_args, **_kwargs)
            return None

        RNS.log = listener
        return self

    def __exit__(self, *_exc):
        if self._previous is not None:
            RNS.log = self._previous
            self._previous = None
        return False

    def reason(self):
        for message in reversed(self.messages):
            text = " ".join(str(message).split())
            if text:
                return text
        return None


class InterfaceFailure(RuntimeError):
    """RNS reports a boolean and logs the cause; name the row and quote it."""

    def __init__(self, action, name, reason=None):
        detail = " %s" % reason if reason else ""
        super().__init__(
            "Reticulum could not %s interface \"%s\".%s Check the address, port "
            "and hardware settings; the previous configuration was restored."
            % (action, name, detail)
        )
        self.name = name
        self.reason = reason


def _rns_failure(action, name, reason=None):
    return InterfaceFailure(action, name, reason)


def apply_configure(interfaces, extra):
    """Render and hot-apply the desired interface set."""
    global _applied
    if reticulum is None:
        # Pre-start: only the file matters; Reticulum reads it next.
        write_config_text(CONFIG_DIR, render_config(interfaces, extra))
        _applied = _applied_map(interfaces)
        return

    if MODE == "shared":
        # The interfaces on the wire belong to the shared instance, and this
        # config file is the client's, so there is nothing here to attach.
        # Keep the file correct for the next start and leave the instance alone.
        write_config_text(CONFIG_DIR, render_config(interfaces, extra))
        _applied = {}
        return

    desired_enabled = {
        str(row.get("name")): row
        for row in interfaces
        if row.get("enabled", True) and row.get("name") and row.get("kind")
    }
    previous_applied = dict(_applied)
    previous_text = read_config(os.path.join(CONFIG_DIR, "config"))

    write_config_text(CONFIG_DIR, render_config(interfaces, extra))
    capture = _RnsLogCapture()
    try:
        with capture:
            # Only interfaces rchat manages take part: a TCP server spawns client
            # interfaces of its own, and detaching those would cut live links.
            for name in sorted(set(previous_applied) - set(desired_enabled)):
                if name in _live_names():
                    result = reticulum.detach_interface(name)
                    if result is False:
                        raise _rns_failure("detach", name, capture.reason())
            for name in desired_enabled:
                if name not in _live_names():
                    result = reticulum.attach_interface(name)
                    if not result:
                        raise _rns_failure("attach", name, capture.reason())
                elif previous_applied.get(name) != _canonical(desired_enabled[name]):
                    result = reticulum.reload_interface(name)
                    if not result:
                        raise _rns_failure("reload", name, capture.reason())
    except Exception:
        _restore(previous_text, previous_applied, desired_enabled)
        raise

    _applied = _applied_map(interfaces)
    _mesh_wake.set()


def _restore(previous_text, previous_applied, desired):
    global _applied
    try:
        if previous_text is not None:
            write_config_text(CONFIG_DIR, previous_text)
        if reticulum is not None:
            live = _live_names()
            target = set(previous_applied)
            for name in sorted((live - target) & set(desired)):
                reticulum.detach_interface(name)
            for name in sorted(target - live):
                reticulum.attach_interface(name)
            for name in sorted(target & live):
                if desired.get(name) and _canonical(desired[name]) != previous_applied.get(name):
                    reticulum.reload_interface(name)
    except Exception:
        emit(
            {
                "ev": "error",
                "message": "rollback failed: " + traceback.format_exc(limit=3),
            }
        )
    _applied = previous_applied


def record_started_config():
    """Remember what the running instance was started from.

    Without this, the first engine `configure` after an import would look like a
    change for every row and reload interfaces that are already correct — which
    drops live peers.
    """
    global _applied
    text = read_config(os.path.join(CONFIG_DIR, "config"))
    if not text:
        return
    try:
        interfaces, _extra = parse_config_text(text)
        _applied = _applied_map(interfaces)
    except Exception:
        _applied = {}


def do_configure(command):
    token = command.get("token")
    interfaces = command.get("interfaces") or []
    extra = command.get("extra") or []
    try:
        apply_configure(interfaces, extra)
        reply(token, True, {"applied": len(interfaces)})
    except Exception as exc:
        name = getattr(exc, "name", None)
        reply(token, False, error=str(exc), extra={"name": name} if name else None)


def do_import(token):
    path = os.path.join(CONFIG_DIR, "config")
    text = read_config(path)
    if text is None:
        reply(token, True, {"interfaces": [], "extra": [], "existed": False})
        return
    try:
        interfaces, extra = parse_config_text(text)
        reply(
            token,
            True,
            {
                "interfaces": interfaces,
                "extra": [[section, raw] for section, raw in extra],
                "existed": True,
            },
        )
    except Exception as exc:
        reply(token, False, error="could not import %s: %s" % (path, exc))


def do_list_devices(token, kind):
    if kind not in ("serial", None, ""):
        reply(token, True, {"devices": [], "reason": "unsupported device kind"})
        return
    try:
        from serial.tools import list_ports

        devices = [
            {"device": port.device, "description": port.description}
            for port in list_ports.comports()
        ]
        reply(token, True, {"devices": devices})
    except Exception:
        reply(
            token,
            True,
            {
                "devices": [],
                "reason": "serial device listing is not available on this platform",
            },
        )


# ---- interface type catalogue ----------------------------------------------


def _field(key, label, widget, **extra):
    field = {"key": key, "label": label, "widget": widget}
    field.update(extra)
    return field


def _common_advanced():
    return [
        _field(
            "interface_mode",
            "Mode",
            "select",
            default="full",
            choices=[
                "full",
                "access_point",
                "pointtopoint",
                "roaming",
                "boundary",
                "gateway",
            ],
            help="How this interface participates in the mesh.",
        ),
        _field("bitrate", "Bitrate (bps)", "int"),
        _field("announce_rate_target", "Announce rate target (s)", "int"),
        _field("ifac_netname", "IFAC network name", "text"),
        _field("ifac_netkey", "IFAC passphrase", "secret"),
        _field("ifac_size", "IFAC size (bytes)", "int", default=16),
    ]


# RNS refuses to detach these (Reticulum._detach_interface), so a managed row
# could never be disabled, edited or deleted: mark it read-only instead.
READ_ONLY_KINDS = {
    "I2PInterface": (
        "Reticulum does not allow detaching an I2P interface, so rchat cannot "
        "change it live. Edit it in the Reticulum config and restart."
    ),
}

# Attached to someone else's instance, the config rchat writes is this client's,
# not the daemon's: the interface sections it manages would be invisible to the
# instance asked to attach them.
SHARED_READ_ONLY_REASON = (
    "Attached to a shared Reticulum instance. Its interfaces belong to that "
    "instance, so manage them in its own config."
)


def interface_catalogue():
    def kind(kind_name, label, summary, group, fields):
        entry = {
            "kind": kind_name,
            "label": label,
            "summary": summary,
            "group": group,
            "fields": fields + _common_advanced(),
        }
        reason = READ_ONLY_KINDS.get(kind_name)
        if reason:
            entry["read_only"] = reason
        return entry

    serial_fields = [
        _field("port", "Serial port", "serial", required=True, common=True,
               help="Pick the device, or paste its path."),
        _field("speed", "Baud rate", "int", default=1200, common=True),
    ]
    kiss_extra = [
        _field("databits", "Data bits", "int", default=8),
        _field(
            "parity",
            "Parity",
            "select",
            default="none",
            choices=["none", "even", "odd"],
        ),
        _field("stopbits", "Stop bits", "int", default=1),
        _field("preamble", "Preamble (ms)", "int", default=150),
        _field("txtail", "TX tail (ms)", "int", default=10),
        _field("persistence", "Persistence", "int", default=64),
        _field("slottime", "Slot time (ms)", "int", default=20),
    ]

    return [
        kind(
            "AutoInterface",
            "AutoInterface",
            "Discover peers on the local network link.",
            "Local",
            [
                _field("interface_enabled", "Enabled", "bool", default=True,
                       common=True,
                       help="Attach this interface when the configuration is applied."),
                _field("group_id", "Group id", "text", default="reticulum",
                       common=True,
                       help="Interfaces with the same group id find each other."),
                _field(
                    "discovery_scope",
                    "Discovery scope",
                    "select",
                    default="link",
                    choices=["link", "admin", "site", "organisation", "global"],
                    common=True,
                    help="How far discovery multicasts travel. \"link\" stays on the local link.",
                ),
                _field("discovery_port", "Discovery port", "port"),
                _field("data_port", "Data port", "port"),
                _field("devices", "Interface devices", "text",
                       placeholder="eth0,wlan0"),
                _field(
                    "multicast_address_type",
                    "Multicast address type",
                    "select",
                    default="temporary",
                    choices=["temporary", "permanent"],
                ),
                _field("configured_bitrate", "Configured bitrate (bps)", "int"),
            ],
        ),
        kind(
            "TCPClientInterface",
            "TCP client",
            "Connect to a peer or relay over TCP.",
            "TCP",
            [
                _field("target_host", "Host", "text", required=True,
                       placeholder="relay.example", common=True,
                       help="Hostname or IP of the relay."),
                _field("target_port", "Port", "port", required=True,
                       default=4242, common=True),
                _field("kiss_framing", "KISS framing", "bool", default=False),
                _field("i2p_tunneled", "Via I2P", "bool", default=False),
                _field("connect_timeout", "Connect timeout (s)", "int"),
            ],
        ),
        kind(
            "TCPServerInterface",
            "TCP server",
            "Listen for peers over TCP.",
            "TCP",
            [
                _field("listen_ip", "Listen address", "text",
                       default="0.0.0.0", common=True,
                       placeholder="0.0.0.0"),
                _field("listen_port", "Port", "port", required=True,
                       default=4242, common=True),
                _field("i2p_tunneled", "Via I2P", "bool", default=False),
            ],
        ),
        kind(
            "BackboneInterface",
            "Backbone",
            "A high-throughput TCP backbone link.",
            "TCP",
            [
                _field("listen_ip", "Listen address", "text",
                       default="0.0.0.0", common=True),
                _field("listen_port", "Port", "port", required=True,
                       default=4242, common=True),
                _field("target_host", "Remote host", "text"),
                _field("target_port", "Remote port", "port"),
                _field("device", "Device", "text"),
            ],
        ),
        kind(
            "BackboneClientInterface",
            "Backbone client",
            "Connect to a backbone node.",
            "TCP",
            [
                _field("target_host", "Host", "text", required=True, common=True),
                _field("target_port", "Port", "port", required=True,
                       default=4242, common=True),
                _field("device", "Device", "text"),
            ],
        ),
        kind(
            "UDPInterface",
            "UDP",
            "A UDP link for local or routed networks.",
            "TCP",
            [
                _field("listen_ip", "Listen address", "text",
                       default="0.0.0.0", common=True),
                _field("listen_port", "Listen port", "port", required=True,
                       default=4242, common=True),
                _field("forward_ip", "Forward address", "text"),
                _field("forward_port", "Forward port", "port"),
            ],
        ),
        kind(
            "PipeInterface",
            "Pipe",
            "Run a command and speak Reticulum over its pipes.",
            "Other",
            [
                _field("command", "Command", "mono", required=True, common=True,
                       placeholder="/usr/bin/my-transport"),
            ],
        ),
        kind(
            "I2PInterface",
            "I2P",
            "Reach peers through an I2P tunnel.",
            "Other",
            [],
        ),
        kind(
            "SerialInterface",
            "Serial",
            "A raw serial link.",
            "Radio",
            serial_fields + [
                _field("databits", "Data bits", "int", default=8),
                _field(
                    "parity",
                    "Parity",
                    "select",
                    default="none",
                    choices=["none", "even", "odd"],
                ),
                _field("stopbits", "Stop bits", "int", default=1),
            ],
        ),
        kind(
            "KISSInterface",
            "KISS",
            "Packet radio over a KISS TNC.",
            "Radio",
            serial_fields + kiss_extra + [
                _field("id_callsign", "ID callsign", "text"),
                _field("id_interval", "ID interval (s)", "int"),
            ],
        ),
        kind(
            "AX25KISSInterface",
            "AX.25 KISS",
            "AX.25 packet radio over a KISS TNC.",
            "Radio",
            [
                _field("callsign", "Callsign", "text", required=True,
                       common=True),
                _field("ssid", "SSID", "int", default=0, common=True),
            ]
            + serial_fields
            + kiss_extra,
        ),
        kind(
            "RNodeInterface",
            "RNode",
            "A LoRa radio managed by RNode firmware.",
            "Radio",
            [
                _field("port", "Serial port", "serial", required=True,
                       common=True),
                _field("frequency", "Frequency", "int", default=868000000,
                       units="MHz", common=True),
                _field("bandwidth", "Bandwidth", "int", default=125000,
                       units="kHz", common=True),
                _field(
                    "spreadingfactor",
                    "Spreading factor",
                    "select",
                    default="8",
                    choices=[str(value) for value in range(5, 13)],
                    common=True,
                ),
                _field(
                    "codingrate",
                    "Coding rate",
                    "select",
                    default="5",
                    choices=["5", "6", "7", "8"],
                    common=True,
                ),
                _field("txpower", "TX power", "int", default=17,
                       units="dBm", common=True),
                _field("airtime_limit_short", "Airtime limit, short (%)",
                       "int", default=33, common=True),
                _field("airtime_limit_long", "Airtime limit, long (%)",
                       "int", default=1, common=True),
                _field("id_callsign", "ID callsign", "text"),
                _field("id_interval", "ID interval (s)", "int"),
            ],
        ),
        kind(
            "RNodeMultiInterface",
            "RNode multi",
            "Several RNode radios on one USB device.",
            "Radio",
            [
                _field("port", "Serial port", "serial", required=True,
                       common=True),
                _field("id_callsign", "ID callsign", "text"),
                _field("id_interval", "ID interval (s)", "int"),
            ],
        ),
        kind(
            "WeaveInterface",
            "Weave",
            "A Weave device over serial.",
            "Radio",
            [
                _field("port", "Serial port", "serial", required=True,
                       common=True),
                _field("configured_bitrate", "Configured bitrate (bps)", "int"),
            ],
        ),
    ]


def describe_payload():
    """The type catalogue plus why the catalogue is unmanageable, if it is."""
    payload = {"types": interface_catalogue()}
    if MODE == "shared":
        payload["read_only"] = SHARED_READ_ONLY_REASON
    return payload


def do_describe(token):
    try:
        reply(token, True, describe_payload())
    except Exception as exc:
        reply(token, False, error=str(exc))


# ---- mesh observability ----------------------------------------------------


def _destination_key(destination_hash):
    if isinstance(destination_hash, (bytes, bytearray)):
        return bytes(destination_hash).hex()
    return str(destination_hash)


def _aspect_for(destination_hash):
    """The aspect of a destination, when RNS or an announce told us.

    ``Transport.destinations_map`` only holds destinations registered locally,
    so remote peers are resolved through what their announces taught us.
    """
    try:
        destination = RNS.Transport.destinations_map.get(destination_hash)
    except Exception:
        destination = None
    if destination is not None:
        name = str(getattr(destination, "name", "") or "")
        parts = name.split(".")
        if len(parts) > 1:
            return ".".join(parts[:-1])
    with _learned_aspects_lock:
        return _learned_aspects.get(_destination_key(destination_hash))


def _learn_aspect(destination_hash, aspect):
    if not aspect:
        return
    key = _destination_key(destination_hash)
    with _learned_aspects_lock:
        _learned_aspects[key] = aspect
        while len(_learned_aspects) > LEARNED_ASPECTS_LIMIT:
            _learned_aspects.pop(next(iter(_learned_aspects)))


def _aspect_from_identity(announced_identity, destination_hash):
    """Match an announced destination against aspects rchat knows by name.

    Reticulum announces carry a destination hash and an identity, not an app
    name, so the aspect is recovered by recomputing the hash. Anything outside
    this list stays unknown rather than guessed; the UI labels it by hash.
    """
    if announced_identity is None:
        return None
    try:
        if isinstance(destination_hash, str):
            destination_hash = bytes.fromhex(destination_hash)
    except ValueError:
        return None
    for full_name in LXMF_ASPECTS:
        parts = full_name.split(".")
        try:
            if RNS.Destination.hash(announced_identity, parts[0], *parts[1:]) == destination_hash:
                return full_name
        except Exception:
            continue
    return None


def _radio_stats(stats):
    radio = {}
    for key in (
        "rssi",
        "snr",
        "frequency",
        "bandwidth",
        "sf",
        "cr",
        "txpower",
        "airtime_short",
        "airtime_long",
    ):
        value = stats.get(key)
        if isinstance(value, (int, float)) and not isinstance(value, bool):
            radio[key] = float(value)
    return radio or None


def _interface_payload(stats):
    hashed = stats.get("hash")
    if isinstance(hashed, (bytes, bytearray)):
        hashed = hashed.hex()
    payload = {
        "name": str(stats.get("short_name") or stats.get("name") or ""),
        "kind": str(stats.get("type") or ""),
        "hash": str(hashed or ""),
        "online": bool(stats.get("status")),
        "rxb": int(stats.get("rxb") or 0),
        "txb": int(stats.get("txb") or 0),
        "rxs": float(stats.get("rxs") or 0.0),
        "txs": float(stats.get("txs") or 0.0),
    }
    bitrate = stats.get("bitrate")
    if isinstance(bitrate, (int, float)) and not isinstance(bitrate, bool):
        payload["bitrate"] = int(bitrate)
    mtu = stats.get("mtu")
    if isinstance(mtu, int) and not isinstance(mtu, bool):
        payload["mtu"] = mtu
    peers = stats.get("peers")
    if isinstance(peers, int) and not isinstance(peers, bool):
        payload["peers"] = peers
    mode = stats.get("mode")
    if mode is not None:
        if isinstance(mode, int) and not isinstance(mode, bool):
            payload["mode"] = INTERFACE_MODES.get(mode, str(mode))
        else:
            payload["mode"] = str(mode)
    ifac = stats.get("ifac_netname")
    if ifac:
        payload["ifac"] = str(ifac)
    radio = _radio_stats(stats)
    if radio:
        payload["radio"] = radio
    return payload


def annotate_interface_health(interfaces, down_since, now_ms):
    """Stamp offline interfaces with when their outage began.

    ``down_since`` is threading state carried between samples. An interface
    that is online loses its stamp; one that is offline keeps the first sample
    that saw it down, so the age is the length of the outage, not the age of
    the process. Returns the state for the next sample.
    """
    updated = dict(down_since)
    for item in interfaces:
        name = item.get("name")
        if not name:
            continue
        if item.get("online"):
            updated.pop(name, None)
            item.pop("down_since", None)
        else:
            item["down_since"] = updated.setdefault(name, now_ms)
    return updated


def _copy_path_table():
    """Copy the path table under its lock; formatting happens outside it."""
    copied = []
    with RNS.Transport.path_table_lock:
        now = time.time()
        for destination, entry in RNS.Transport.path_table.items():
            try:
                timestamp, next_hop, hops, _expires, _blobs, interface = entry[:6]
            except Exception:
                continue
            used = entry[7] if len(entry) > 7 else 0
            copied.append((destination, next_hop, hops, timestamp, used, interface))
    return copied, now


def mesh_snapshot():
    global _interface_down_since
    interfaces = []
    stats = None
    if reticulum is not None:
        try:
            stats = reticulum.get_interface_stats()
        except Exception:
            stats = None
    if stats:
        for entry in stats.get("interfaces") or []:
            try:
                interfaces.append(_interface_payload(entry))
            except Exception:
                continue
    interfaces.sort(key=lambda item: item.get("name") or "")
    _interface_down_since = annotate_interface_health(
        interfaces, _interface_down_since, now_ms()
    )

    bitrates = {        item["name"]: item.get("bitrate")
        for item in interfaces
        if item.get("bitrate") is not None
    }

    copied, now = _copy_path_table()
    online_by_name = {item["name"]: item["online"] for item in interfaces}
    paths = []
    unresponsive = 0
    for destination, next_hop, hops, timestamp, used, interface in copied:
        try:
            refused = RNS.Transport.path_is_unresponsive(destination)
        except Exception:
            refused = False
        name = str(getattr(interface, "name", interface) or "")
        # A path through an interface that is down cannot carry anything right
        # now, even though Reticulum keeps the entry until it expires.
        if online_by_name.get(name) is False:
            refused = True
        if refused:
            unresponsive += 1
        age = now - (used or timestamp or now)
        aspect = _aspect_for(destination)
        entry = {
            "destination": destination.hex()
            if isinstance(destination, (bytes, bytearray))
            else str(destination),
            "hops": int(hops or 0),
            "next_hop": next_hop.hex()
            if isinstance(next_hop, (bytes, bytearray))
            else str(next_hop),
            "interface": name,
            "state": "unresponsive" if refused else "responsive",
            "age": round(float(age), 3),
        }
        if bitrates.get(name) is not None:
            entry["bitrate"] = bitrates[name]
        if aspect:
            entry["aspect"] = aspect
        paths.append(entry)

    path_total = len(paths)
    # Nearest hops first so a capped table never drops the peers we know, and
    # most recently used first within a hop count, matching the engine's order.
    paths.sort(key=lambda item: (item["hops"], item["age"]))
    truncated = path_total > MESH_PATH_CAP
    if truncated:
        paths = paths[:MESH_PATH_CAP]

    counters = {
        "rx_packets": int(getattr(RNS.Transport, "rx_packets", 0) or 0),
        "tx_packets": int(getattr(RNS.Transport, "tx_packets", 0) or 0),
        "rx_pps": float(getattr(RNS.Transport, "rx_pps", 0.0) or 0.0),
        "tx_pps": float(getattr(RNS.Transport, "tx_pps", 0.0) or 0.0),
        "announce_rx": int(getattr(RNS.Transport, "announce_rxc", 0) or 0),
        "announce_tx": int(getattr(RNS.Transport, "announce_txc", 0) or 0),
        "links": int(getattr(RNS.Transport, "link_count", lambda: 0)() or 0),
        "path_count": path_total,
        "path_unresponsive": unresponsive,
    }

    with _other_services_lock:
        other_services = list(_other_services)

    return {
        "transport": "ready" if state["ready"] else "starting",
        "address": state["address"],
        "started_at": state["started_at"],
        "interfaces": interfaces,
        "counters": counters,
        "path_total": path_total,
        "paths_truncated": truncated,
        "paths": paths,
        "other_services": other_services,
    }


def mesh_fingerprint(payload):
    """Cheap identity of the mesh picture, ignoring byte counters."""
    return (
        payload.get("transport"),
        payload.get("address"),
        tuple(
            (
                item.get("name"),
                item.get("online"),
                item.get("bitrate"),
                item.get("peers"),
                item.get("mode"),
                item.get("down_since"),
            )
            for item in payload.get("interfaces") or []
        ),
        (payload.get("counters") or {}).get("path_count"),
        (payload.get("counters") or {}).get("announce_rx"),
        tuple(
            (
                item.get("destination"),
                item.get("hops"),
                item.get("next_hop"),
                item.get("interface"),
                item.get("state"),
            )
            for item in payload.get("paths") or []
        ),
        tuple(
            (item.get("destination"), item.get("aspect"))
            for item in payload.get("other_services") or []
        ),
    )


def emit_mesh(force=False):
    global _mesh_fingerprint
    if not state["ready"]:
        return
    try:
        payload = mesh_snapshot()
    except Exception:
        emit(
            {
                "ev": "error",
                "message": "mesh sampling failed: " + traceback.format_exc(limit=3),
            }
        )
        return
    fingerprint = mesh_fingerprint(payload)
    if not force and fingerprint == _mesh_fingerprint:
        return
    _mesh_fingerprint = fingerprint
    emit({"ev": "mesh", "mesh": payload})


def mesh_loop():
    while state["ready"]:
        emit_mesh()
        _mesh_wake.wait(MESH_SAMPLE_INTERVAL)
        _mesh_wake.clear()


def now_ms():
    """Epoch milliseconds: the unit every timestamp on the wire uses but
    ``started_at`` (see MeshStatus), which is seconds for uptime maths."""
    return int(time.time() * 1000)


def _remember_other_service(destination_hash, aspect, now):
    destination = destination_hash.hex() if isinstance(destination_hash, (bytes, bytearray)) else str(destination_hash)
    with _other_services_lock:
        for index in range(len(_other_services) - 1, -1, -1):
            if _other_services[index]["destination"] == destination:
                _other_services.pop(index)
        entry = {"destination": destination, "last_seen": int(now)}
        if aspect:
            entry["aspect"] = aspect
        _other_services.append(entry)


# ---- announce handling -----------------------------------------------------


def _announce_context(destination_hash):
    """Interface and hop count for an announce, from the path table."""
    interface_name = None
    hops = None
    try:
        with RNS.Transport.path_table_lock:
            entry = RNS.Transport.path_table.get(destination_hash)
            if entry is not None:
                hops = int(entry[2] or 0)
                received = entry[5]
                interface_name = str(getattr(received, "name", received) or "") or None
    except Exception:
        pass
    return interface_name, hops


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
            # This handler is filtered to lxmf.delivery, so an identity match is
            # the normal path and the filter itself is the fallback.
            aspect = _aspect_from_identity(announced_identity, destination_hash)
            if aspect not in LXMF_ASPECTS:
                aspect = self.aspect_filter
            _learn_aspect(destination_hash, aspect)
            interface, hops = _announce_context(destination_hash)
            payload = {"ev": "announce", "address": address, "name": name,
                       "aspect": aspect}
            if interface:
                payload["interface"] = interface
            if hops is not None:
                payload["hops"] = hops
            emit(payload)
            _mesh_wake.set()
        except Exception:
            emit(
                {
                    "ev": "error",
                    "message": "announce handling failed: "
                    + traceback.format_exc(limit=2),
                }
            )


class ServiceAnnounceHandler:
    """Keeps a short in-memory ring of non-LXMF destinations for the graph."""

    aspect_filter = None

    def received_announce(self, destination_hash, announced_identity, app_data):
        try:
            if local_destination is not None:
                try:
                    if bytes(destination_hash) == bytes(local_destination.hash):
                        return
                except Exception:
                    pass
            aspect = _aspect_from_identity(announced_identity, destination_hash)
            if aspect in LXMF_ASPECTS:
                # Still worth remembering: the path table uses the aspect too.
                _learn_aspect(destination_hash, aspect)
                return
            _remember_other_service(destination_hash, aspect, now_ms())
            _mesh_wake.set()
        except Exception:
            pass


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


# ---- message handling ------------------------------------------------------


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
    elif kind == "describe":
        do_describe(command.get("token"))
    elif kind == "import_config":
        do_import(command.get("token"))
    elif kind == "configure":
        do_configure(command)
    elif kind == "list_devices":
        do_list_devices(command.get("token"), command.get("kind"))
    elif kind == "shutdown":
        raise SystemExit(0)


def _read_commands(stream):
    try:
        for line in stream:
            line = line.strip()
            if line:
                _commands.put(line)
    except Exception:
        pass
    _commands_closed.set()


def _next_command(timeout):
    try:
        return _commands.get(timeout=timeout)
    except queue.Empty:
        return None


def _prestart(deferred):
    """Give the engine a chance to render the config before Reticulum starts."""
    if not MANAGED:
        return
    deadline = time.time() + CONFIGURE_GRACE_SECONDS
    while True:
        left = deadline - time.time()
        if left <= 0:
            return
        line = _next_command(left)
        if line is None:
            return
        try:
            command = json.loads(line)
        except Exception:
            continue
        if command.get("cmd") == "configure":
            handle_command(line)
            return
        deferred.append(line)


def main(socket_spec=None, data_dir=None, display_name=None, managed=False):
    """Start Reticulum and talk to the daemon until it goes away.

    With no arguments this is the stdio bridge the desktop daemon spawns.
    ``socket_spec`` (a port, or ``host:port``) dials the daemon's loopback
    listener instead. ``data_dir`` and ``display_name`` override the
    environment, which an embedded interpreter (Chaquopy) cannot always set.
    ``data_dir`` is the bridge's own directory — identity, configurations and
    LXMF storage — not the daemon's data directory. ``managed`` says the engine
    owns the interface rows and will send a configure before Reticulum starts.
    """
    global DATA_DIR, DISPLAY_NAME, CONFIG_DIR, MANAGED, _socket

    if data_dir:
        DATA_DIR = os.path.abspath(os.path.expanduser(str(data_dir)))
    if display_name:
        DISPLAY_NAME = str(display_name)
    if managed:
        MANAGED = True
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

    if MODE != "shared" and not MANAGED:
        write_config(CONFIG_DIR)

    # The command reader starts first so a configure can render the config
    # before Reticulum reads it.
    if _socket is not None:
        commands = _socket.makefile("r", encoding="utf-8", errors="replace")
    else:
        commands = sys.stdin
    threading.Thread(target=_read_commands, args=(commands,), daemon=True).start()

    deferred = []
    _prestart(deferred)

    source_notes = []
    if AUTO:
        source_notes.append("auto")
    if TCP_SERVER:
        source_notes.append("tcp-server")
    if TCP_CLIENT:
        source_notes.append("tcp-client")
    if MANAGED:
        source_notes.append("managed")
    emit(
        {
            "ev": "log",
            "level": "info",
            "message": "starting reticulum mode=%s config=%s interfaces=%s"
            % (MODE, CONFIG_DIR, ",".join(source_notes) or "none"),
        }
    )

    global reticulum
    reticulum = RNS.Reticulum(
        configdir=CONFIG_DIR,
        loglevel=3,
        # RNS logs to stdout by default, which is the JSON event channel here.
        logdest=RNS.LOG_FILE,
        require_shared_instance=(MODE == "shared"),
    )
    record_started_config()

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
    RNS.Transport.register_announce_handler(ServiceAnnounceHandler())

    address = local_destination.hash.hex()
    state["ready"] = True
    state["address"] = address
    state["started_at"] = int(time.time())
    emit(
        {
            "ev": "state",
            "state": "ready",
            "address": address,
            "name": DISPLAY_NAME,
        }
    )

    for line in deferred:
        handle_command(line)
    deferred = []

    do_announce()
    # A peer that dialled in a moment after we announced should still find us
    # without waiting the full interval. Daemon so it cannot hold exit open.
    reminder = threading.Timer(8.0, do_announce)
    reminder.daemon = True
    reminder.start()
    global _mesh_thread, _announce_thread
    _announce_thread = threading.Thread(target=announce_loop, daemon=True)
    _announce_thread.start()
    _mesh_thread = threading.Thread(target=mesh_loop, daemon=True)
    _mesh_thread.start()

    # Commands arrive from the daemon, one JSON object per line, on the reader
    # thread started before the router came up.
    while True:
        line = _next_command(1.0)
        if line is None:
            if _commands_closed.is_set() and _commands.empty():
                break
            continue
        handle_command(line)

    # The engine is gone; stop sampling and announcing so nothing outlives the
    # bridge (the embedded Android interpreter keeps the process alive).
    state["ready"] = False
    _mesh_wake.set()
    for thread in (_mesh_thread, _announce_thread):
        if thread is not None:
            thread.join(timeout=1.0)
    return 0


def load_identity(path):
    try:
        if os.path.exists(path):
            return RNS.Identity.from_file(path)
    except Exception as exc:  # corrupt or incompatible file
        emit({"ev": "log", "level": "warn", "message": "identity reload failed: %s" % exc})
    identity = RNS.Identity()
    identity.to_file(path)
    return identity


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
