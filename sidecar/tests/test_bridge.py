"""Tests for the Reticulum bridge's pure helpers.

Run with: python3 -m unittest discover -s sidecar/tests
"""

import os
import stat
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import bridge  # noqa: E402
import RNS  # noqa: E402

SAMPLE_CONFIG = """# hand written
[reticulum]
  enable_transport = Yes
  share_instance = No

[logging]
  loglevel = 3

[interfaces]
  [[Relay]]
    type = TCPClientInterface
    interface_enabled = True
    target_host = relay.example
    target_port = 4242
    kiss_framing = False
    connect_timeout = 15

  [[My RNode]]
    type = RNodeInterface
    port = /dev/ttyUSB0
    frequency = 868000000
    spreadingfactor = 8

  [[Mystery board]]
    type = NotARealInterface
    port = /dev/ttyAMA0

[transport]
  enable_transport = Yes

[unknown section]
  keep = this exactly
"""


class ParseAndRenderTests(unittest.TestCase):
    def setUp(self):
        bridge.MODE = "embedded"
        self.interfaces, self.extra = bridge.parse_config_text(SAMPLE_CONFIG)

    def test_known_interfaces_become_managed_rows(self):
        names = [row["name"] for row in self.interfaces]
        self.assertEqual(names, ["Relay", "My RNode"])
        relay = self.interfaces[0]
        self.assertEqual(relay["kind"], "TCPClientInterface")
        self.assertEqual(relay["config"]["target_host"], "relay.example")
        self.assertEqual(relay["config"]["target_port"], 4242)
        self.assertIs(relay["config"]["kiss_framing"], False)
        self.assertTrue(relay["enabled"])
        # `type` and `interface_enabled` are row attributes, not config keys.
        self.assertNotIn("type", relay["config"])
        self.assertNotIn("interface_enabled", relay["config"])

    def test_unknown_interface_and_sections_are_preserved(self):
        sections = dict(self.extra)
        self.assertIn("interfaces/Mystery board", sections)
        self.assertIn("[transport]", sections["transport"])
        self.assertIn("[unknown section]", sections["unknown section"])
        self.assertIn(
            "keep = this exactly", sections["unknown section"]
        )

    def test_render_round_trips_managed_rows(self):
        rendered = bridge.render_config(self.interfaces, self.extra)
        rows, extra = bridge.parse_config_text(rendered)
        self.assertEqual(
            [(row["name"], row["kind"]) for row in rows],
            [("Relay", "TCPClientInterface"), ("My RNode", "RNodeInterface")],
        )
        self.assertEqual(rows[0]["config"], self.interfaces[0]["config"])
        self.assertEqual(rows[1]["config"], self.interfaces[1]["config"])
        # Preserved blocks survive another round trip unchanged.
        self.assertEqual(dict(extra)["unknown section"], dict(self.extra)["unknown section"])

    def test_render_order_and_disabled_flag(self):
        rows = [
            {"name": "First", "kind": "TCPServerInterface",
             "config": {"listen_port": 1}, "enabled": True},
            {"name": "Second", "kind": "TCPClientInterface",
             "config": {"target_host": "a", "target_port": 2}, "enabled": False},
        ]
        rendered = bridge.render_config(rows, [])
        self.assertLess(rendered.index("[[First]]"), rendered.index("[[Second]]"))
        self.assertIn("interface_enabled = True", rendered)
        self.assertIn("interface_enabled = False", rendered)
        self.assertTrue(rendered.startswith("[reticulum]"))
        self.assertTrue(rendered.strip().endswith("target_port = 2"))

    def test_empty_and_ragged_input(self):
        self.assertEqual(bridge.parse_config_text(""), ([], []))
        rows, extra = bridge.parse_config_text("[interfaces]\n  stray = 1\n")
        self.assertEqual(rows, [])
        self.assertEqual(extra[0][0], "interfaces")

    def test_typed_values(self):
        self.assertIs(bridge._coerce("True"), True)
        self.assertIs(bridge._coerce("no"), False)
        self.assertEqual(bridge._coerce("42"), 42)
        self.assertEqual(bridge._coerce("1.5"), 1.5)
        self.assertEqual(bridge._coerce("hello"), "hello")
        self.assertEqual(bridge._coerce('"quoted"'), "quoted")


class WriteTests(unittest.TestCase):
    def test_atomic_write_is_private(self):
        with tempfile.TemporaryDirectory() as folder:
            bridge.write_config_text(folder, "hello\n")
            path = os.path.join(folder, "config")
            with open(path, encoding="utf-8") as handle:
                self.assertEqual(handle.read(), "hello\n")
            mode = stat.S_IMODE(os.stat(path).st_mode)
            self.assertEqual(mode, 0o600)
            self.assertFalse(os.path.exists(path + ".rchat-tmp"))


class DefaultNetworkTests(unittest.TestCase):
    @unittest.skipIf(
        "RCHAT_RNS_AUTO" in os.environ,
        "the environment deliberately overrides the default",
    )
    def test_discovery_is_on_without_any_configuration(self):
        self.assertTrue(
            bridge.AUTO,
            "a fresh install must discover peers with no setup at all",
        )

    def test_adding_a_relay_never_turns_discovery_off(self):
        original = (bridge.AUTO, bridge.TCP_CLIENT, bridge.TCP_SERVER)
        try:
            bridge.AUTO = True
            bridge.TCP_CLIENT = "relay.example:4242"
            bridge.TCP_SERVER = None
            kinds = [row["kind"] for row in bridge.seed_interfaces()]
            self.assertIn("AutoInterface", kinds)
            self.assertIn("TCPClientInterface", kinds)
        finally:
            bridge.AUTO, bridge.TCP_CLIENT, bridge.TCP_SERVER = original

    def test_rendered_config_keeps_discovery_next_to_a_relay(self):
        rows = [
            {
                "name": "AutoInterface",
                "kind": "AutoInterface",
                "config": {"group_id": "reticulum"},
                "enabled": True,
            },
            {
                "name": "Relay",
                "kind": "TCPClientInterface",
                "config": {"target_host": "relay.example", "target_port": 4242},
                "enabled": True,
            },
        ]
        rendered = bridge.render_config(rows, [])
        self.assertIn("[[AutoInterface]]", rendered)
        self.assertIn("[[Relay]]", rendered)
        parsed, _extra = bridge.parse_config_text(rendered)
        self.assertEqual(
            [(row["name"], row["enabled"]) for row in parsed],
            [("AutoInterface", True), ("Relay", True)],
        )


class InterfaceHealthTests(unittest.TestCase):
    def test_an_offline_interface_keeps_the_start_of_its_outage(self):
        state = {}
        down = [{"name": "Relay", "online": False}]
        state = bridge.annotate_interface_health(down, state, 1_000)
        self.assertEqual(down[0]["down_since"], 1_000)

        # A later sample keeps the original start, so the age is the outage.
        still_down = [{"name": "Relay", "online": False}]
        state = bridge.annotate_interface_health(still_down, state, 9_000)
        self.assertEqual(still_down[0]["down_since"], 1_000)

        up = [{"name": "Relay", "online": True}]
        state = bridge.annotate_interface_health(up, state, 12_000)
        self.assertNotIn("down_since", up[0])
        self.assertEqual(state, {})

    def test_a_refused_interface_quotes_reticulums_own_reason(self):
        original = bridge.RNS.log
        try:
            with bridge._RnsLogCapture() as capture:
                bridge.RNS.log(
                    "Could not connect to relay.example:4242: Connection refused",
                    bridge.RNS.LOG_ERROR,
                )
                bridge.RNS.log("routine chatter", 4)
            self.assertEqual(
                capture.reason(),
                "Could not connect to relay.example:4242: Connection refused",
            )
        finally:
            bridge.RNS.log = original
        self.assertIs(bridge.RNS.log, original, "the log must be restored")

        failure = bridge.InterfaceFailure(
            "attach", "Relay", "Could not connect: Connection refused"
        )
        text = str(failure)
        self.assertIn("Relay", text)
        self.assertIn("Connection refused", text)
        self.assertEqual(failure.name, "Relay")


class MeshPayloadTests(unittest.TestCase):
    def test_interface_whitelist_drops_extra_stats(self):
        raw = {
            "name": "TCPClientInterface[Relay]",
            "short_name": "Relay",
            "type": "TCPClientInterface",
            "hash": b"\x01\x02",
            "status": True,
            "bitrate": 10_000_000,
            "mtu": 16384,
            "rxb": 10,
            "txb": 20,
            "rxs": 1.5,
            "txs": 2.5,
            "peers": 3,
            "mode": 1,
            "ifac_netname": "net",
            "ifac_signature": b"secret",
            "txbuffered": 9,
            "blocked_ip_list": ["10.0.0.1"],
            "rssi": -88,
            "snr": 7.5,
            "frequency": 868_000_000,
            "bandwidth": 125_000,
            "sf": 8,
            "cr": 5,
            "txpower": 17,
            "airtime_short": 1.2,
            "airtime_long": 0.4,
            "cpu_temp": 41.5,
        }
        payload = bridge._interface_payload(raw)
        self.assertEqual(payload["name"], "Relay")
        self.assertEqual(payload["kind"], "TCPClientInterface")
        self.assertEqual(payload["hash"], "0102")
        self.assertTrue(payload["online"])
        self.assertEqual(payload["ifac"], "net")
        self.assertEqual(
            set(payload["radio"]),
            {"rssi", "snr", "frequency", "bandwidth", "sf", "cr", "txpower",
             "airtime_short", "airtime_long"},
        )
        for leaked in ("ifac_signature", "blocked_ip_list", "txbuffered", "cpu_temp"):
            self.assertNotIn(leaked, payload)
        self.assertNotIn("radio", bridge._interface_payload({"short_name": "X"}))

    def test_fingerprint_ignores_byte_counters_but_sees_structure(self):
        base = {
            "transport": "ready",
            "address": "ab" * 16,
            "interfaces": [
                {"name": "LAN", "online": True, "bitrate": 1, "peers": 2, "mode": "1"}
            ],
            "counters": {"path_count": 1, "announce_rx": 4, "rxb": 10},
            "paths": [
                {"destination": "cd" * 16, "hops": 1, "next_hop": "cd" * 16,
                 "interface": "LAN", "state": "responsive"}
            ],
            "other_services": [],
        }
        first = bridge.mesh_fingerprint(base)
        changed_bytes = {**base, "counters": {**base["counters"], "rxb": 999}}
        self.assertEqual(bridge.mesh_fingerprint(changed_bytes), first)

        changed_path = {
            **base,
            "paths": [{**base["paths"][0], "hops": 2}],
        }
        self.assertNotEqual(bridge.mesh_fingerprint(changed_path), first)

        changed_online = {
            **base,
            "interfaces": [{**base["interfaces"][0], "online": False}],
        }
        self.assertNotEqual(bridge.mesh_fingerprint(changed_online), first)


class CatalogueTests(unittest.TestCase):
    def test_every_type_is_describable(self):
        catalogue = bridge.interface_catalogue()
        kinds = {entry["kind"] for entry in catalogue}
        self.assertEqual(kinds, bridge.INTERFACE_TYPES)
        widgets = {"text", "mono", "secret", "port", "int", "bool", "select", "serial"}
        for entry in catalogue:
            self.assertTrue(entry["label"])
            self.assertTrue(entry["fields"], entry["kind"])
            keys = [field["key"] for field in entry["fields"]]
            self.assertEqual(len(keys), len(set(keys)), entry["kind"])
            for field in entry["fields"]:
                self.assertIn(field["widget"], widgets, field)
                self.assertTrue(field["label"])
                if "choices" in field:
                    self.assertIn(field.get("default"), field["choices"], field)

    def test_tcp_client_defaults_form_a_valid_config(self):
        catalogue = {entry["kind"]: entry for entry in bridge.interface_catalogue()}
        fields = {field["key"]: field for field in catalogue["TCPClientInterface"]["fields"]}
        self.assertTrue(fields["target_host"]["required"])
        self.assertEqual(fields["target_port"]["default"], 4242)
        self.assertEqual(fields["target_port"]["widget"], "port")

    def test_only_types_rns_refuses_are_read_only(self):
        catalogue = {entry["kind"]: entry for entry in bridge.interface_catalogue()}
        # RNS refuses to detach I2P interfaces, so rchat must not offer to.
        self.assertIn("I2P", catalogue["I2PInterface"]["read_only"])
        marked = {kind for kind, entry in catalogue.items() if entry.get("read_only")}
        self.assertEqual(marked, {"I2PInterface"})

    def test_shared_instance_marks_the_catalogue_read_only(self):
        bridge.MODE = "embedded"
        try:
            self.assertNotIn("read_only", bridge.describe_payload())
            bridge.MODE = "shared"
            payload = bridge.describe_payload()
            self.assertEqual(payload["read_only"], bridge.SHARED_READ_ONLY_REASON)
            self.assertEqual(
                {entry["kind"] for entry in payload["types"]}, bridge.INTERFACE_TYPES
            )
        finally:
            bridge.MODE = "embedded"

    def test_shared_configure_writes_the_client_config_only(self):
        with tempfile.TemporaryDirectory() as folder:
            original = (bridge.MODE, bridge.CONFIG_DIR, bridge.reticulum, bridge._applied)
            bridge.MODE = "shared"
            bridge.CONFIG_DIR = folder
            bridge.reticulum = object()  # a live instance that must not be touched
            try:
                bridge.apply_configure(
                    [{"name": "Relay", "kind": "TCPClientInterface",
                      "config": {"target_host": "a", "target_port": 1}, "enabled": True}],
                    [],
                )
            finally:
                bridge.MODE, bridge.CONFIG_DIR, bridge.reticulum, bridge._applied = (
                    original[0], original[1], original[2], original[3]
                )
            text = bridge.read_config(os.path.join(folder, "config"))
            self.assertIn("share_instance = Yes", text)
            self.assertIn("[[Relay]]", text)


class AspectAndServiceTests(unittest.TestCase):
    """Announce aspects RNS cannot answer locally (it only caches our own)."""

    def setUp(self):
        bridge._learned_aspects.clear()
        bridge._other_services.clear()
        self.identity = RNS.Identity()

    def test_lxmf_delivery_is_recovered_from_the_announced_identity(self):
        destination = RNS.Destination.hash(self.identity, "lxmf", "delivery")
        self.assertEqual(
            bridge._aspect_from_identity(self.identity, destination), "lxmf.delivery"
        )
        self.assertEqual(
            bridge._aspect_from_identity(self.identity, destination.hex()),
            "lxmf.delivery",
        )

    def test_an_unknown_service_aspect_is_not_guessed(self):
        destination = RNS.Destination.hash(self.identity, "nomadnetwork", "node")
        self.assertIsNone(bridge._aspect_from_identity(self.identity, destination))

    def test_a_learned_aspect_answers_the_path_table(self):
        key = "ab" * 16
        bridge._learn_aspect(bytes.fromhex(key), "lxmf.propagation")
        self.assertEqual(bridge._aspect_for(bytes.fromhex(key)), "lxmf.propagation")
        self.assertIsNone(bridge._aspect_for(bytes.fromhex("cd" * 16)))

    def test_a_service_without_an_aspect_is_still_remembered(self):
        bridge._remember_other_service(bytes.fromhex("ef" * 16), None, 1234)
        self.assertEqual(len(bridge._other_services), 1)
        self.assertNotIn("aspect", bridge._other_services[0])

    def test_the_service_ring_keeps_non_lxmf_and_skips_lxmf(self):
        handler = bridge.ServiceAnnounceHandler()
        delivery = RNS.Destination.hash(self.identity, "lxmf", "delivery")
        handler.received_announce(delivery, self.identity, None)
        self.assertEqual(len(bridge._other_services), 0)
        node = RNS.Destination.hash(self.identity, "nomadnetwork", "node")
        handler.received_announce(node, self.identity, None)
        self.assertEqual(len(bridge._other_services), 1)
        self.assertEqual(
            bridge._other_services[0]["destination"], bytes(node).hex()
        )
        self.assertEqual(
            bridge._learned_aspects.get(bytes(node).hex()), None
        )


if __name__ == "__main__":
    unittest.main()
