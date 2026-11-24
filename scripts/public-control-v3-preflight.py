#!/usr/bin/env python3
"""Offline v3 invitation metadata preflight, not a service launcher.

Use only your own explicitly selected role ZIPs and roster on the trusted
operator computer. Nothing is started or contacted. Results never print paths,
addresses, fingerprints, tokens or option values. This does not authenticate
credentials, certificates, routes, or actual service lifetimes.
"""
import argparse
import io
import json
from pathlib import Path
import sys
import time
import unittest
import uuid
import zipfile

MIN_SECONDS = 1860
MAX_SECONDS = 2400
ROSTER_BYTES = 16 * 1024
ROLE_BYTES = 32 * 1024
ARCHIVE_BYTES = 64 * 1024


class PreflightError(ValueError):
    pass


def require(condition, reason):
    if not condition:
        raise PreflightError(reason)


def exact_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, "DUPLICATE_METADATA_FIELD")
        result[key] = value
    return result


def data(stream, maximum):
    blob = stream.read(maximum + 1)
    require(0 < len(blob) <= maximum, "INVALID_FILE_SIZE")
    value = json.loads(blob, object_pairs_hook=exact_object)
    require(isinstance(value, dict), "INVALID_METADATA")
    return value


def read_role(archive):
    with zipfile.ZipFile(archive) as zipped:
        members = zipped.infolist()
        require(
            len(members) == 1
            and members[0].filename == "round-01.json"
            and not (members[0].flag_bits & 1)
            and 0 < members[0].file_size <= ROLE_BYTES,
            "INVALID_ROLE_ARCHIVE",
        )
        with zipped.open(members[0]) as role:
            return data(role, ROLE_BYTES)


def check(roster, host, viewer, now, turn_seconds):
    require(type(now) is int and type(turn_seconds) is int, "INVALID_TIME")
    require(roster.get("version") == 3, "ROSTER_NOT_V3")
    rooms = roster.get("rooms")
    require(isinstance(rooms, list) and len(rooms) == 1, "ROSTER_MUST_HAVE_ONE_ROOM")
    room = rooms[0]
    require(isinstance(room, dict), "INVALID_ROOM")
    try:
        identifier = uuid.UUID(room.get("room", ""))
    except (ValueError, AttributeError, TypeError) as exc:
        raise PreflightError("INVALID_ROOM") from exc
    require(identifier.version == 4 and room.get("room") == identifier.hex, "INVALID_ROOM")
    start = room.get("not_before")
    stop = room.get("expires_at")
    require(
        type(start) is int
        and type(stop) is int
        and 0 < start <= now
        and MIN_SECONDS <= stop - now <= MAX_SECONDS
        and MIN_SECONDS <= stop - start <= MAX_SECONDS
        and roster.get("expires_at") == stop,
        "INVITATION_OUTSIDE_V3_WINDOW",
    )
    require(turn_seconds >= stop - now, "DECLARED_TURN_RUNTIME_TOO_SHORT")
    for role, profile in (("host", host), ("viewer", viewer)):
        require(profile.get("version") == 3, "ROLE_NOT_V3")
        require(
            profile.get("purpose") == "public-control"
            and profile.get("role") == role
            and profile.get("room") == str(identifier)
            and profile.get("expires_at") == stop
            and profile.get("turn_expires_at") == stop,
            "ROLE_ROOM_OR_LEASE_MISMATCH",
        )
    require(
        host.get("mode") in ("direct_preferred", "relay_only")
        and all(
            key in host and key in viewer and host[key] == viewer[key]
            for key in ("wss", "turn", "relay_ports", "mode", "ca_sha256")
        ),
        "ROLE_ROUTE_MISMATCH",
    )


def bounded_file(path, limit):
    with path.open("rb") as file:
        blob = file.read(limit + 1)
    require(0 < len(blob) <= limit, "INVALID_FILE_SIZE")
    return io.BytesIO(blob)


def main(argv=None):
    args = argparse.ArgumentParser(description=__doc__)
    args.add_argument("--roster", type=Path, required=True)
    args.add_argument("--host-zip", type=Path, required=True)
    args.add_argument("--viewer-zip", type=Path, required=True)
    args.add_argument("--declared-turn-runtime-seconds", type=int, required=True)
    flags = args.parse_args(argv)
    try:
        check(
            data(bounded_file(flags.roster, ROSTER_BYTES), ROSTER_BYTES),
            read_role(bounded_file(flags.host_zip, ARCHIVE_BYTES)),
            read_role(bounded_file(flags.viewer_zip, ARCHIVE_BYTES)),
            int(time.time()),
            flags.declared_turn_runtime_seconds,
        )
    except PreflightError as exc:
        print(f"V3_PREFLIGHT_REJECTED: {exc}", file=sys.stderr)
        return 1
    except (OSError, ValueError, RuntimeError, NotImplementedError, TypeError, zipfile.BadZipFile):
        print("V3_PREFLIGHT_REJECTED: UNREADABLE_PRIVATE_INPUT", file=sys.stderr)
        return 1
    print("V3_PREFLIGHT_OK: local metadata and declared runtime only; no services checked or started.")
    return 0


class SyntheticTests(unittest.TestCase):
    def fixture(self):
        room = uuid.uuid4()
        common = dict(
            version=3, purpose="public-control", room=str(room), expires_at=3400,
            turn_expires_at=3400, wss="fixture-wss", turn="fixture-turn",
            relay_ports=[49152, 49159], mode="direct_preferred", ca_sha256="fixture-pin",
        )
        return (
            dict(version=3, expires_at=3400, rooms=[dict(room=room.hex, not_before=1000, expires_at=3400)]),
            dict(common, role="host"),
            dict(common, role="viewer"),
        )

    def test_compatible_metadata(self):
        check(*self.fixture(), 1000, 2400)

    def test_legacy_role_and_roster(self):
        for index in (0, 1, 2):
            files = self.fixture()
            files[index]["version"] = 2
            with self.assertRaises(PreflightError):
                check(*files, 1000, 2400)

    def test_expiry_and_short_turn_runtime(self):
        with self.assertRaisesRegex(PreflightError, "TURN_RUNTIME"):
            check(*self.fixture(), 1000, 1800)
        with self.assertRaisesRegex(PreflightError, "WINDOW"):
            check(*self.fixture(), 1541, 2400)

    def test_wrong_role_or_route(self):
        roster, host, viewer = self.fixture()
        with self.assertRaisesRegex(PreflightError, "ROLE_ROOM"):
            check(roster, viewer, host, 1000, 2400)
        viewer["mode"] = "relay_only"
        with self.assertRaisesRegex(PreflightError, "ROLE_ROUTE"):
            check(roster, host, viewer, 1000, 2400)

    def test_rejects_duplicate_metadata_without_exposing_value(self):
        with self.assertRaisesRegex(PreflightError, "^DUPLICATE_METADATA_FIELD$"):
            data(io.BytesIO(b'{"version":2,"version":3}'), ROSTER_BYTES)

    def test_only_generator_archive_shape(self):
        _, host, _ = self.fixture()
        for name, expected in (("round-01.json", True), ("./round-01.json", False)):
            archive = io.BytesIO()
            with zipfile.ZipFile(archive, "w") as zipped:
                zipped.writestr(name, json.dumps(host))
            if expected:
                self.assertEqual(read_role(archive), host)
            else:
                with self.assertRaises(PreflightError):
                    read_role(archive)


if __name__ == "__main__":
    if sys.argv[1:] == ["--self-test"]:
        unittest.main(argv=[sys.argv[0]])
    else:
        raise SystemExit(main())
