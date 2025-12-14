# tests/utils/quic_utils.py

import os
import struct


def create_quic_initial_packet(dcid: bytes, scid: bytes, token: bytes = b"") -> bytes:
    """
    Creates a minimal RFC 9000 compliant QUIC Long Header Initial Packet.
    This allows testing the Vane QUIC parser without a full QUIC stack.
    """
    # Header Form (1) | Fixed Bit (1) | Long Packet Type (00) | Reserved (00) | PN Len (00 -> 1 byte)
    # 0xC0 = 1100 0000
    first_byte = 0xC0

    version = b"\x00\x00\x00\x01"  # QUIC v1

    dcid_len = len(dcid)
    scid_len = len(scid)

    # Token Length (VarInt) - simplified for short tokens
    token_len_byte = len(token)

    # Length (VarInt) - Length of remainder. We'll put a dummy payload.
    # Payload = Packet Number (1 byte) + Crypto Frame (dummy)
    payload = b"\x00" + b"\x00" * 10
    length_val = len(payload)

    # Construct Packet
    packet = struct.pack("B", first_byte)
    packet += version
    packet += struct.pack("B", dcid_len)
    packet += dcid
    packet += struct.pack("B", scid_len)
    packet += scid
    packet += struct.pack("B", token_len_byte)
    packet += token
    packet += struct.pack("B", length_val)  # Simplified VarInt for short length
    packet += payload

    return packet
