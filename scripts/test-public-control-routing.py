"""Cross-platform, in-memory invitation tests. No server or real credentials."""
import argparse
import io
import json
from pathlib import Path
import runpy
import shlex
import unittest
import uuid
import zipfile

kit = runpy.run_path(str(Path(__file__).with_name("prepare-public-video-test.py")))


class RoutingTests(unittest.TestCase):
    def args(self, mode="relay_only"):
        return argparse.Namespace(
            purpose="public-control", mode=mode, ttl=300, all_ready=True,
            wss="93.184.216.34:8443", turn="93.184.216.34:3478",
            relay_ports=[49152, 49153], ca_der=b"synthetic-public-ca",
            ca_sha256="01" * 32, host_local_ip="192.168.1.2",
            viewer_local_ip="192.168.2.2", host_fingerprint="02" * 32,
            viewer_fingerprint="03" * 32, bind="127.0.0.1:8443",
            host_egress_ip="93.184.216.45", viewer_egress_ip="93.184.216.46",
            tls_cert="/fixture/public.pem", tls_key="/fixture/not-read.pem",
        )

    def test_both_modes_keep_each_round_bound_to_both_roles(self):
        for mode in ("relay_only", "direct_preferred"):
            files, expiry = kit["make_batch"](self.args(mode), b"fixture-only", 1000, "/fixture")
            self.assertEqual(expiry, 2800)
            roles = {}
            for role in ("host", "viewer"):
                with zipfile.ZipFile(io.BytesIO(files[f"{role}-test-rounds.zip"])) as archive:
                    roles[role] = [json.loads(archive.read(name)) for name in archive.namelist()]
                    self.assertEqual(len(roles[role]), 8)
                    for profile in roles[role]:
                        self.assertEqual((profile["role"], profile["mode"], profile["version"]), (role, mode, 2))
                        self.assertEqual(profile["expires_at"], expiry)
                        self.assertEqual(profile["turn_expires_at"], expiry)
            self.assertEqual(len({p["room"] for p in roles["host"]}), 8)
            for host, viewer in zip(roles["host"], roles["viewer"]):
                self.assertEqual(host["room"], viewer["room"])
                self.assertEqual(host["turn_credential"], viewer["turn_credential"])
                self.assertNotEqual(host["relay_token"], viewer["relay_token"])

    def test_legacy_arguments_keep_relay_only(self):
        args = self.args()
        del args.mode
        files, _ = kit["make_kit"](args, b"fixture-only", 1000)
        self.assertEqual(json.loads(files["host.json"])["mode"], "relay_only")

    def test_invalid_route_or_video_purpose_is_rejected(self):
        for args in (self.args("unknown"), self.args("direct_preferred")):
            if args.mode == "direct_preferred":
                args.purpose = "public-video"
            with self.assertRaises(kit["Invalid"]):
                kit["make_kit"](args, b"fixture-only", 1000)

    def sustained_args(self, mode="relay_only", ttl=None):
        args = self.args(mode)
        args.batch = args.all_ready = False
        args.basic_sustained = True
        args.ttl = ttl
        return args

    def test_cli_sustained_mode_has_a_separate_opt_in_and_default(self):
        args = self.sustained_args()
        required = {
            **{key: getattr(args, key) for key in (
                "wss", "bind", "turn", "host_local_ip", "viewer_local_ip", "host_egress_ip",
                "viewer_egress_ip", "host_fingerprint", "viewer_fingerprint", "tls_cert", "tls_key",
            )},
            "out_dir": "/fixture/output", "relay_ports": "49152:49153", "ca": "/fixture/public-ca.der",
        }
        base = [f"--{key.replace('_', '-')}={value}" for key, value in required.items()]
        cli = kit["parser"]()
        self.assertEqual(kit["invitation_lifetime"](cli.parse_args(base)), 300)
        with self.assertRaises(kit["Invalid"]):
            kit["invitation_lifetime"](cli.parse_args(base + ["--basic-sustained"]))
        product = cli.parse_args(base + ["--basic-sustained", "--purpose=public-control"])
        self.assertTrue(product.basic_sustained)
        self.assertEqual(kit["invitation_lifetime"](product), 2400)

    def test_sustained_role_archives_and_roster_share_exact_scope_and_expiry(self):
        for mode in ("direct_preferred", "relay_only"):
            args = self.sustained_args(mode)
            files, expiry = kit["make_basic_sustained"](args, b"fixture-only", 1000, "/fixture")
            self.assertEqual(set(files), set(kit["BASIC_SUSTAINED_FILES"]))
            self.assertEqual(expiry, 3400)
            roster = json.loads(files["batch.json"])
            self.assertEqual(set(roster), {"version", "expires_at", "host_ipv4", "viewer_ipv4", "rooms"})
            self.assertEqual((roster["version"], roster["expires_at"]), (3, expiry))
            self.assertEqual((roster["host_ipv4"], roster["viewer_ipv4"]),
                             (args.host_egress_ip, args.viewer_egress_ip))
            room, = roster["rooms"]
            self.assertEqual(set(room), {"room", "not_before", "expires_at", "host_token", "viewer_token"})
            self.assertEqual((room["not_before"], room["expires_at"]), (1000, expiry))

            legacy = self.args(mode)
            legacy_files, _ = kit["make_kit"](legacy, b"fixture-only", 1000)
            profiles = {}
            for role, other in (("host", "viewer"), ("viewer", "host")):
                with zipfile.ZipFile(io.BytesIO(files[f"{role}-basic-sustained.zip"])) as archive:
                    self.assertEqual(archive.namelist(), ["round-01.json"])
                    raw = archive.read("round-01.json")
                profile = profiles[role] = json.loads(raw)
                self.assertEqual(set(profile), set(json.loads(legacy_files[f"{role}.json"])))
                self.assertEqual((profile["version"], profile["purpose"], profile["role"], profile["mode"]),
                                 (3, "public-control", role, mode))
                room_id = uuid.UUID(profile["room"])
                self.assertEqual((profile["room"], room["room"], room_id.version),
                                 (str(room_id), room_id.hex, 4))
                self.assertEqual((profile["expires_at"], profile["turn_expires_at"]), (expiry, expiry))
                self.assertEqual(profile["turn_username"], f"{expiry}:{room_id}")
                self.assertEqual(profile["relay_token"], room[f"{role}_token"])
                self.assertNotIn(room[f"{other}_token"].encode(), raw)
                self.assertEqual(profile["expected_peer_fingerprint"], getattr(args, f"{other}_fingerprint"))
                self.assertEqual(profile["ca_sha256"], args.ca_sha256)
                self.assertEqual(profile["local_ipv4"], getattr(args, f"{role}_local_ip"))
            common = {"room", "wire_nonce", "session_nonce", "pairing_key", "turn_credential", "ca_der_base64"}
            for key in common:
                self.assertEqual(profiles["host"][key], profiles["viewer"][key])
            self.assertNotEqual(profiles["host"]["relay_token"], profiles["viewer"]["relay_token"])
            self.assertNotIn(profiles["host"]["turn_credential"].encode(), files["batch.json"])
            env = dict(shlex.split(line)[1].split("=", 1)
                       for line in files["wss.env"].decode().splitlines()[1:])
            self.assertEqual(set(env), {
                "REMOTEHUB_V2_MODE", "REMOTEHUB_V2_BIND", "REMOTEHUB_V2_TLS_CERT",
                "REMOTEHUB_V2_TLS_KEY", "REMOTEHUB_V2_PUBLIC_CONTROL_BATCH_FILE",
            })
            self.assertEqual(env["REMOTEHUB_V2_MODE"], "public-control")
            self.assertTrue(env["REMOTEHUB_V2_PUBLIC_CONTROL_BATCH_FILE"].endswith("batch.json"))
            self.assertFalse(any(b"fixture-only" in data for data in files.values()))
            self.assertNotIn(room["host_token"].encode(), files["wss.env"])
            self.assertNotIn(room["viewer_token"].encode(), files["wss.env"])

    def test_sustained_is_explicit_strict_bounded_and_does_not_extend_legacy(self):
        legacy = self.sustained_args(ttl=None)
        legacy.basic_sustained = False
        self.assertEqual(kit["invitation_lifetime"](legacy), 300)
        legacy.ttl = 601
        with self.assertRaises(kit["Invalid"]):
            kit["invitation_lifetime"](legacy)
        for ttl in (1860, 2400):
            args = self.sustained_args(ttl=ttl)
            _, expiry = kit["make_basic_sustained"](args, b"fixture-only", 1000, "/fixture")
            self.assertEqual(expiry, 1000 + ttl)
        for overrides in (
            {"ttl": 1859}, {"ttl": 2401}, {"purpose": "public-video"}, {"batch": True},
            {"all_ready": True}, {"basic_sustained": False}, {"host_fingerprint": "03" * 32},
        ):
            args = self.sustained_args()
            vars(args).update(overrides)
            with self.assertRaises(kit["Invalid"]):
                kit["make_basic_sustained"](args, b"fixture-only", 1000, "/fixture")
        with self.assertRaises(kit["Invalid"]):
            kit["make_basic_sustained"](self.sustained_args(), b"fixture-only", (1 << 64) - 1, "/fixture")
        for legacy_writer, extras in (("make_kit", ()), ("make_batch", ("/fixture",))):
            with self.assertRaises(kit["Invalid"]):
                kit[legacy_writer](self.sustained_args(), b"fixture-only", 1000, *extras)


if __name__ == "__main__":
    unittest.main()
