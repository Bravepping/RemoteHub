"""Offline checks only. Public test-vector data from RFC 5769 section 2.1."""
import unittest
from g6_turn_wire import credential_denied, integrity, packet, parse

VECTOR = bytes.fromhex('000100582112a442b7e7a701bc34d686fa87dfae802200105354554e207465737420636c69656e74002400046e0001ff80290008932ff9b151263b36000600096576746a3a68367659202020000800149aeaa70cbfd8cb56781ef2b5b2d3f249c1b571a280280004e57a3bcf')


class WireTests(unittest.TestCase):
    def test_rfc5769_message_integrity_and_fingerprint(self):
        self.assertTrue(integrity(VECTOR, b'VOkJxbRl1RmTxUk/WvJxBt'))
        self.assertFalse(integrity(VECTOR, b'wrong-key'))
        self.assertEqual(parse(VECTOR)[0], 1)

    def test_builder_integrity_padding_and_oversize(self):
        wire = packet(3, bytes(range(12)), [(6, b'username1')], b'unit-key')
        self.assertTrue(integrity(wire, b'unit-key'))
        self.assertEqual(parse(wire)[2][6][0], b'username1')
        for invalid in (wire[:19], wire[:-1], wire + bytes(4096)):
            with self.assertRaises(ValueError):
                parse(invalid)

    def test_fingerprint_tamper_rejected(self):
        altered = bytearray(VECTOR)
        altered[25] ^= 1
        with self.assertRaises(ValueError):
            parse(bytes(altered))

    def test_realm_rejection_requires_exact_coturn_reason_not_generic_437(self):
        reason = b'Allocation mismatch: wrong credentials: the realm value is incorrect'
        values = {9: (bytes([0, 0, 4, 37]) + reason, 20)}
        self.assertTrue(credential_denied('wrong_realm', 0x0113, values, False))
        self.assertFalse(credential_denied('wrong_realm', 0x0113, values, True))
        self.assertFalse(credential_denied('wrong_password', 0x0113, values, False))
        self.assertFalse(credential_denied('wrong_realm', 0x0113, {9: (bytes([0, 0, 4, 37]) + b'Invalid allocation', 20)}, False))


if __name__ == '__main__':
    unittest.main()
