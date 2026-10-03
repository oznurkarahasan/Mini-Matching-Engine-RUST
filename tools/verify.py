#!/usr/bin/env python3
"""Independent black-box check for the Mini Matching Engine.

Generates random command files, runs your `engine` binary on each one and compares
its stdout byte for byte with an independent Python reference implementation.
The reference is an independent black-box reference written directly from the
spec by the second AI used for design discussions, not by the coding assistant.
It shares no code with the engine but follows the same spec interpretations
listed in README §4, so it cannot catch a misreading that both share.

Usage:
    python3 tools/verify.py ./target/release/engine [--streams 500] [--len 2000] [--seed 1]

On the first mismatch it stops and writes the input and both outputs to ./verify_fail/.
"""
import argparse
import os
import random
import subprocess
import sys
import tempfile
from collections import deque

U64_MAX = 2**64 - 1


class RefEngine:
    """Straightforward reference: spec rules written as directly as possible."""

    def __init__(self):
        self.bids = {}      # price -> deque of ids (FIFO)
        self.asks = {}
        self.orders = {}    # live id -> [account, side, price, remaining]
        self.accepted = set()
        self.last = None

    def live_ids(self):
        return list(self.orders.keys())

    def _book(self, side):
        return self.bids if side == "B" else self.asks

    def step(self, line):
        out = []
        line = line.rstrip("\r\n")
        if not line or line.startswith("#"):
            return out
        p = line.split(",")
        cmd = p[0]
        if cmd == "NEW":
            oid, acct, side, price, qty = int(p[1]), p[2], p[3], int(p[4]), int(p[5])
            if oid in self.accepted:
                return [f"REJECT,{oid},DUPLICATE_ID"]
            if price == 0 or qty == 0:
                return [f"REJECT,{oid},INVALID_VALUE"]
            if self.last is not None and abs(price - self.last) * 10 > self.last:
                return [f"REJECT,{oid},PRICE_BAND"]
            self.accepted.add(oid)
            out.append(f"ACK,{oid}")
            rem = qty
            opp = self.asks if side == "B" else self.bids
            while rem > 0 and opp:
                best = min(opp) if side == "B" else max(opp)
                if side == "B" and best > price:
                    break
                if side == "S" and best < price:
                    break
                q = opp[best]
                mid = q[0]
                m = self.orders[mid]
                if m[0] == acct:  # self-trade prevention, cancel newest
                    out.append(f"CANCELED,{oid},{rem}")
                    rem = 0
                    break
                t = min(rem, m[3])
                out.append(f"TRADE,{oid},{mid},{best},{t}")
                self.last = best
                rem -= t
                m[3] -= t
                if m[3] == 0:
                    q.popleft()
                    del self.orders[mid]
                    if not q:
                        del opp[best]
            if rem > 0:
                self.orders[oid] = [acct, side, price, rem]
                self._book(side).setdefault(price, deque()).append(oid)
        elif cmd == "CANCEL":
            oid = int(p[1])
            if oid not in self.orders:
                return [f"CANCEL_REJECT,{oid},UNKNOWN_ORDER"]
            o = self.orders.pop(oid)
            b = self._book(o[1])
            b[o[2]].remove(oid)
            if not b[o[2]]:
                del b[o[2]]
            out.append(f"CANCELED,{oid},{o[3]}")
        elif cmd == "MODIFY":
            oid, nq = int(p[1]), int(p[2])
            if oid not in self.orders:
                return [f"MODIFY_REJECT,{oid},UNKNOWN_ORDER"]
            if nq == 0:
                return [f"MODIFY_REJECT,{oid},INVALID_VALUE"]
            o = self.orders[oid]
            if nq > o[3]:  # increase loses priority
                q = self._book(o[1])[o[2]]
                q.remove(oid)
                q.append(oid)
            o[3] = nq
            out.append(f"MODIFIED,{oid},{nq}")
        return out


ACCOUNTS = ["A", "B", "C", "a", "acct-7"]  # 'a' vs 'A' checks byte-exact comparison


def gen_stream(rng, n):
    """Generate one stream, using a live reference to aim at interesting cases."""
    ref = RefEngine()
    lines, expected = [], []
    next_id = 1
    accepted_ids, rejected_ids = [], []
    huge_mode = rng.random() < 0.1  # some streams live near u64::MAX
    base = U64_MAX - 1000 if huge_mode else 100
    for _ in range(n):
        r = rng.random()
        if r < 0.03:
            line = rng.choice(["", "# comment", "#NEW,1,A,B,1,1"])
        elif r < 0.60:
            x = rng.random()
            if x < 0.08 and accepted_ids:
                oid = rng.choice(accepted_ids)
            elif x < 0.14 and rejected_ids:
                oid = rng.choice(rejected_ids)
            elif x < 0.16:
                oid = rng.choice([0, U64_MAX, rng.randrange(2**63, 2**64)])
            else:
                oid = next_id
                next_id += rng.randint(1, 3)
            side = rng.choice("BS")
            y = rng.random()
            if y < 0.04:
                price = 0
            elif y < 0.10 and ref.last is not None:
                d = ref.last // 10
                price = rng.choice([ref.last + d, ref.last - d, ref.last + d + 1, ref.last - d - 1])
                price = max(0, min(U64_MAX, price))
            elif y < 0.13:
                price = rng.choice([1, U64_MAX, rng.randrange(1, 2**64)])
            else:
                price = max(1, min(U64_MAX, base + rng.randint(-5, 5)))
            z = rng.random()
            qty = 0 if z < 0.04 else (U64_MAX if z < 0.05 else rng.randint(1, 10))
            line = f"NEW,{oid},{rng.choice(ACCOUNTS)},{side},{price},{qty}"
        elif r < 0.85:
            live = ref.live_ids()
            x = rng.random()
            if live and x < 0.75:
                oid = rng.choice(live)
            elif accepted_ids and x < 0.9:
                oid = rng.choice(accepted_ids)
            else:
                oid = rng.randrange(0, 2**64)
            line = f"CANCEL,{oid}"
        else:
            live = ref.live_ids()
            x = rng.random()
            oid = rng.choice(live) if live and x < 0.8 else rng.randrange(0, 2**64)
            cur = ref.orders.get(oid, [None, None, None, 5])[3]
            y = rng.random()
            if y < 0.12:
                nq = 0
            elif y < 0.4:
                nq = max(1, cur - rng.randint(0, cur))
            elif y < 0.55:
                nq = cur
            elif y < 0.58:
                nq = U64_MAX
            else:
                nq = min(U64_MAX, cur + rng.randint(1, 10))
            line = f"MODIFY,{oid},{nq}"
        ev = ref.step(line)
        if line.startswith("NEW,"):
            oid = int(line.split(",")[1])
            if ev and ev[0].startswith("ACK"):
                accepted_ids.append(oid)
            elif ev and ev[0].startswith("REJECT") and not ev[0].endswith("DUPLICATE_ID"):
                rejected_ids.append(oid)
        lines.append(line)
        expected.extend(ev)
    return lines, expected


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("engine")
    ap.add_argument("--streams", type=int, default=500)
    ap.add_argument("--len", type=int, default=2000)
    ap.add_argument("--seed", type=int, default=1)
    a = ap.parse_args()
    rng = random.Random(a.seed)
    total_lines = 0
    for s in range(a.streams):
        lines, expected = gen_stream(rng, a.len)
        crlf = rng.random() < 0.1
        nl = "\r\n" if crlf else "\n"
        no_final_nl = rng.random() < 0.1
        text = nl.join(lines) + ("" if no_final_nl else nl)
        exp = "".join(e + "\n" for e in expected)
        with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False, newline="") as f:
            f.write(text)
            path = f.name
        try:
            res = subprocess.run([a.engine, path], capture_output=True)
        finally:
            got_input = text
            os.unlink(path)
        got = res.stdout.decode("utf-8", errors="replace")
        total_lines += len(lines)
        if res.returncode != 0 or got != exp:
            os.makedirs("verify_fail", exist_ok=True)
            with open("verify_fail/input.txt", "w", newline="") as f:
                f.write(got_input)
            with open("verify_fail/expected.txt", "w") as f:
                f.write(exp)
            with open("verify_fail/got.txt", "w") as f:
                f.write(got)
            g, e = got.splitlines(), exp.splitlines()
            i = next((k for k in range(max(len(g), len(e)))
                      if k >= len(g) or k >= len(e) or g[k] != e[k]), None)
            print(f"MISMATCH in stream {s} (seed {a.seed}, crlf={crlf}, exit={res.returncode})")
            if i is not None:
                print(f"  first differing output line {i + 1}:")
                print(f"    expected: {e[i] if i < len(e) else '<none>'}")
                print(f"    got:      {g[i] if i < len(g) else '<none>'}")
            if res.stderr:
                print("  stderr:", res.stderr.decode(errors="replace").strip()[:500])
            print("  files written to verify_fail/")
            sys.exit(1)
        if (s + 1) % 50 == 0:
            print(f"{s + 1}/{a.streams} streams OK")
    print(f"ALL OK: {a.streams} streams, {total_lines} input lines")


if __name__ == "__main__":
    main()