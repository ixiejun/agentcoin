"""Protocol encoding (against the Rust vectors) and the background sender."""

import json
import os
import socket
import struct
import tempfile
import threading
import time
from pathlib import Path

import pytest

from agentcoin_vllm import client

VECTORS = (
    Path(__file__).resolve().parents[3]
    / "crates/ac-market-proto/tests/vectors/engine_protocol.json"
)


def encode(message: dict) -> bytes:
    kind = message["type"]
    if kind == "hello":
        assert message["version"] == client.VERSION
        mode = {"prove": client.PROVE, "verify": client.VERIFY}[message["mode"]]
        return client.hello(message["hidden_size"], mode)
    if kind == "welcome":
        payload = struct.pack(">BBH", 0x11, message["version"], message["topk"])
        assert client.parse_welcome(payload) == (message["version"], message["topk"])
        return client.frame(payload)
    if kind == "segment":
        phase = {"prefill": client.PREFILL, "decode": client.DECODE}[message["phase"]]
        pairs = [tuple(p) for p in message["candidates"]]
        return client.segment(message["request"], phase, message["len"], pairs)
    assert kind == "finish"
    return client.finish(message["request"])


def test_frames_match_the_rust_vectors():
    vectors = json.loads(VECTORS.read_text())
    assert vectors["version"] == client.VERSION
    for v in vectors["messages"]:
        assert encode(v["message"]).hex() == v["frame"], v["name"]


def test_bad_welcome_and_oversized_frames():
    with pytest.raises(ValueError):
        client.parse_welcome(b"\x10\x01\x00\x80")
    with pytest.raises(ValueError):
        client.frame(b"\x00" * (client.MAX_FRAME + 1))


class FakeProvider:
    """A provider socket that answers Hello with Welcome and records frames."""

    def __init__(self, path: str, version: int = client.VERSION, topk: int = 128, mode: int = client.PROVE):
        self.path, self.version, self.topk, self.mode = path, version, topk, mode
        self.frames: list[bytes] = []
        self.modes: list[int] = []
        self.connections = 0
        self.server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.server.bind(path)
        self.server.listen()
        self.closed = False
        self.live: list[socket.socket] = []
        threading.Thread(target=self._serve, daemon=True).start()

    def _serve(self):
        while not self.closed:
            try:
                conn, _ = self.server.accept()
            except OSError:
                return
            self.connections += 1
            self.live.append(conn)
            threading.Thread(target=self._handle, args=(conn,), daemon=True).start()

    def _handle(self, conn):
        buf = b""
        welcomed = False
        with conn:
            while True:
                try:
                    data = conn.recv(65536)
                except OSError:
                    return
                if not data:
                    return
                buf += data
                while len(buf) >= 4:
                    (n,) = struct.unpack(">I", buf[:4])
                    if len(buf) < 4 + n:
                        break
                    payload, buf = buf[4 : 4 + n], buf[4 + n :]
                    if not welcomed:
                        assert payload[:5] == b"\x10ACTL"
                        self.modes.append(payload[10])
                        # A receiver for the other mode answers with a top-k of 0 and closes.
                        topk = self.topk if payload[10] == self.mode else 0
                        conn.sendall(client.frame(struct.pack(">BBH", 0x11, self.version, topk)))
                        if topk == 0:
                            return
                        welcomed = True
                    else:
                        self.frames.append(payload)

    def close(self):
        self.closed = True
        self.server.close()
        for conn in self.live:
            try:
                conn.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
        os.unlink(self.path)


def wait(cond, seconds=10.0):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        if cond():
            return True
        time.sleep(0.02)
    return False


@pytest.fixture
def sock_path():
    with tempfile.TemporaryDirectory() as d:
        yield os.path.join(d, "toploc.sock")


def test_sends_after_the_handshake(sock_path):
    provider = FakeProvider(sock_path, topk=7)
    sender = client.Sender(sock_path, 896)
    assert wait(sender.connected)
    assert sender.topk == 7
    sender.put(lambda: [client.finish("r1")])
    assert wait(lambda: provider.frames == [b"\x02\x00\x02r1"])
    sender.close()
    provider.close()


# Scenario "提供者不在线": frames are dropped while the provider is away, then the sender
# reconnects.
def test_reconnects_after_the_provider_restarts(sock_path, monkeypatch):
    monkeypatch.setattr(client, "RECONNECT_SECONDS", 0.05)
    sender = client.Sender(sock_path, 896)
    sender.put(lambda: [client.finish("lost")])
    assert sender.dropped == 1
    provider = FakeProvider(sock_path)
    assert wait(sender.connected)
    provider.close()
    # The sender notices the loss on its next write.
    assert wait(lambda: (sender.put(lambda: [client.finish("x")]), not sender.connected())[1])
    provider = FakeProvider(sock_path)
    assert wait(sender.connected)
    sender.put(lambda: [client.finish("back")])
    assert wait(lambda: provider.frames[-1:] == [b"\x02\x00\x04back"])
    sender.close()
    provider.close()


# Scenario "版本不符".
def test_stops_on_a_version_mismatch(sock_path):
    provider = FakeProvider(sock_path, version=9)
    sender = client.Sender(sock_path, 896)
    assert wait(lambda: sender.stopped)
    sender.put(lambda: [client.finish("r")])
    assert not sender.connected() and sender.dropped == 1
    time.sleep(0.2)
    assert provider.frames == [] and provider.connections == 1
    provider.close()


def test_a_full_queue_drops(sock_path, monkeypatch):
    monkeypatch.setattr(client, "QUEUE_MAX", 2)
    provider = FakeProvider(sock_path)
    sender = client.Sender(sock_path, 896)
    assert wait(sender.connected)
    gate = threading.Event()
    sender.put(lambda: (gate.wait(5), [client.finish("a")])[1])
    time.sleep(0.1)  # the worker is now blocked in the first item
    for _ in range(5):
        sender.put(lambda: [client.finish("b")])
    assert sender.dropped == 3
    gate.set()
    assert wait(lambda: len(provider.frames) == 3)
    sender.close()
    provider.close()


# Scenarios "提供者拒绝复核模式" / "复核程序拒绝证明模式", seen from the plugin: the receiver
# answers with a top-k of 0, the plugin logs a mode mismatch and never reconnects.
def test_stops_on_a_mode_mismatch(sock_path, caplog, monkeypatch):
    monkeypatch.setattr(client, "RECONNECT_SECONDS", 0.05)
    provider = FakeProvider(sock_path, mode=client.PROVE)
    with caplog.at_level("ERROR", logger="agentcoin_vllm"):
        sender = client.Sender(sock_path, 896, client.VERIFY)
        assert wait(lambda: sender.stopped)
    assert "mode mismatch" in caplog.text
    time.sleep(0.3)
    assert provider.modes == [client.VERIFY] and provider.connections == 1
    sender.put(lambda: [client.finish("r")])
    assert not sender.connected() and provider.frames == []
    provider.close()


def test_verify_mode_handshake_and_queue(sock_path):
    auditor = FakeProvider(sock_path, mode=client.VERIFY)
    sender = client.Sender(sock_path, 896, client.VERIFY)
    assert wait(sender.connected)
    assert auditor.modes == [client.VERIFY]
    assert sender._queue.maxsize == client.VERIFY_QUEUE_MAX
    sender.put(lambda: [client.finish("v")])
    assert wait(lambda: auditor.frames == [b"\x02\x00\x01v"])
    sender.close()
    auditor.close()
