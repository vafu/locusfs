#!/usr/bin/env python3
"""Watch a pollable LocusFS file with Linux epoll and print each reread value."""

import argparse
import os
import select
import sys
import time
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Watch a LocusFS file using epoll, then reread it on wakeup."
    )
    parser.add_argument("path", type=Path)
    parser.add_argument("--timeout", type=float, default=None)
    parser.add_argument("--max-events", type=int, default=0)
    args = parser.parse_args()

    fd = os.open(args.path, os.O_RDONLY | os.O_CLOEXEC)
    try:
        epoll = select.epoll()
        try:
            epoll.register(fd, select.EPOLLIN | select.EPOLLERR | select.EPOLLHUP)
        except OSError as error:
            print(f"epoll.register failed: {error}", file=sys.stderr)
            return 1

        print_value(fd, "initial")

        seen = 0
        while args.max_events <= 0 or seen < args.max_events:
            events = epoll.poll(args.timeout)
            if not events:
                print(f"{time.time():.6f} timeout", flush=True)
                continue

            for event_fd, mask in events:
                if event_fd != fd:
                    continue
                if mask & select.EPOLLERR:
                    print(f"{time.time():.6f} EPOLLERR", file=sys.stderr, flush=True)
                    return 1
                if mask & select.EPOLLHUP:
                    print(f"{time.time():.6f} EPOLLHUP", file=sys.stderr, flush=True)
                    return 1
                if mask & select.EPOLLIN:
                    seen += 1
                    print_value(fd, f"event {seen}")
    finally:
        os.close(fd)

    return 0


def print_value(fd: int, label: str) -> None:
    os.lseek(fd, 0, os.SEEK_SET)
    value = os.read(fd, 1024 * 1024).decode(errors="replace").rstrip("\n")
    print(f"{time.time():.6f} {label}: {value}", flush=True)


if __name__ == "__main__":
    raise SystemExit(main())
