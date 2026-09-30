"""The local protocol to the provider agent and the background sender.

Layout (big-endian; crates/ac-market-proto/src/engine.rs is the reference):
frame = u32 length + payload;
Hello = 0x10 "ACTL" version:u8 hidden:u32; Welcome = 0x11 version:u8 topk:u16;
Segment = 0x01 id(u16 len + UTF-8) phase:u8 len:u32 count:u16 count x (index:u32 bits:u16);
Finish = 0x02 id(u16 len + UTF-8).
"""

from __future__ import annotations

import logging
import queue
import socket
import struct
import threading
import time

VERSION = 1
MAGIC = b"ACTL"
PREFILL, DECODE = 0, 1
MAX_FRAME = 1 << 20
QUEUE_MAX = 4096
RECONNECT_SECONDS = 1.0

log = logging.getLogger("agentcoin_vllm")


def frame(payload: bytes) -> bytes:
    if len(payload) > MAX_FRAME:
        raise ValueError("frame too large")
    return struct.pack(">I", len(payload)) + payload


def _id(request: str) -> bytes:
    raw = request.encode("utf-8")
    return struct.pack(">H", len(raw)) + raw


def hello(hidden_size: int) -> bytes:
    return frame(b"\x10" + MAGIC + struct.pack(">BI", VERSION, hidden_size))


def segment(request: str, phase: int, length: int, pairs) -> bytes:
    """`pairs`: (index, bf16 bits) for each candidate."""
    body = [b"\x01", _id(request), struct.pack(">BIH", phase, length, len(pairs))]
    body.extend(struct.pack(">IH", i, b) for i, b in pairs)
    return frame(b"".join(body))


def finish(request: str) -> bytes:
    return frame(b"\x02" + _id(request))


def parse_welcome(payload: bytes) -> tuple[int, int]:
    """(version, topk) of a Welcome payload."""
    if len(payload) != 4 or payload[0] != 0x11:
        raise ValueError("not a Welcome message")
    _, version, topk = struct.unpack(">BBH", payload)
    return version, topk


def _read_exact(sock: socket.socket, n: int) -> bytes:
    out = b""
    while len(out) < n:
        chunk = sock.recv(n - len(out))
        if not chunk:
            raise ConnectionError("provider closed the connection")
        out += chunk
    return out


class Sender:
    """Sends in a background thread; the inference thread only enqueues.

    Items are dropped (and counted) while the provider is unreachable or the queue is full, so
    inference is never slowed down. A provider speaking another protocol version stops the
    sender for good.
    """

    def __init__(self, path: str, hidden_size: int) -> None:
        self.path = path
        self.hidden_size = hidden_size
        self.topk: int | None = None
        self.stopped = False
        self.dropped = 0
        self.sent = 0
        self._queue: queue.Queue = queue.Queue(maxsize=QUEUE_MAX)
        self._sock: socket.socket | None = None
        self._thread = threading.Thread(target=self._run, name="agentcoin-toploc", daemon=True)
        self._thread.start()

    def connected(self) -> bool:
        return self.topk is not None and not self.stopped

    def put(self, item) -> None:
        if not self.connected():
            self.dropped += 1
            return
        try:
            self._queue.put_nowait(item)
        except queue.Full:
            self.dropped += 1

    def close(self) -> None:
        self.stopped = True
        self._queue.put(None)
        self._thread.join(timeout=5)

    def _connect(self) -> None:
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.settimeout(5)
        sock.connect(self.path)
        sock.sendall(hello(self.hidden_size))
        (length,) = struct.unpack(">I", _read_exact(sock, 4))
        if length > MAX_FRAME:
            raise ConnectionError("oversized reply")
        version, topk = parse_welcome(_read_exact(sock, length))
        if version != VERSION:
            sock.close()
            log.error(
                "agentcoin toploc: the provider speaks protocol version %d, this plugin %d; "
                "no proofs will be sent",
                version,
                VERSION,
            )
            self.stopped = True
            return
        sock.settimeout(None)
        self._sock = sock
        self.topk = topk
        log.info("agentcoin toploc: connected to the provider (top-k %d)", topk)

    def _disconnect(self) -> None:
        if self._sock is not None:
            try:
                self._sock.close()
            except OSError:
                pass
        self._sock = None
        self.topk = None
        # Whatever was queued for the old connection is useless now.
        while True:
            try:
                if self._queue.get_nowait() is None:
                    self._queue.put(None)
                    return
                self.dropped += 1
            except queue.Empty:
                return

    def _run(self) -> None:
        while not self.stopped:
            if self._sock is None:
                try:
                    self._connect()
                except (OSError, ValueError, ConnectionError):
                    self._disconnect()
                    time.sleep(RECONNECT_SECONDS)
                continue
            try:
                item = self._queue.get(timeout=RECONNECT_SECONDS)
            except queue.Empty:
                continue
            if item is None:
                break
            try:
                for data in item():
                    self._sock.sendall(data)
                self.sent += 1
            except (OSError, ValueError):
                log.warning("agentcoin toploc: lost the provider connection; reconnecting")
                self._disconnect()
        if self._sock is not None:
            self._sock.close()
