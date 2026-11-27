#!/usr/bin/env python3
"""Small dependency-free load tester for the Matchs API."""

import argparse
import concurrent.futures
import os
import statistics
import time
import urllib.error
import urllib.request

PATHS = [
    "/api/v1/competitions",
    "/api/v1/seasons?competition=L1",
    "/api/v1/matches?competition=L1&per_page=20",
    "/api/v1/standings?season=lfp:6&competition=L1",
    "/api/v1/live",
    "/api/v1/status",
]


def request(base, api_key, path):
    started = time.perf_counter()
    req = urllib.request.Request(base + path, headers={"X-API-Key": api_key})
    try:
        with urllib.request.urlopen(req, timeout=10) as response:
            response.read()
            status = response.status
    except urllib.error.HTTPError as err:
        status = err.code
    except Exception:
        status = 0
    return status, (time.perf_counter() - started) * 1000


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", default="http://192.168.122.35:18080")
    parser.add_argument("--key-file", default="api_key.txt")
    parser.add_argument("--requests", type=int, default=600)
    parser.add_argument("--workers", type=int, default=32)
    args = parser.parse_args()

    key = os.environ.get("API_KEY") or open(args.key_file).read().strip()
    tasks = [(args.base, key, PATHS[i % len(PATHS)]) for i in range(args.requests)]

    statuses = {}
    latencies = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.workers) as pool:
        for status, latency in pool.map(lambda task: request(*task), tasks):
            statuses[status] = statuses.get(status, 0) + 1
            latencies.append(latency)

    latencies.sort()
    print("requests:", args.requests, "workers:", args.workers)
    print("statuses:", dict(sorted(statuses.items())))
    print("p50_ms:", round(statistics.median(latencies), 1))
    print("p95_ms:", round(latencies[int(len(latencies) * 0.95) - 1], 1))
    print("max_ms:", round(latencies[-1], 1))


if __name__ == "__main__":
    main()
