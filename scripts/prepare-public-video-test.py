#!/usr/bin/env python3
"""Prepare a short-lived, view-only RelayOnly test kit without any network I/O."""

import argparse
import base64
import binascii
import ctypes
import errno
import getpass
import hashlib
import hmac
import ipaddress
import json
import os
from pathlib import Path
import re
import secrets
import shlex
import ssl
import stat
import sys
import time
import uuid
import warnings

MAX_CA_DER = 16 * 1024
MAX_CA_FILE = 24 * 1024
MIN_TTL, MAX_TTL = 180, 600
DIR_FLAGS = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0)
FILES = ("host.json", "viewer.json", "wss.env")
PRIVATE = tuple(map(ipaddress.IPv4Network, ("10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16")))
NONPUBLIC = PRIVATE + tuple(
    map(ipaddress.IPv4Network, (
        "0.0.0.0/8", "100.64.0.0/10", "127.0.0.0/8", "169.254.0.0/16",
        "192.0.0.0/16", "198.18.0.0/15", "198.51.100.0/24", "203.0.113.0/24",
        "224.0.0.0/3",
    ))
)


class Invalid(Exception):
    """A safe error containing no provided values or credentials."""


class ArgumentParser(argparse.ArgumentParser):
    def error(self, _message):
        # argparse echoes unknown arguments verbatim, which could include
        # a secret mistakenly supplied as an argument.
        self.exit(2, "Invalid or missing arguments. See --help.\n")


def parser():
    p = ArgumentParser(
        description=__doc__,
        epilog="Linux only. Prompts for the coturn REST secret without echo. "
        "Keep wss.env on the server and transfer only each PC's own JSON via a private channel. "
        "These files do not start any listener or grant permission to view or capture. "
        "Do not use them beyond the explicitly approved one-room test.",
        allow_abbrev=False,
    )
    for option, help_text in (
        ("out-dir", "New output directory under a trusted local parent (not in Git)."),
        ("wss", "Approved public WSS IPv4:PORT reachable by both PCs."),
        ("bind", "Exact server listener IPv4:PORT (private or public; never wildcard)."),
        ("turn", "Approved TURN UDP IPv4:PORT (same public IP as --wss)."),
        ("relay-ports", "Inclusive TURN UDP allocation MIN:MAX (49152+, at most 256 ports)."),
        ("host-local-ip", "Shared-screen PC's local interface IPv4."),
        ("viewer-local-ip", "Viewing PC's local interface IPv4."),
        ("host-egress-ip", "Host public IPv4 observed by the WSS server, not a proxy header."),
        ("viewer-egress-ip", "Viewer public IPv4 observed by the WSS server."),
        ("host-fingerprint", "Host device's independently verified full lowercase fingerprint."),
        ("viewer-fingerprint", "Viewer device's independently verified full lowercase fingerprint."),
        ("ca", "Trusted single TLS CA certificate in DER or PEM form (public only)."),
        ("tls-cert", "Absolute server-side TLS certificate PEM path (reference only)."),
        ("tls-key", "Absolute server-side TLS private-key PEM path (never read or copied)."),
    ):
        p.add_argument("--" + option, required=True, help=help_text)
    p.add_argument("--turn-config", help="Existing local coturn configuration. "
                   "Read the REST secret locally without echo; do not change the file.")
    p.add_argument("--ca-sha256", help="Require an independently verified lowercase SHA-256 of the CA's DER.")
    p.add_argument("--ttl", type=int, default=300, help="Lifetime in seconds (180–600; default 300).")
    p.add_argument("--self-test", action="store_true", help="Run standalone synthetic tests; use by itself.")
    return p


def ipv4(text, label, *, local=False, bind=False):
    try:
        ip = ipaddress.IPv4Address(text)
    except ValueError:
        raise Invalid(label + " must be a literal IPv4 address.") from None
    private = any(ip in net for net in PRIVATE)
    if not any(ip in net for net in NONPUBLIC):
        return str(ip)
    if private and (local or (bind and ip.packed[-1] not in (0, 255))):
        return str(ip)
    raise Invalid(label + " is outside the approved IPv4 policy.")


def decimal(text, label):
    # A canonical spelling also avoids argparse/path shell surprises.
    if not re.fullmatch(r"(?:0|[1-9][0-9]{0,4})", text):
        raise Invalid(label + " must be an unsigned decimal value.")
    return int(text)


def endpoint(text, label, *, bind=False):
    ip, sep, port_text = text.partition(":")
    if not sep:
        raise Invalid(label + " must be a literal IPv4:PORT.")
    port = decimal(port_text, label)
    if not 1024 <= port <= 65535:
        raise Invalid(label + " port must be 1024–65535.")
    return f"{ipv4(ip, label, bind=bind)}:{port}"


def fingerprint(text, label):
    if not re.fullmatch(r"[0-9a-f]{64}", text) or int(text, 16) == 0:
        raise Invalid(label + " must be a nonzero 64-digit lowercase hex fingerprint.")
    return text


def server_path(text):
    # Paths are server-side references only. No control characters in an env file.
    if (not text.startswith("/") or text.startswith("//") or text.endswith("/")
            or any(ord(c) < 32 or ord(c) == 127 for c in text)
            or any(part in ("", ".", "..") for part in text[1:].split("/"))):
        raise Invalid("TLS file paths must be absolute unambiguous server paths.")
    return text


def read_ca(path):
    flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK
    try:
        with os.fdopen(os.open(path, flags), "rb") as stream:
            if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
                raise Invalid("CA must be a regular public certificate file.")
            content = stream.read(MAX_CA_FILE + 1)
    except OSError:
        raise Invalid("Could not safely read the public CA certificate.") from None
    if not content or len(content) > MAX_CA_FILE:
        raise Invalid("Public CA certificate is empty or too large.")
    match = re.fullmatch(
        rb"-----BEGIN CERTIFICATE-----\s+([A-Za-z0-9+/=\r\n]+)-----END CERTIFICATE-----",
        content.strip(),
    )
    try:
        der = base64.b64decode(b"".join(match[1].splitlines()), validate=True) if match else content
        if not 1 <= len(der) <= MAX_CA_DER or not exact_der_sequence(der):
            raise Invalid("Expected exactly one PEM or DER public CA certificate.")
        # Parse the supplied X.509 bytes in memory, without opening a
        # connection or fetching revocation information.
        trust = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        trust.load_verify_locations(cadata=ssl.DER_cert_to_PEM_cert(der))
        if der not in trust.get_ca_certs(binary_form=True):
            raise Invalid("Expected a single public CA certificate, not a leaf certificate.")
    except (ssl.SSLError, binascii.Error):
        raise Invalid("Expected exactly one PEM or DER public CA certificate.") from None
    return der


def exact_der_sequence(der):
    if len(der) < 2 or der[0] != 0x30:
        return False
    size = der[1]
    if size < 128:
        return len(der) == size + 2
    width = size & 0x7f
    return (width in (1, 2) and len(der) > 2 + width and der[2] != 0
            and (size := int.from_bytes(der[2:2 + width], "big")) >= 128
            and len(der) == size + 2 + width)


def normalize(args):
    args.wss = endpoint(args.wss, "--wss")
    args.turn = endpoint(args.turn, "--turn")
    args.bind = endpoint(args.bind, "--bind", bind=True)
    if args.wss.split(":")[0] != args.turn.split(":")[0]:
        raise Invalid("WSS and TURN must share the same approved public IPv4.")
    low, sep, high = args.relay_ports.partition(":")
    if not sep:
        raise Invalid("--relay-ports must be MIN:MAX.")
    low, high = decimal(low, "--relay-ports"), decimal(high, "--relay-ports")
    if not 49152 <= low <= high <= 65535 or high - low > 255:
        raise Invalid("Relay range must be 49152–65535 and contain at most 256 ports.")
    args.relay_ports = [low, high]
    for role in ("host", "viewer"):
        for kind in ("local", "egress"):
            attr = f"{role}_{kind}_ip"
            setattr(args, attr, ipv4(getattr(args, attr), "--" + attr.replace("_", "-"), local=kind == "local"))
        attr = role + "_fingerprint"
        setattr(args, attr, fingerprint(getattr(args, attr), "--" + attr.replace("_", "-")))
    if args.host_fingerprint == args.viewer_fingerprint:
        raise Invalid("Host and viewer must be two distinct verified devices.")
    if not MIN_TTL <= args.ttl <= MAX_TTL:
        raise Invalid("TTL must be 180–600 seconds.")
    args.tls_cert, args.tls_key = server_path(args.tls_cert), server_path(args.tls_key)
    args.ca_der = read_ca(args.ca)
    digest = hashlib.sha256(args.ca_der).hexdigest()
    if args.ca_sha256 is not None and not hmac.compare_digest(
        fingerprint(args.ca_sha256, "--ca-sha256"), digest
    ):
        raise Invalid("Public CA certificate does not match the approved SHA-256.")
    args.ca_sha256 = digest
    return args


def turn_credential(secret, username):
    # coturn REST requires HMAC-SHA1 and standard padded Base64 (not Base64URL).
    return base64.b64encode(hmac.digest(secret, username.encode("ascii"), "sha1")).decode("ascii")


def read_turn_secret():
    try:
        with warnings.catch_warnings():
            # getpass's echoing stdin fallback must never read the secret.
            warnings.simplefilter("error", getpass.GetPassWarning)
            text = getpass.getpass("coturn REST shared secret (not echoed): ")
    except (getpass.GetPassWarning, EOFError, OSError):
        raise Invalid("An interactive terminal with hidden input is required.") from None
    if not re.fullmatch(r"[A-Za-z0-9+/_=\-]{32,128}", text):
        raise Invalid("TURN shared secret does not match the server issuer's format.")
    return bytearray(text, "ascii")


def read_turn_config(args):
    """Read and verify one existing local coturn config without printing values."""
    safe_error = "Cannot use the current private coturn configuration; verify file permissions and settings."
    try:
        with os.fdopen(os.open(args.turn_config, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK), "rb") as stream:
            info = os.fstat(stream.fileno())
            if (not stat.S_ISREG(info.st_mode)
                    or info.st_uid not in (0, os.geteuid())
                    or info.st_mode & 0o027):
                raise Invalid(safe_error)
            raw = bytearray(stream.read(64 * 1024 + 1))
    except OSError:
        raise Invalid(safe_error) from None
    public_ip, turn_port = args.turn.split(":")
    local_ip = args.bind.split(":")[0]
    expected = {
        b"listening-ip": local_ip.encode("ascii"),
        b"relay-ip": local_ip.encode("ascii"),
        b"external-ip": (public_ip + "/" + local_ip if public_ip != local_ip else public_ip).encode("ascii"),
        b"listening-port": turn_port.encode("ascii"),
        b"min-port": str(args.relay_ports[0]).encode("ascii"),
        b"max-port": str(args.relay_ports[1]).encode("ascii"),
    }
    found = {}
    auth = 0
    seen_secret = False
    try:
        if len(raw) > 64 * 1024:
            raise Invalid(safe_error)
        for line in raw.splitlines():
            line = line.strip()
            if not line or line.startswith(b"#"):
                continue
            if line == b"use-auth-secret":
                auth += 1
            key, separator, value = line.partition(b"=")
            key = bytes(key.strip())
            if separator and key in (b"no-auth", b"use-auth-secret"):
                raise Invalid(safe_error)
            if key == b"no-auth":
                raise Invalid(safe_error)
            if separator and key == b"static-auth-secret":
                if seen_secret:
                    raise Invalid(safe_error)
                seen_secret = True
                found[key] = value.strip()
            elif separator and key in expected:
                if key in found:
                    raise Invalid(safe_error)
                found[key] = value.strip()
        master = found.pop(b"static-auth-secret", b"")
        if auth != 1 or found != expected or not re.fullmatch(rb"[A-Za-z0-9+/_=-]{32,128}", master):
            raise Invalid(safe_error)
        return bytearray(master)
    finally:
        raw[:] = b"\0" * len(raw)


def random_bytes(length, preceding=()):
    while True:
        value = secrets.token_bytes(length)
        if any(value) and value not in preceding:
            return value


def make_kit(args, secret, now):
    room = uuid.uuid4()
    wire_nonce = uuid.uuid4()
    while wire_nonce == room:
        wire_nonce = uuid.uuid4()
    pair = random_bytes(16, (room.bytes, wire_nonce.bytes))
    nonce = random_bytes(32)
    host_token = random_bytes(32, (nonce,))
    viewer_token = random_bytes(32, (nonce, host_token))
    expires = now + args.ttl
    username = f"{expires}:{room}"
    common = {
        "version": 1,
        "room": str(room),
        "expires_at": expires,
        "wss": args.wss,
        "turn": args.turn,
        "relay_ports": args.relay_ports,
        "mode": "relay_only",
        "ca_sha256": args.ca_sha256,
        "ca_der_base64": base64.b64encode(args.ca_der).decode("ascii"),
        "pairing_key": pair.hex(),
        "wire_nonce": str(wire_nonce),
        "session_nonce": nonce.hex(),
        "turn_username": username,
        "turn_credential": turn_credential(secret, username),
        "turn_expires_at": expires,
    }
    host = dict(common, role="host", local_ipv4=args.host_local_ip,
                expected_peer_fingerprint=args.viewer_fingerprint, relay_token=host_token.hex())
    viewer = dict(common, role="viewer", local_ipv4=args.viewer_local_ip,
                  expected_peer_fingerprint=args.host_fingerprint, relay_token=viewer_token.hex())
    env = {
        "REMOTEHUB_V2_MODE": "public-video",
        "REMOTEHUB_V2_BIND": args.bind,
        "REMOTEHUB_V2_TLS_CERT": args.tls_cert,
        "REMOTEHUB_V2_TLS_KEY": args.tls_key,
        "REMOTEHUB_V2_PUBLIC_VIDEO_ROOM": room.hex,
        "REMOTEHUB_V2_PUBLIC_VIDEO_HOST_IP": args.host_egress_ip,
        "REMOTEHUB_V2_PUBLIC_VIDEO_VIEWER_IP": args.viewer_egress_ip,
        "REMOTEHUB_V2_PUBLIC_VIDEO_HOST_TOKEN": host_token.hex(),
        "REMOTEHUB_V2_PUBLIC_VIDEO_VIEWER_TOKEN": viewer_token.hex(),
        "REMOTEHUB_V2_PUBLIC_VIDEO_EXPIRES_AT": str(expires),
    }
    encoded = {
        "host.json": (json.dumps(host, indent=2) + "\n").encode("ascii"),
        "viewer.json": (json.dumps(viewer, indent=2) + "\n").encode("ascii"),
        "wss.env": ("# Private one-room test credentials. Only source your own freshly generated file.\n"
                    + "".join(f"export {name}={shlex.quote(value)}\n" for name, value in env.items())).encode("utf-8"),
    }
    if any(len(encoded[name]) > 32 * 1024 for name in ("host.json", "viewer.json")):
        raise Invalid("Profile would exceed the desktop parser's size limit.")
    return encoded, expires


def open_output_parent(path):
    # Walk with no symlinks and refuse untrusted owners/writable ancestors
    # (a trusted sticky directory such as /tmp may protect its child entry).
    path = Path(path).absolute()
    if len(path.parts) < 2 or path.anchor != "/" or ".." in path.parts:
        raise Invalid("Output must name a new child of a trusted local directory.")
    trusted = (0, os.geteuid())
    fd = os.open("/", DIR_FLAGS)
    try:
        for part in (*path.parts[1:-1], None):
            info = os.fstat(fd)
            if info.st_uid not in trusted or (info.st_mode & 0o022 and not info.st_mode & stat.S_ISVTX):
                raise Invalid("Output parent path cannot be controlled by another user.")
            if part is None:
                break
            child = os.open(part, DIR_FLAGS, dir_fd=fd)
            os.close(fd)
            fd = child
        try:
            os.stat(path.name, dir_fd=fd, follow_symlinks=False)
        except FileNotFoundError:
            return fd, path.name
        raise Invalid("Output already exists; nothing was overwritten.")
    except BaseException:
        os.close(fd)
        raise


def rename_noreplace():
    # Linux's atomic no-replace rename prevents a race from overwriting even
    # an empty existing directory. Never fall back to clobber-capable rename.
    try:
        rename = ctypes.CDLL(None, use_errno=True).renameat2
    except AttributeError:
        raise Invalid("Linux libc with renameat2 is required for atomic private output.") from None
    rename.argtypes = (ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint)
    rename.restype = ctypes.c_int

    def move(parent, old, new):
        if rename(parent, os.fsencode(old), parent, os.fsencode(new), 1):
            error = ctypes.get_errno()
            if error == errno.EEXIST:
                raise Invalid("Output already exists; nothing was overwritten.")
            raise OSError(error, "atomic publication failed")
    return move


def write_private(directory, name, data):
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW
    with os.fdopen(os.open(name, flags, 0o600, dir_fd=directory), "wb") as stream:
        os.fchmod(stream.fileno(), 0o600)
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())


def publish(parent, name, files, move):
    stage = ".remotehub-test-stage-" + secrets.token_hex(16)
    old_umask = os.umask(0o077)
    try:
        os.mkdir(stage, 0o700, dir_fd=parent)
    finally:
        os.umask(old_umask)
    directory = None
    try:
        directory = os.open(stage, DIR_FLAGS, dir_fd=parent)
        for filename in FILES:
            write_private(directory, filename, files[filename])
        os.fsync(directory)
        move(parent, stage, name)
    finally:
        try:
            # If rename already succeeded, the original entry no longer
            # exists, including if interrupted immediately after the move.
            os.stat(stage, dir_fd=parent, follow_symlinks=False)
        except FileNotFoundError:
            pass
        else:
            if directory is not None:
                for filename in FILES:
                    try:
                        os.unlink(filename, dir_fd=directory)
                    except FileNotFoundError:
                        pass
            os.rmdir(stage, dir_fd=parent)
        finally:
            if directory is not None:
                os.close(directory)


def main(argv):
    if sys.platform != "linux":
        print("Linux is required for private, no-replace output.", file=sys.stderr)
        return 2
    if argv == ["--self-test"]:
        return self_test()
    try:
        args = normalize(parser().parse_args(argv))
        move = rename_noreplace()
        parent, name = open_output_parent(args.out_dir)
        try:
            secret = read_turn_config(args) if args.turn_config else read_turn_secret()
            try:
                files, expires = make_kit(args, secret, int(time.time()))
            finally:
                secret[:] = b"\0" * len(secret)
            publish(parent, name, files, move)
        finally:
            os.close(parent)
    except KeyboardInterrupt:
        print("Cancelled.", file=sys.stderr)
        return 2
    except Invalid as error:
        print(error, file=sys.stderr)
        return 2
    except (OSError, ValueError):
        print("Could not prepare a complete private kit. Check the output path and filesystem.", file=sys.stderr)
        return 2
    print("Created host.json, viewer.json and wss.env (no services started).")
    print(f"CA SHA-256: {args.ca_sha256}")
    print(f"Expires at Unix UTC: {expires}; desktop startup requires 60 seconds remaining.")
    return 0


def self_test():
    import tempfile
    import unittest
    from unittest.mock import patch

    # Public fixture from the Rust desktop parser. No private key exists here.
    ca_der = base64.b64decode(
        "MIIBpzCCAU2gAwIBAgIUZF+y9UzgIF1CLzMjQUhR/B5D5S4wCgYIKoZIzj0EAwIwKTEnMCUG"
        "A1UEAwweUmVtb3RlSHViLXByb2ZpbGUtZml4dHVyZS1vbmx5MB4XDTI2MTEyMzEwMDA1OFoX"
        "DTI3MTEyMzEwMDA1OFowKTEnMCUGA1UEAwweUmVtb3RlSHViLXByb2ZpbGUtZml4dHVyZS1v"
        "bmx5MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEi3gv4JryBxxuCU9DDEICKUJafSQ3O/uK"
        "z8CrHciFnMOVSPS9rupgHZeyMogWSOEQ4HSz43yO2dHac0HcIZ99+KNTMFEwHQYDVR0OBBYE"
        "FHNCmZBsyU07uAny4QluUTX070w9MB8GA1UdIwQYMBaAFHNCmZBsyU07uAny4QluUTX070w9"
        "MA8GA1UdEwEB/wQFMAMBAf8wCgYIKoZIzj0EAwIDSAAwRQIhAIdgwr/NL8t/fpSawiLUKOuZ"
        "vS4inrClYYEb2KOsYrGsAiAsUCOv4eUoKHwcYOdR63r5PHPcKi+QOyRgWt6qkLfoLw=="
    )
    fake_secret = b"0123456789abcdef0123456789abcdef"

    class Fixtures(unittest.TestCase):
        def setUp(self):
            temporary = tempfile.TemporaryDirectory(dir="/tmp")
            self.addCleanup(temporary.cleanup)
            self.root = Path(temporary.name)
            self.ca = self.root / "public-ca.der"
            self.ca.write_bytes(ca_der)

        def args(self, **overrides):
            args = {
                "out-dir": str(self.root / "kit"),
                "wss": "93.184.216.34:8443",
                "bind": "172.30.1.23:8443",
                "turn": "93.184.216.34:3478",
                "relay-ports": "49152:49407",
                "host-local-ip": "192.168.1.12",
                "viewer-local-ip": "10.0.0.11",
                "host-egress-ip": "93.184.216.45",
                "viewer-egress-ip": "93.184.216.45",
                "host-fingerprint": "01" * 32,
                "viewer-fingerprint": "02" * 32,
                "ca": str(self.ca),
                "tls-cert": "/test/certificate.pem",
                "tls-key": "/test/private 'key' = $(never-execute).pem",
                "ttl": "300",
            }
            args.update(overrides)
            return parser().parse_args([f"--{key}={value}" for key, value in args.items()])

        def test_rust_coturn_known_vector(self):
            username = "1300:00000000-0000-4000-8000-000000000001"
            self.assertEqual(turn_credential(fake_secret, username), "0ldqFwNKnG3DNOQTbQ9ED2WAfr8=")

        def test_profiles_and_wss_env_are_role_correct_and_private(self):
            args = normalize(self.args())
            files, expires = make_kit(args, fake_secret, 1_000)
            host, viewer = (json.loads(files[role + ".json"]) for role in ("host", "viewer"))
            env = dict(shlex.split(line)[1].split("=", 1)
                       for line in files["wss.env"].decode().splitlines()[1:])
            self.assertEqual(expires, 1_300)
            self.assertEqual(host["mode"], "relay_only")
            self.assertEqual(viewer["turn_username"], f"1300:{host['room']}")
            self.assertEqual(host["turn_expires_at"], 1_300)
            self.assertEqual(len(base64.b64decode(host["turn_credential"])), 20)
            self.assertEqual(host["turn_credential"], viewer["turn_credential"])
            self.assertEqual(host["expected_peer_fingerprint"], args.viewer_fingerprint)
            self.assertEqual(viewer["expected_peer_fingerprint"], args.host_fingerprint)
            self.assertEqual(env["REMOTEHUB_V2_MODE"], "public-video")
            self.assertEqual(env["REMOTEHUB_V2_TLS_KEY"], args.tls_key)
            self.assertEqual(env["REMOTEHUB_V2_PUBLIC_VIDEO_HOST_IP"], args.host_egress_ip)
            self.assertEqual(env["REMOTEHUB_V2_PUBLIC_VIDEO_VIEWER_IP"], args.viewer_egress_ip)
            self.assertEqual(env["REMOTEHUB_V2_PUBLIC_VIDEO_EXPIRES_AT"], "1300")
            self.assertEqual(env["REMOTEHUB_V2_PUBLIC_VIDEO_HOST_TOKEN"], host["relay_token"])
            self.assertEqual(env["REMOTEHUB_V2_PUBLIC_VIDEO_VIEWER_TOKEN"], viewer["relay_token"])
            self.assertEqual(env["REMOTEHUB_V2_PUBLIC_VIDEO_ROOM"], uuid.UUID(host["room"]).hex)
            self.assertRegex(env["REMOTEHUB_V2_PUBLIC_VIDEO_ROOM"], r"^[0-9a-f]{32}$")
            self.assertNotEqual(host["relay_token"], viewer["relay_token"])
            self.assertNotIn(viewer["relay_token"].encode(), files["host.json"])
            self.assertNotIn(host["relay_token"].encode(), files["viewer.json"])
            self.assertNotIn(host["turn_credential"].encode(), files["wss.env"])
            self.assertFalse(any(fake_secret in data for data in files.values()))
            parent, name = open_output_parent(args.out_dir)
            try:
                publish(parent, name, files, rename_noreplace())
            finally:
                os.close(parent)
            self.assertEqual(stat.S_IMODE(os.stat(args.out_dir).st_mode), 0o700)
            for filename, data in files.items():
                output = Path(args.out_dir) / filename
                self.assertEqual(stat.S_IMODE(output.stat().st_mode), 0o600)
                self.assertEqual(output.read_bytes(), data)

        def test_failed_publication_leaves_no_partial_output_and_does_not_replace_race(self):
            args = normalize(self.args())
            files, _ = make_kit(args, fake_secret, 1_000)
            parent, name = open_output_parent(args.out_dir)
            try:
                with patch(f"{__name__}.write_private", side_effect=[None, OSError("fixture")]):
                    with self.assertRaises(OSError):
                        publish(parent, name, files, rename_noreplace())
                self.assertEqual(list(self.root.iterdir()), [self.ca])
                # Destination created after the caller's initial existence check.
                destination = Path(args.out_dir)
                destination.mkdir()
                with self.assertRaises(Invalid):
                    publish(parent, name, files, rename_noreplace())
                self.assertEqual(list(destination.iterdir()), [])
                self.assertCountEqual(self.root.iterdir(), [self.ca, destination])
            finally:
                os.close(parent)

        def test_rejects_unapproved_endpoints_and_credentials(self):
            for bad in [
                {"wss": "localhost:8443"}, {"wss": "192.168.1.10:8443"},
                {"turn": "93.184.216.88:3478"}, {"turn": "93.184.216.34:53"},
                {"bind": "0.0.0.0:8443"}, {"bind": "10.1.1.255:8443"},
                {"viewer-egress-ip": "100.64.1.1"}, {"host-egress-ip": "198.51.100.33"},
                {"viewer-local-ip": "127.0.0.1"}, {"ttl": "179"}, {"ttl": "601"},
                {"relay-ports": "49151:49152"}, {"relay-ports": "49152:49408"},
                {"viewer-fingerprint": "01" * 32}, {"viewer-fingerprint": "AB" * 32},
                {"ca-sha256": "ff" * 32}, {"tls-key": "private.pem"}, {"tls-key": "/key\nx"},
            ]:
                with self.subTest(field=next(iter(bad))), self.assertRaises(Invalid):
                    normalize(self.args(**bad))

        def test_pem_and_der_identical_and_rejects_private_or_multiple_pem(self):
            self.assertEqual(read_ca(self.ca), ca_der)
            pem = ssl.DER_cert_to_PEM_cert(ca_der).encode()
            self.ca.write_bytes(pem)
            self.assertEqual(read_ca(self.ca), ca_der)
            for malformed in (pem * 2, ca_der + ca_der, b"-----BEGIN PRIVATE KEY-----\nAA==\n"
                              b"-----END PRIVATE KEY-----\n"):
                self.ca.write_bytes(malformed)
                with self.assertRaises(Invalid):
                    read_ca(self.ca)

        def test_read_private_turn_config_without_exposing_secret(self):
            from types import SimpleNamespace
            p = self.ca.parent / "turnserver.conf"
            settings = (b"listening-ip=10.1.2.3\nrelay-ip=10.1.2.3\n"
                        b"external-ip=93.184.216.34/10.1.2.3\nlistening-port=3478\n"
                        b"min-port=49160\nmax-port=49259\nuse-auth-secret\n")
            secret = b"z" * 40
            good = settings + b"static-auth-secret=" + secret + b"\n"
            p.write_bytes(good)
            p.chmod(0o640)
            args = SimpleNamespace(turn="93.184.216.34:3478", bind="10.1.2.3:8443",
                                   relay_ports=[49160, 49259], turn_config=p)
            self.assertEqual(read_turn_config(args), secret)
            for bad in (good + b"no-auth\n", good + b"static-auth-secret=duplicate\n",
                        good.replace(b"49259", b"49260")):
                p.write_bytes(bad)
                with self.assertRaises(Invalid):
                    read_turn_config(args)
            p.write_bytes(good)
            p.chmod(0o644)
            with self.assertRaises(Invalid):
                read_turn_config(args)

        def test_getpass_fallback_fails_without_echoed_input(self):
            with patch.object(getpass, "getpass", wraps=getpass.fallback_getpass), self.assertRaises(Invalid):
                read_turn_secret()

    result = unittest.TextTestRunner(verbosity=2).run(unittest.defaultTestLoader.loadTestsFromTestCase(Fixtures))
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
