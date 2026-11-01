"""Enforce repository read-only access for an orchestrator and all its descendants."""

import ctypes
import os
from pathlib import Path
import sys


WRITE_FILE = 1 << 1
TRUNCATE = 1 << 14
WRITE = WRITE_FILE | sum(1 << bit for bit in range(4, 15))


class Ruleset(ctypes.Structure):
    _fields_ = [('handled_access_fs', ctypes.c_uint64)]


class Beneath(ctypes.Structure):
    _layout_ = 'ms'
    _pack_ = 1
    _fields_ = [('allowed_access', ctypes.c_uint64), ('parent_fd', ctypes.c_int32)]


def restrict(repository, output):
    libc = ctypes.CDLL(None, use_errno=True)
    abi = libc.syscall(444, 0, 0, 1)  # landlock_create_ruleset VERSION

    if abi < 3:
        raise RuntimeError('Landlock ABI 3 or newer is required for read-only orchestration')

    attributes = Ruleset(WRITE)
    rules = libc.syscall(444, ctypes.byref(attributes), ctypes.sizeof(attributes), 0)

    if rules < 0:
        raise OSError(ctypes.get_errno(), 'cannot create Landlock ruleset')

    def allow(path):
        flags = WRITE if path.is_dir() else WRITE_FILE | TRUNCATE
        descriptor = os.open(path, os.O_PATH | os.O_CLOEXEC)

        try:
            rule = Beneath(flags, descriptor)

            if libc.syscall(445, rules, 1, ctypes.byref(rule), 0) != 0:
                raise OSError(ctypes.get_errno(), 'cannot add Landlock path rule')
        finally:
            os.close(descriptor)

    try:
        # Permit ordinary agent runtime state outside the repository. Never grant a
        # repository ancestor: Landlock grants would also cover the protected subtree.
        current = Path('/')

        for component in repository.parts[1:]:
            selected = current / component

            for sibling in current.iterdir():
                if sibling == selected or sibling.is_symlink():
                    continue

                try:
                    allow(sibling)
                except FileNotFoundError:
                    continue

            current = selected

        if not output.is_file() or output.is_symlink():
            raise RuntimeError('final response must be a precreated regular file')

        allow(output)

        if libc.prctl(38, 1, 0, 0, 0) != 0:  # PR_SET_NO_NEW_PRIVS
            raise OSError(ctypes.get_errno(), 'cannot set no_new_privs')

        if libc.syscall(446, rules, 0) != 0:
            raise OSError(ctypes.get_errno(), 'cannot enforce Landlock ruleset')
    finally:
        os.close(rules)


def main():
    repository = Path(sys.argv[1]).resolve(strict=True)
    output = Path(sys.argv[2]).absolute()

    restrict(repository, output)
    os.execvp(sys.argv[3], sys.argv[3:])


if __name__ == '__main__':
    try:
        main()
    except (OSError, RuntimeError, ValueError) as error:
        print(f'[ERROR] Read-only enforcement unavailable: {error}', file=sys.stderr)
        sys.exit(70)
