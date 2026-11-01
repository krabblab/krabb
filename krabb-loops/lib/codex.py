"""Check the native sandbox and disable MCP without exposing configuration."""

import json
import os
import subprocess
import sys


def main():
    command = sys.argv[1:]
    directory = command[command.index('--cd') + 1]
    mode = command[command.index('--sandbox') + 1]

    probe = ['codex', 'sandbox',
             '--config', f'sandbox_mode="{mode}"',
             '--config', 'sandbox_workspace_write.network_access=false',
             '--config', 'sandbox_workspace_write.writable_roots=[]',
             '--', sys.executable, '-c',
             "from pathlib import Path; Path('Cargo.toml').read_bytes()"]

    result = subprocess.run(probe, cwd=directory, timeout=15)

    if result.returncode:
        print('[ERROR] Codex sandbox preflight failed; no agent started. '
              'Restore native sandbox access before rerunning the loop.', file=sys.stderr)

        return 70

    result = subprocess.run(['codex', '--disable', 'plugins', '--disable', 'apps',
                             'mcp', 'list', '--json'], capture_output=True, timeout=15)

    if result.returncode:
        raise ValueError('unable to inspect effective Codex MCP configuration')

    if len(result.stdout) > 4 * 1024 * 1024:
        raise ValueError('Codex MCP configuration exceeds size limit')

    entries = json.loads(result.stdout)

    if not isinstance(entries, list):
        raise ValueError('unexpected Codex MCP list format')

    flags = []

    for entry in entries:
        if not isinstance(entry, dict) or not isinstance(entry.get('name'), str):
            raise ValueError('unexpected Codex MCP server entry')

        name = json.dumps(entry['name'], ensure_ascii=True)

        flags.extend(['--config', f'mcp_servers.{name}.enabled=false'])

    # Keep stdin prompt marker last; every setting remains one literal argv item.
    command = command[:-1] + flags + command[-1:]

    os.execvp(command[0], command)


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, ValueError, subprocess.TimeoutExpired):
        print('[ERROR] Codex sandbox/MCP preflight failed; no agent started.',
              file=sys.stderr)

        sys.exit(70)
