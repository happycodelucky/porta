# Python Prototype Record

The repository began with a Python CLI named `portfree`. Its implementation is
preserved in Git history as behavioral design evidence, but it is not part of
the active Port Authority source tree or public release.

## Why it was built

The prototype tested whether a daemonless registry could coordinate several
application instances and coding agents on one machine. It made the difficult
parts concrete before choosing a distribution-oriented implementation language:

- Sparse allocation across a requested range.
- Durable directory-scoped ownership.
- Stable key-to-port mappings.
- Coordination between concurrent CLI processes.
- Cleanup after worktrees disappear.
- Shell-friendly and JSON command contracts.

## Implemented command surface

The final prototype executable is `portfree` version `0.4.0`:

```text
open          find open ports without reserving them
reserve       reserve ports for a directory
release       free reserved ports for a directory
get           return reserved ports for a directory
find          find the reservation for a port
list          list every registry entry
cleanup       observe and reap stale reservations
```

It supports the short options `-f`, `-t`, `-n`, `-k`, `-j`, and `-V`, plus the
compatibility aliases `--base` and `--max-port`.

Representative workflow:

```console
$ portfree reserve -f 6006 -k web -k api
web=6006 api=6008

$ portfree get -k web -k api
6006 6008

$ portfree release -k api
Released 6008
```

## Registry and configuration

Prototype state lives under `~/.portfree/`, or the directory supplied through
`PORTFREE_HOME`:

```text
~/.portfree/registry
~/.portfree/registry.lock
~/.portfree/config.toml
```

The version 2 JSON registry stores one record per canonical directory and one
record per allocation. Each allocation has a port, optional key, and creation
timestamp. Registry validation rejects duplicate directories, ports, and keyed
allocations.

The reader also accepts the original version 1 contiguous-range schema and
converts its ranges into unkeyed allocations on the next mutation.

Configuration defaults:

```toml
cleanup_threshold = 30
missing_for_days = 7
```

Writes use a per-user advisory file lock, a temporary file in the state
directory, file synchronization, and an atomic replacement of the registry.
This provided evidence for the locked JSON design. The Rust implementation kept
that design behind a backend-independent transaction boundary and added bounded
locks, strict schema validation, retained probe sockets, and multi-process
concurrency tests.

## Behaviors validated

The prototype was exercised end to end for:

- Open and reserved allocation.
- Sparse selection around occupied sockets and registered ports.
- Additive keyed and unkeyed reservations.
- Idempotent reuse of existing keys.
- Concurrent reservers receiving distinct ports.
- Partial keyed release and full directory release.
- Directory, key, and port lookup failures.
- First observation, restoration, and eventual reaping of missing directories.
- Automatic cleanup at the reservation threshold.
- Version 1 registry migration without losing ports.
- Plain and structured error output.
- Source and wheel builds plus isolated wheel installation.
- Homebrew formula-template syntax.

Formatting, Ruff linting, and strict BasedPyright checks passed at the final
prototype checkpoint.

## Implementation constraints and learnings

- It uses only the Python standard library at runtime.
- TCP availability is probed on IPv4 and IPv6 wildcard addresses.
- Probe sockets remain open while the registry transaction is committed.
- Reservations are advisory after the command exits.
- `/tmp` canonicalizes to `/private/tmp` on macOS, which reinforced the need for
  canonical directory identity.
- An editable installation was unreliable when macOS marked uv's generated
  `.pth` file hidden. Development checks therefore used forced non-editable
  installs so they exercised packaged code.
- The implementation uses `fcntl`, so its tested target is macOS and Linux.

## Commit history

The prototype was built in small validated checkpoints:

| Commit | Work recorded |
| --- | --- |
| `601f697` | Initial contiguous range allocator and registry |
| `6edf3b8` | Keyed, additive, sparse reservation commands |
| `4fbbdea` | `open` and `release` command naming |
| `3694c92` | Final `get` and `cleanup` names and command order |
| `c9a43c4` | Multiplexed-workstream CLI description |

These commits provide implementation evidence when porting behavior to Rust.
The Rust rewrite follows the contracts in the current
[specification](../SPEC.md) rather than mechanically translating Python modules.

## Mapping to Port Authority

| Prototype | Port Authority target |
| --- | --- |
| Product name `portfree` | Product name Port Authority |
| Executable `portfree` | Executable `porta` |
| Python package | Standalone Rust binary crate |
| `~/.portfree/registry` | Locked strict JSON at `~/.porta/registry.json` |
| `PORTFREE_HOME` | `PORTA_HOME` |
| Registry schema version 2 | New Porta schema version 1 |
| Configuration edited as TOML | Typed `porta config get/set` commands |
| Python and uv distribution | GitHub binaries, Cargo, Homebrew, and mise |
| `open` returns unregistered candidates | `lease` creates one timed lease |
| `get` returns one or more allocations | `get` returns one unambiguous allocation |
| `find` looks up one registered port | `info` reports registry and live state for several ports |
| `cleanup` ages missing directories | `clean` also sweeps leases and explicit expirations |
| Range and count options | Preferred base port, keyed counts, and `--contiguous` |

Version 1 intentionally makes a clean break and does not read prototype state.
Any future migration tool must copy and validate prototype data without
changing or deleting the old registry.

## Preserved artifacts

The prototype source, Python packaging, uv lockfile, original mise task graph,
and Python Homebrew formula are preserved by commits `601f697` through
`c9a43c4`. They were removed from the active tree when the Rust implementation
replaced them, keeping the public package unambiguous while retaining the full
design record.
