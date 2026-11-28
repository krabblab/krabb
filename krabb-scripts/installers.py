#!/usr/bin/env python3
"""Write the installers a release attaches, with the SHA256SUMS lines that cover them.

Usage: installers.py VERSION DIRECTORY

The install scripts are stamped with VERSION, so a copy fetched from a release installs that
release unless KRABB_VERSION names another one. The uninstall scripts are copied unchanged.
"""

import hashlib
from pathlib import Path
import re
import sys


ROOT = Path(__file__).resolve().parents[1]
VERSION = re.compile(r'^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$')

STAMPS = {
    'install.sh': ("stamp=''", "stamp='{}'"),
    'install.ps1': ('$stamp = ""', '$stamp = "{}"'),
    'uninstall.sh': None,
    'uninstall.ps1': None,
}


class Failure(Exception):
    pass


def stamp(text, name, version):
    """The script with its stamp line naming VERSION."""

    rule = STAMPS[name]

    if rule is None:
        return text

    blank, stamped = rule
    lines = text.split('\n')
    found = [number for number, line in enumerate(lines) if line.rstrip('\r') == blank]

    if len(found) != 1:
        raise Failure(f'{name} must have exactly one `{blank}` line to stamp.')

    lines[found[0]] = stamped.format(version)

    return '\n'.join(lines)


def write(version, directory, root=ROOT):
    """Writes the stamped installers and their SHA256SUMS into DIRECTORY."""

    if not VERSION.fullmatch(version):
        raise Failure(f"Version '{version}' is not a valid release version.")

    directory.mkdir(parents=True, exist_ok=True)

    sums = []

    for name in STAMPS:
        text = (root / name).read_bytes().decode().replace('\r\n', '\n')
        content = stamp(text, name, version).encode()

        (directory / name).write_bytes(content)
        sums.append(f'{hashlib.sha256(content).hexdigest()}  {name}\n')

    (directory / 'SHA256SUMS').write_text(''.join(sums), newline='\n')


def main(argv=None):
    arguments = sys.argv[1:] if argv is None else argv

    if len(arguments) != 2:
        print(__doc__.strip().split('\n\n')[1], file=sys.stderr)
        return 2

    try:
        write(arguments[0], Path(arguments[1]))
    except Failure as error:
        print(f'[ERROR] {error}', file=sys.stderr)
        return 2

    print(f'[INFO] Wrote the Krabb {arguments[0]} installers to {arguments[1]}.')

    return 0


if __name__ == '__main__':
    sys.exit(main())
