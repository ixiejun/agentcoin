#!/usr/bin/env python3
"""Monte Carlo of the audit detection latency (m6-auditor-agent design D10, task 8.1).

A provider starts cheating at a uniformly random block of a round and keeps cheating. Each
round, m auditors are drawn for it; each sends its pinned request at a uniformly random block
of the first (100 - margin)% of the round, and its verdict lands `submit` blocks later. A
request sent after the switch fails. A dispute opens when the second failing verdict from a
distinct auditor lands (the chain counts the current and the previous round; with a cheat that
does not stop, every later request fails, so the second failure always qualifies). The
reviewers then need `review` blocks to fetch the evidence, re-check it and reach the quorum;
the provider is jailed at that block.

Prints, per round length, the mean, p95 and maximum blocks from the switch to the jail, and
the analytic worst case (the switch right after both of a round's requests, and both of the
next round's requests at the end of their window):

    worst = round + (1 - margin) * round + submit + review

Times are blocks of one second (MILLISECS_PER_BLOCK = 1000).

Usage: scripts/sim-audit-latency.py [--samples 100000] [--seed 1] [--rounds 1800,1200]
           [--assign 2] [--margin 25] [--submit 6] [--review 120] [--vote 600] [--json]
"""

import argparse
import json
import random
import statistics


def one(rng, length, assign, margin, submit, review):
    window = max(1, length - (length * margin) // 100)
    switch = rng.randrange(length)  # the switch, in round 0
    fails = []
    r = 0
    while len(fails) < 2:
        start = r * length
        for _ in range(assign):
            sent = start + rng.randrange(window)
            if sent > switch:
                fails.append(sent + submit)
        r += 1
    second = sorted(fails)[1]
    return second + review - switch


def percentile(values, q):
    ordered = sorted(values)
    k = max(0, min(len(ordered) - 1, int(round(q / 100 * len(ordered) + 0.5)) - 1))
    return ordered[k]


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--samples", type=int, default=100_000)
    p.add_argument("--seed", type=int, default=1)
    p.add_argument("--rounds", default="1800,1200")
    p.add_argument("--assign", type=int, default=2)
    p.add_argument("--margin", type=int, default=25)
    p.add_argument("--submit", type=int, default=6, help="blocks from request to verdict")
    p.add_argument("--review", type=int, default=120, help="blocks from dispute to quorum")
    p.add_argument("--vote", type=int, default=600, help="vote deadline in blocks")
    p.add_argument("--json", action="store_true")
    a = p.parse_args()

    out = []
    for length in (int(x) for x in a.rounds.split(",")):
        rng = random.Random(a.seed)
        xs = [one(rng, length, a.assign, a.margin, a.submit, a.review) for _ in range(a.samples)]
        window = length - (length * a.margin) // 100
        out.append({
            "round_blocks": length,
            "mean_s": round(statistics.mean(xs), 1),
            "p95_s": percentile(xs, 95),
            "max_s": max(xs),
            "worst_s": length + window + a.submit + a.review,
            "worst_with_full_vote_s": length + window + a.submit + a.vote,
        })
    if a.json:
        print(json.dumps(out))
        return
    print(f"samples {a.samples}, seed {a.seed}, assign {a.assign}, margin {a.margin}%, "
          f"submit {a.submit} s, review {a.review} s (vote deadline {a.vote} s)")
    for r in out:
        print(f"round {r['round_blocks']:>5} s: mean {r['mean_s'] / 60:5.1f} min, "
              f"p95 {r['p95_s'] / 60:5.1f} min, max {r['max_s'] / 60:5.1f} min, "
              f"worst {r['worst_s'] / 60:5.1f} min "
              f"({r['worst_with_full_vote_s'] / 60:5.1f} min if the vote takes its whole deadline)")


if __name__ == "__main__":
    main()
