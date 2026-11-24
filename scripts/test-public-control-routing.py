"""Cross-platform, in-memory invitation tests. No server or real credentials."""
import argparse
import io
import json
from pathlib import Path
import runpy
import unittest
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


if __name__ == "__main__":
    unittest.main()
