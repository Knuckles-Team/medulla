"""Served proof for MEDULLA-DURABLE-KERNEL-R869: the replayable stream log.

Eight concurrent publishers append to one stream over the real native engine;
the offsets they receive have no gap and no repeat. A cursor read returns the
messages in offset order, a consumer group commits its offset idempotently,
and after an engine restart the same reads return the same messages.
"""

from concurrent.futures import ThreadPoolExecutor

import pytest

pytestmark = [
    pytest.mark.no_engine,  # Each case owns its real isolated process.
    pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R869"),
]

PUBLISHERS = 8
PER_PUBLISHER = 25
STREAM = "inbox/tenant/session-r869"


def _payload(value):
    # A raw `Vec<u8>` may surface as bytes or a list of ints.
    return bytes(value)


def _read(rpc, ok, sock, from_offset, limit=0):
    rows = ok(
        rpc(
            sock,
            "StreamRead",
            {"stream": STREAM, "from_offset": from_offset, "max": limit},
        )
    )
    return [(int(offset), _payload(body)) for offset, body in rows or []]


def test_order_cursor_and_restart(tmp_path):
    """Verifies MEDULLA-DURABLE-KERNEL-R869.

    Replayable append-only stream log with offset reads.
    """
    from test_native_no_auth_served import ok, rpc, server

    with server(tmp_path) as sock:
        ok(
            rpc(
                sock,
                "StreamDeclare",
                {"stream": STREAM, "max_messages": None, "max_age_ms": None},
                key="declare",
            )
        )

        def publish(worker):
            offsets = []
            for index in range(PER_PUBLISHER):
                body = f"{worker}:{index}".encode()
                offsets.append(
                    (
                        int(
                            ok(
                                rpc(
                                    sock,
                                    "StreamPublish",
                                    {
                                        "stream": STREAM,
                                        "payload": body,
                                        "now_ms": 1_000 + index,
                                    },
                                    key=f"publish-{worker}-{index}",
                                )
                            )
                        ),
                        body,
                    )
                )
            return offsets

        with ThreadPoolExecutor(max_workers=PUBLISHERS) as pool:
            published = [
                pair for batch in pool.map(publish, range(PUBLISHERS)) for pair in batch
            ]
        total = PUBLISHERS * PER_PUBLISHER
        offsets = sorted(offset for offset, _ in published)
        assert offsets == list(range(total)), "offset gap or repeat"
        expected = sorted(published)

        everything = _read(rpc, ok, sock, 0)
        assert everything == expected
        # Each publisher's own messages keep their publish order.
        for worker in range(PUBLISHERS):
            mine = [
                body for _, body in everything if body.startswith(f"{worker}:".encode())
            ]
            assert mine == [f"{worker}:{i}".encode() for i in range(PER_PUBLISHER)]

        # Cursor reads page through the log without consuming it.
        cursor, paged = 0, []
        while True:
            page = _read(rpc, ok, sock, cursor, 30)
            if not page:
                break
            paged.extend(page)
            cursor = page[-1][0] + 1
        assert paged == expected
        assert _read(rpc, ok, sock, -1) == [], (
            "a negative cursor sees only new messages"
        )

        commit = {"stream": STREAM, "group": "g", "offset": cursor}
        ok(rpc(sock, "StreamCommitOffset", commit, key="commit"))
        ok(rpc(sock, "StreamCommitOffset", commit, key="commit-again"))
        committed = ok(
            rpc(sock, "StreamCommittedOffset", {"stream": STREAM, "group": "g"})
        )
        assert committed == cursor

    with server(tmp_path) as sock:
        assert _read(rpc, ok, sock, 0) == expected
        assert _read(rpc, ok, sock, 100, 10) == expected[100:110]
        committed = ok(
            rpc(sock, "StreamCommittedOffset", {"stream": STREAM, "group": "g"})
        )
        assert committed == cursor
        nxt = ok(
            rpc(
                sock,
                "StreamPublish",
                {"stream": STREAM, "payload": b"after-restart", "now_ms": 9_000},
                key="after-restart",
            )
        )
        assert int(nxt) == total
