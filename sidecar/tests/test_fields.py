"""The LXMF field shapes, and the bridge's decoder, with no peer in sight.

A field shape LXMF refuses to serialise is the one mistake no Rust test can
reach. This packs a real message carrying each attachment field and unpacks it
again, then feeds malformed payloads through the bridge's own decoder.

Run with: python3 -m unittest discover -s sidecar/tests
"""

import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import LXMF  # noqa: E402
import RNS  # noqa: E402

import bridge  # noqa: E402


PNG = b"\x89PNG\r\n\x1a\n" + bytes(range(64))
PDF = b"%PDF-1.4\n" + bytes(range(48))


class FieldShapeTests(unittest.TestCase):
    """Pack and unpack, because that is what a peer actually exchanges."""

    @classmethod
    def setUpClass(cls):
        # Reticulum refuses to initialise twice, and LXMessage needs a live
        # instance to sign. An explicit config keeps it off the network: no
        # interfaces, no shared instance, so nothing can bind or dial.
        cls.config = tempfile.TemporaryDirectory()
        with open(os.path.join(cls.config.name, "config"), "w", encoding="utf-8") as handle:
            handle.write(
                "[reticulum]\n"
                "  enable_transport = No\n"
                "  share_instance = No\n"
                "\n[logging]\n"
                "  loglevel = 0\n"
            )
        cls.reticulum = RNS.Reticulum(
            configdir=cls.config.name,
            loglevel=0,
            logdest=RNS.LOG_FILE,
        )

    @classmethod
    def tearDownClass(cls):
        cls.config.cleanup()

    def roundtrip(self, fields):
        """Pack a message with these fields and unpack it as a peer would."""
        destination = RNS.Destination(
            RNS.Identity(), RNS.Destination.OUT, RNS.Destination.SINGLE,
            "lxmf", "delivery",
        )
        source = RNS.Destination(
            RNS.Identity(), RNS.Destination.IN, RNS.Destination.SINGLE,
            "lxmf", "delivery",
        )
        message = LXMF.LXMessage(
            destination,
            source,
            content="caption",
            title="",
            fields=fields,
            desired_method=LXMF.LXMessage.DIRECT,
        )
        message.pack()
        unpacked = LXMF.LXMessage.unpack_from_bytes(message.packed)
        self.assertEqual(unpacked.content_as_string(), "caption")
        return unpacked.fields

    def test_the_image_field_keeps_its_name_and_bytes(self):
        back = self.roundtrip({LXMF.FIELD_IMAGE: ["photo.png", PNG]})
        name, payload = back[LXMF.FIELD_IMAGE]
        self.assertEqual(name, "photo.png")
        self.assertEqual(payload, PNG)

    def test_the_file_field_keeps_every_entry_in_order(self):
        entries = [["first.pdf", PDF], ["second.bin", b"\x00\x01\x02"]]
        back = self.roundtrip({LXMF.FIELD_FILE_ATTACHMENTS: entries})
        self.assertEqual(
            [(name, bytes(payload)) for name, payload in back[LXMF.FIELD_FILE_ATTACHMENTS]],
            [("first.pdf", PDF), ("second.bin", b"\x00\x01\x02")],
        )

    def test_both_fields_survive_together_and_decode_in_order(self):
        fields = {
            LXMF.FIELD_IMAGE: ["shot.png", PNG],
            LXMF.FIELD_FILE_ATTACHMENTS: [["doc.pdf", PDF]],
        }
        back = self.roundtrip(fields)
        decoded = bridge.decode_attachment_fields(back)
        self.assertEqual(decoded, [("shot.png", PNG), ("doc.pdf", PDF)])

    def test_a_plain_message_carries_no_attachments(self):
        back = self.roundtrip({})
        self.assertEqual(bridge.decode_attachment_fields(back), [])


class DecoderTests(unittest.TestCase):
    """Bad payloads are skipped, never raised: text delivery must survive."""

    def test_a_filename_sent_as_bytes_is_accepted(self):
        decoded = bridge.decode_attachment_fields(
            {LXMF.FIELD_IMAGE: [b"shot.png", PNG]}
        )
        self.assertEqual(decoded, [("shot.png", PNG)])

    def test_payloads_of_the_wrong_shape_are_skipped(self):
        shapes = [
            None,
            "not a dict",
            {LXMF.FIELD_IMAGE: "not a tuple"},
            {LXMF.FIELD_IMAGE: ["name-only"]},
            {LXMF.FIELD_IMAGE: ["name", "a str is not bytes"]},
            {LXMF.FIELD_IMAGE: ["name", PNG, "extra"]},
            {LXMF.FIELD_FILE_ATTACHMENTS: "not a list"},
            {LXMF.FIELD_FILE_ATTACHMENTS: [42, None, ["ok.pdf", PDF]]},
        ]
        self.assertEqual(bridge.decode_attachment_fields(shapes[0]), [])
        self.assertEqual(bridge.decode_attachment_fields(shapes[1]), [])
        for shape in shapes[2:7]:
            self.assertEqual(bridge.decode_attachment_fields(shape), [], shape)
        # The one well-formed entry among the junk still arrives.
        self.assertEqual(
            bridge.decode_attachment_fields(shapes[7]),
            [("ok.pdf", PDF)],
        )

    def test_names_that_are_not_text_still_carry_their_bytes(self):
        decoded = bridge.decode_attachment_fields(
            {LXMF.FIELD_FILE_ATTACHMENTS: [[1234, PDF]]}
        )
        self.assertEqual(decoded, [("", PDF)])


class StagingTests(unittest.TestCase):
    def setUp(self):
        self.original = bridge.DATA_DIR
        self.folder = tempfile.TemporaryDirectory()
        bridge.DATA_DIR = self.folder.name

    def tearDown(self):
        bridge.DATA_DIR = self.original
        self.folder.cleanup()

    def test_a_staged_payload_is_on_disk_under_the_data_directory(self):
        staged = bridge.stage_attachments({LXMF.FIELD_IMAGE: ["shot.png", PNG]})
        self.assertEqual(len(staged), 1)
        item = staged[0]
        self.assertEqual(item["name"], "shot.png")
        self.assertEqual(item["ordinal"], 0)
        self.assertEqual(item["size"], len(PNG))
        self.assertNotIn("reason", item)
        self.assertTrue(
            os.path.abspath(item["path"]).startswith(
                os.path.abspath(os.path.join(self.folder.name, "staging"))
            )
        )
        with open(item["path"], "rb") as handle:
            self.assertEqual(handle.read(), PNG)
        os.remove(item["path"])

    def test_an_oversized_payload_is_reported_without_being_written(self):
        os.environ["RCHAT_ATTACHMENT_MAX_BYTES"] = "16"
        try:
            staged = bridge.stage_attachments(
                {LXMF.FIELD_FILE_ATTACHMENTS: [["big.bin", PDF + PDF]]}
            )
        finally:
            del os.environ["RCHAT_ATTACHMENT_MAX_BYTES"]
        self.assertEqual(len(staged), 1)
        self.assertEqual(staged[0]["reason"], bridge.REASON_TOO_LARGE)
        self.assertNotIn("path", staged[0])
        self.assertEqual(staged[0]["size"], len(PDF + PDF))
        staging = os.path.join(self.folder.name, "staging")
        self.assertFalse(
            os.path.isdir(staging) and os.listdir(staging),
            "an oversized payload must not touch the disk",
        )

    def test_startup_drops_payloads_the_engine_never_collected(self):
        staging = os.path.join(self.folder.name, "staging")
        os.makedirs(staging)
        leftover = os.path.join(staging, "rx-leftover")
        with open(leftover, "wb") as handle:
            handle.write(b"orphan")
        bridge._clear_staging()
        self.assertFalse(os.path.exists(leftover))


class SendFieldsTests(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.original = bridge.DATA_DIR
        bridge.DATA_DIR = self.folder.name

    def tearDown(self):
        bridge.DATA_DIR = self.original
        self.folder.cleanup()

    def write(self, name, data):
        path = os.path.join(self.folder.name, name)
        with open(path, "wb") as handle:
            handle.write(data)
        return path

    def test_one_image_uses_the_image_field(self):
        path = self.write("photo.png", PNG)
        fields = bridge.attachment_fields(
            {"attachments": [{"name": "photo.png", "path": path}]}
        )
        self.assertEqual(list(fields), [LXMF.FIELD_IMAGE])
        self.assertEqual(fields[LXMF.FIELD_IMAGE], ["photo.png", PNG])

    def test_a_file_uses_the_file_field(self):
        path = self.write("doc.pdf", PDF)
        fields = bridge.attachment_fields(
            {"attachments": [{"name": "doc.pdf", "path": path}]}
        )
        self.assertEqual(list(fields), [LXMF.FIELD_FILE_ATTACHMENTS])
        self.assertEqual(fields[LXMF.FIELD_FILE_ATTACHMENTS], [["doc.pdf", PDF]])

    def test_several_files_keep_their_order(self):
        first = self.write("one.png", PNG)
        second = self.write("two.bin", b"xyz")
        fields = bridge.attachment_fields(
            {
                "attachments": [
                    {"name": "one.png", "path": first},
                    {"name": "two.bin", "path": second},
                ]
            }
        )
        # A burst is not an image field, even if its first file is a photo.
        self.assertEqual(list(fields), [LXMF.FIELD_FILE_ATTACHMENTS])
        self.assertEqual(
            fields[LXMF.FIELD_FILE_ATTACHMENTS],
            [["one.png", PNG], ["two.bin", b"xyz"]],
        )

    def test_no_attachments_means_no_fields(self):
        self.assertEqual(bridge.attachment_fields({}), {})
        self.assertEqual(bridge.attachment_fields({"attachments": []}), {})

    def test_an_unreadable_path_is_reported_rather_than_dropped(self):
        with self.assertRaises(bridge.AttachmentReadError) as caught:
            bridge.attachment_fields(
                {
                    "attachments": [
                        {"name": "gone.pdf", "path": "/nonexistent/gone.pdf"}
                    ]
                }
            )
        self.assertEqual(caught.exception.name, "gone.pdf")


if __name__ == "__main__":
    unittest.main()
