# Home Migrations

This guide explains how Krabb upgrades the data in `KRABB_HOME`. See
[Storage](../krabb-docs/storage.md) for the layout, retention, and custom paths.

## Schema Steps

`schema.json` records the home schema, versioned independently of the application
and plugin protocol; a home without it is schema 0. `krabb-migrations` holds an
ordered registry in which step `n` migrates schema `n` to `n + 1`, so skipped
releases run every intermediate step. Before the first step, each path the
pending steps declare is copied to `.migration/` and verified byte for byte, then
the `.migration.json` journal is committed. The daemon and every command that
takes exclusive ownership of the state apply pending steps first. A failed step,
or a journal left by an interruption, restores the recovery point before state
is opened. Krabb refuses a home with a newer or unreadable schema before changing
it; `version`, `completion`, `service`, and `init --force` remain available.

Existing root channel state, `krabb.db`, and `media/` remain usable until an offline
migration moves them. New installations use the layout above. IPC clients can
connect to a legacy daemon, and the new daemon also acquires an existing legacy
`daemon.lock` to prevent concurrent writers. That legacy lock inode may remain at
the root after migration; metadata/plugin transaction locks also stay beside the
state they protect.

## Offline Migration

An offline migration must lock both daemon locations and plugin/metadata state,
reject old/new conflicts, back up original data, update attachment references,
and publish newly hashed immutable records before catalog references. It must not
edit hash-named records in place. An interrupted migration leaves `.migration.json`;
the next exclusive owner restores the recovery point before opening state. Keep
backups until you have checked chats, pending deliveries, memory, and schedules.
