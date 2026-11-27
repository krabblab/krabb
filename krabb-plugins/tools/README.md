# Tools

Install the released plugin with:

```sh
curl -fsSL https://raw.githubusercontent.com/krabblab/krabb/main/install.sh | sh -s -- tools
```

The plugin confines reads, writes, searches, and declared patch targets to
`KRABB_ROOT`, or to the active isolated workspace when the host supplies one.
Unix operations use descriptor-relative no-follow access; Windows operations
hold parent directory handles that prohibit directory replacement and use
reparse-point-safe file handles. Confined file operations fail closed on
platforms other than Unix and Windows. Writes reject
symbolic-link targets, and writes, patches, shell commands, and
worktree mutations need explicit approval.
File tools also reject paths inside `KRABB_HOME` when the workspace contains it, and
`file_read` reads only regular files.
Git inspection allows status, diff, worktree listing, and worktree add/remove
operations within that active workspace, and passes Git the resolved absolute
worktree path. Patches must name plain, unquoted paths. All Git operations and Git-backed
patches require shell access and the configured tool approval policy because
Git can execute repository-configured filters, even during `diff`. They are
unavailable when shell access is disabled; approval authorizes that execution. When a container sandbox is configured,
use the sandboxed shell tool for Git operations and patches; the direct Git
and patch tools reject execution on the host.
Git runs in the active workspace, but repository-configured programs have
shell permissions and may access paths outside that workspace.
Direct Git operations and Git-backed patches use the same descendant containment as host
shell commands. Descendants are stopped on completion, timeout, or plugin restart, with
the macOS limits described for the shell above.

The read-only `web_fetch` tool retrieves bounded text from public HTTPS URLs. It
blocks local and private network addresses, pins requests to validated DNS
addresses, rechecks redirects, caps redirects at three and response bodies at
2 MiB, and returns at most 48 KiB of text. Fetch requests have a 15-second
deadline and a four-request concurrency limit. The plugin declares its network
permission in the manifest.

Approved shell commands use the container sandbox when one is configured and otherwise run
on the host as the daemon's user. Host commands start in their own process group on Unix
and in a Job Object on Windows. Linux also sets no-new-privileges and rejects process-group
and session changes, so descendants cannot escape cancellation. macOS and other Unix hosts
have no equivalent restriction: a descendant that starts its own session survives
cancellation, and setuid programs keep their privileges. The host shell has the daemon
user's file access; only the container keeps commands inside the workspace.
To configure the sandbox, set `KRABB_SANDBOX_RUNTIME` to `docker` or
`podman` and `KRABB_SANDBOX_IMAGE` to an image already available locally.
The tools plugin never pulls images. The container has no network, a read-only
root filesystem, dropped Linux capabilities, bounded CPU, memory, process and
temporary-file resources, and a writable mount limited to the active workspace.
The workspace's `.git` and `.krabb` entries are mounted read-only.
The selected image must contain `sh` and allow its configured user to write the
workspace. Runtime errors fail the command; they never fall back to host shell
execution. Set `[clients.tui] shell = true` to enable TUI shell access. Shell
tools requested by an agent also require an enabled approval policy.
Shell and Git results set `truncated` to `true` when their output was shortened to fit
the protocol frame.

Use a local Docker or Podman engine. Remote container contexts are unsupported
because the active workspace mount must refer to this host's filesystem. The
sandbox narrows command access but does not replace kernel, daemon, or host
security updates.

Worktree turns expose only confined `file_read`, `file_write`, `file_list`, and `file_search`
operations.
The host blocks shell, Git/patch execution, and tools that cannot enforce the
worktree boundary. Filesystem operations reject internal runtime storage and
other conversation directories; searches skip `.git` and `.krabb` trees unless
explicitly reading permitted conversation media. See
[Visibility And Worktrees](../../krabb-docs/sharing.md).
