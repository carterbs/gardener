#!/usr/bin/env python3
"""
Gardener backlog watchdog.

Watches ~/.gardener/backlog.sqlite using kqueue. On any modification,
truncation, or deletion, immediately captures stat + lsof and appends
a timestamped entry to ~/.gardener/audit.log.

Designed to run as a launchd KeepAlive user agent so it survives
gardener session boundaries and catches wipes that happen between runs.

Usage:
  python3 scripts/watchdog.py [--db PATH] [--log PATH]
  (paths default to ~/.gardener/backlog.sqlite and ~/.gardener/audit.log)
"""

import datetime
import os
import select
import subprocess
import sys
import time


def default_path(name: str) -> str:
    home = os.environ.get("HOME", os.path.expanduser("~"))
    return os.path.join(home, ".gardener", name)


def resolve_args() -> tuple[str, str]:
    args = sys.argv[1:]
    db = default_path("backlog.sqlite")
    log = default_path("audit.log")
    i = 0
    while i < len(args):
        if args[i] == "--db" and i + 1 < len(args):
            db = args[i + 1]
            i += 2
        elif args[i] == "--log" and i + 1 < len(args):
            log = args[i + 1]
            i += 2
        else:
            i += 1
    return db, log


def ts() -> str:
    return datetime.datetime.now().isoformat(timespec="seconds")


def write_log(log_path: str, lines: list[str]) -> None:
    os.makedirs(os.path.dirname(log_path), exist_ok=True)
    with open(log_path, "a") as f:
        for line in lines:
            f.write(f"{ts()} {line}\n")


def stat_db(db_path: str) -> dict:
    try:
        st = os.stat(db_path)
        return {"exists": True, "size_bytes": st.st_size, "mtime": st.st_mtime}
    except FileNotFoundError:
        return {"exists": False, "size_bytes": -1, "mtime": 0}


def lsof_snapshot(dir_path: str) -> str:
    try:
        result = subprocess.run(
            ["lsof", "+D", dir_path],
            capture_output=True,
            text=True,
            timeout=5,
        )
        return result.stdout.strip() or "(no open files)"
    except Exception as e:
        return f"(lsof failed: {e})"


def open_db_fd(db_path: str) -> int | None:
    try:
        return os.open(db_path, os.O_RDONLY | os.O_NONBLOCK)
    except OSError:
        return None


def build_kevent(fd: int, fflags: int) -> select.kevent:
    return select.kevent(
        fd,
        filter=select.KQ_FILTER_VNODE,
        flags=select.KQ_EV_ADD | select.KQ_EV_CLEAR,
        fflags=fflags,
    )


# fflags for VNODE watches
DIR_FFLAGS = (
    select.KQ_NOTE_WRITE   # file created/deleted in directory
    | select.KQ_NOTE_ATTRIB
)
FILE_FFLAGS = (
    select.KQ_NOTE_WRITE   # data written (includes truncation)
    | select.KQ_NOTE_EXTEND
    | select.KQ_NOTE_DELETE
    | select.KQ_NOTE_ATTRIB
    | select.KQ_NOTE_RENAME
)


def run(db_path: str, log_path: str) -> None:
    dir_path = os.path.dirname(db_path)
    os.makedirs(dir_path, exist_ok=True)

    write_log(log_path, [f"[watchdog] started db={db_path}"])

    kq = select.kqueue()
    dir_fd = os.open(dir_path, os.O_RDONLY)
    file_fd = open_db_fd(db_path)

    events: list[select.kevent] = [build_kevent(dir_fd, DIR_FFLAGS)]
    if file_fd is not None:
        events.append(build_kevent(file_fd, FILE_FFLAGS))

    prev = stat_db(db_path)
    write_log(log_path, [f"[watchdog] baseline size={prev['size_bytes']} exists={prev['exists']}"])

    while True:
        try:
            triggered = kq.control(events, 8, 30.0)
        except (OSError, InterruptedError):
            time.sleep(1)
            continue

        if not triggered:
            # timeout — re-check in case we missed an event after a re-watch
            curr = stat_db(db_path)
            if curr["size_bytes"] != prev["size_bytes"] or curr["exists"] != prev["exists"]:
                triggered = ["poll"]  # force the block below

        if triggered:
            curr = stat_db(db_path)
            if curr["size_bytes"] != prev["size_bytes"] or curr["exists"] != prev["exists"]:
                entry: list[str] = [
                    f"[CHANGE] size={curr['size_bytes']} (was {prev['size_bytes']}) "
                    f"exists={curr['exists']} mtime={curr['mtime']}",
                ]
                lsof = lsof_snapshot(dir_path)
                entry.append(f"[lsof]\n{lsof}")
                write_log(log_path, entry)
                prev = curr

            # If the file was deleted or replaced, re-open and re-watch its new inode.
            if not curr["exists"]:
                if file_fd is not None:
                    try:
                        os.close(file_fd)
                    except OSError:
                        pass
                    file_fd = None
                    events = [build_kevent(dir_fd, DIR_FFLAGS)]
            else:
                new_fd = open_db_fd(db_path)
                if new_fd is not None and new_fd != file_fd:
                    if file_fd is not None:
                        try:
                            os.close(file_fd)
                        except OSError:
                            pass
                    file_fd = new_fd
                    events = [
                        build_kevent(dir_fd, DIR_FFLAGS),
                        build_kevent(file_fd, FILE_FFLAGS),
                    ]


if __name__ == "__main__":
    db_path, log_path = resolve_args()
    run(db_path, log_path)
