"""Linux subprocess supervision, including descendants that create new sessions."""

import ctypes
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time


STOP = 0


def interrupted(signum, _frame):
    global STOP

    STOP = signum


def children(pid):
    found = set()

    try:
        threads = list(Path(f'/proc/{pid}/task').iterdir())
    except FileNotFoundError:
        return found

    for thread in threads:
        try:
            found.update(map(int, (thread / 'children').read_text().split()))
        except FileNotFoundError:
            continue

    return found


def descendants(pid):
    found = set()
    pending = [pid]

    while pending:
        for child in children(pending.pop()):
            if child not in found:
                found.add(child)
                pending.append(child)

    return found


def send(pid, signum):
    try:
        os.kill(pid, signum)
    except ProcessLookupError:
        pass


def reap():
    while True:
        try:
            pid, _status = os.waitpid(-1, os.WNOHANG)
        except ChildProcessError:
            return

        if pid == 0:
            return


def cleanup():
    # Subreaping keeps double-forked descendants attached after their parent exits.
    # Stop before killing so descendants cannot continue creating new children.
    deadline = time.monotonic() + 3

    while time.monotonic() < deadline:
        targets = descendants(os.getpid())

        if not targets:
            reap()
            return

        for pid in targets:
            send(pid, signal.SIGSTOP)

        targets |= descendants(os.getpid())

        for pid in targets:
            send(pid, signal.SIGKILL)

        reap()
        time.sleep(0.01)

    if descendants(os.getpid()):
        raise RuntimeError('unable to stop all child processes')


def main():
    if sys.platform != 'linux' or not Path('/proc/self/task').is_dir():
        raise RuntimeError('loop supervision requires Linux /proc (including supported WSL)')

    libc = ctypes.CDLL(None, use_errno=True)

    if libc.prctl(36, 1, 0, 0, 0) != 0:  # PR_SET_CHILD_SUBREAPER
        raise RuntimeError('kernel refused child-subreaper supervision')

    duration, *command = sys.argv[1:]

    if duration == 'loop':
        limit = None
    else:
        match = re.fullmatch(r'([1-9][0-9]*)([smhd]?)', duration)

        if not match:
            raise ValueError('invalid execution duration')

        limit = int(match[1]) * {'': 1, 's': 1, 'm': 60, 'h': 3600, 'd': 86400}[match[2]]

    for signum in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        signal.signal(signum, interrupted)

    process = subprocess.Popen(command, start_new_session=True)
    started = time.monotonic()
    result = None

    try:
        while result is None:
            expired = limit is not None and time.monotonic() - started >= limit

            if STOP or expired:
                result = 128 + STOP if STOP else 124

                for pid in descendants(os.getpid()):
                    send(pid, signal.SIGTERM)

                try:
                    process.wait(timeout=1)
                except subprocess.TimeoutExpired:
                    pass

                break

            try:
                code = process.wait(timeout=0.1)
                result = code if code >= 0 else 128 - code
            except subprocess.TimeoutExpired:
                continue
    finally:
        cleanup()

    return result


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, ValueError, RuntimeError) as error:
        print(f'[ERROR] Process supervision: {error}', file=sys.stderr)
        sys.exit(70)
