# Port Authority (`porta`) Specification

- Status: Implemented baseline (`0.1.0`)
- Implementation: Rust 1.93
- Product name: Port Authority
- Executable name: `porta`

## 1. Summary

Port Authority is a daemonless command-line tool for finding and reserving local
TCP ports across multiplexed development workstreams. Its primary users are
developers and coding agents running several worktrees, sandboxes, services, or
application instances on one host.

Product description:

> Reserve and lease local TCP ports so parallel worktrees, agents, and dev
> servers stop colliding

Port Authority maintains a per-user registry so separate processes and
directories can coordinate port ownership even when a reserved application is
not currently running. It also supports short-lived leases when the caller
needs a port without a directory reservation.

## 2. Goals

- Allocate from a caller-preferred base port or from the default pool.
- Create short-lived, directory-independent leases for one-off port use.
- Associate durable reservations with canonical directory paths.
- Support stable key-to-port mappings such as `web`, `api`, or `database`.
- Coordinate all `porta` processes for the same operating-system user.
- Account for both live socket usage and inactive entries in the registry.
- Reclaim reservations conservatively after their directories disappear.
- Offer predictable plain-text output for shell scripts and versioned JSON
  output for programs and coding agents.
- Ship as a small, fast, standalone Rust binary for macOS and Linux.
- Support installation through GitHub releases, Homebrew, Cargo, and mise.

## 3. Non-goals

- Port Authority does not guarantee that a leased or reserved port remains
  available after the command exits. A process that does not coordinate through
  `porta` can still bind it.
- Port Authority does not launch, supervise, or stop applications.
- Port Authority does not terminate processes occupying ports.
- Port Authority does not allocate UDP ports in the initial release.
- Port Authority does not synchronize reservations between hosts.
- Port Authority does not initially coordinate different operating-system
  users. A home-directory registry is host-wide only for the current user.
- Port Authority does not require or run a daemon.

## 4. Terminology

- **Allocation**: one port plus an optional key and creation timestamp.
- **Directory reservation**: all durable allocations associated with one
  directory.
- **Lease**: a temporary, directory-independent claim on one port.
- **Keyed allocation**: an allocation with a stable caller-provided name.
- **Unkeyed allocation**: a directory allocation without a key.
- **Open port**: a port that is absent from the registry and can currently be
  bound on the host.
- **Missing reservation**: a reservation whose directory was absent during the
  most recent cleanup observation.
- **Reaped reservation**: a missing reservation removed after its grace period.
- **Sweep**: one pass that evaluates lease timeouts, reservation
  expirations, and missing-directory state.

## 5. Command-line interface

Running `porta` without a subcommand prints top-level help and exits successfully.
Top-level help must present commands in this order:

```text
lease         lease an open port without a directory
reserve       reserve ports for a directory
get           return a reserved port for a directory
list          list directory reservations and leases
listeners     show OS TCP listeners and owning processes
info          show registry or socket information for ports
release       release reserved ports or leases
clean         sweep and clean the registry
config        list, read, or update configuration
```

### 5.1 Shared options

| Short | Long | Meaning |
| --- | --- | --- |
| `-p` | `--port` | Preferred allocation scan base, or exact global release target |
| `-t` | `--timeout` | Lifetime of a lease |
| `-k` | `--key` | Lease identity, or repeatable reservation key |
| `-c` | `--contiguous` | Require a contiguous multi-port allocation |
| `-e` | `--expires` | Reservation expiration duration or timestamp |
| `-o` | `--order` | Listener output column ordering |
| — | `--force` | Reap missing directories without their grace period |
| `-j` | `--json` | Emit machine-readable JSON |
| `-V` | `--version` | Print the version |
| `-h` | `--help` | Print contextual help |

`--json` is a global option and should be accepted before or after the
subcommand. Subcommand-specific options may appear before or after the optional
directory argument where the argument parser permits it.

The canonical spelling is `--contiguous`. The misspelled `--contigous` form is
not part of the interface.

The Python prototype names `find` and `cleanup` are not aliases for `info` and
`clean`. The Rust CLI starts with the final interface rather than carrying
unpublished compatibility names.

### 5.2 Defaults and validation

- Allocation scans from the `--port` base through `65535`, or from `55000` when
  `--port` is omitted.
- `lease` claims exactly one port.
- `reserve` allocates one unkeyed port when no `--key` is supplied, whether the
  scan uses `--port` or the default base.
- On `lease` and `reserve`, `--port` is a non-repeatable starting preference,
  not an exact request. If that port is unavailable, scanning continues upward.
- On `release`, `--port` is repeatable and each value is an exact global target.
- On `reserve`, repeating `--key` requests one allocation for each key. On
  `lease`, `--key` is singular and identifies one renewable lease.
- Keys must be non-empty and unique within one request.
- The base port must be an integer in the inclusive range `1..=65535`.
- Allocation scans upward and returns the first available ports. Returned ports
  do not need to be consecutive unless `--contiguous` is set.
- Duration components use integer values followed by `s`, `m`, `h`, `d`, or
  `w`, and may be combined, for example `2d3h5m`.
- A bare integer supplied to `--timeout` means minutes.
- Absolute expirations use RFC 3339 with an explicit offset, for example
  `2026-08-01T12:00:00Z`.

### 5.3 `porta lease`

Lease one open port temporarily without associating it with a directory.

```text
porta lease [-p PORT] [-t DURATION] [-k KEY] [-j]
```

Examples:

```console
$ porta lease
55000

$ porta lease --port 6006 --timeout 1h --json
{"version":1,"type":"lease","port":6006,"key":null,"lease_id":"...","created_at":"...","expires_at":"..."}

$ porta lease --port 6006 --key preview
6006

$ porta lease --key preview --timeout 2h
6006
```

The default timeout is two minutes. The command leases the first available port
at or above the requested base, defaulting to `55000`. A live, reserved, or
already leased candidate is skipped rather than causing immediate failure.

`--key/-k` is an optional singular, global identity for a lease. If the
key does not exist, allocation follows the normal scan rules and stores the key.
If the key already exists, `lease` returns the same port and lease ID, preserves
`created_at`, and renews `expires_at` from the current time. A supplied `--port`
is validated but only controls initial allocation; it does not move an existing
keyed lease. Unkeyed invocations remain additive.

Because Port Authority has no daemon, timeout does not trigger work at the exact
deadline. An expired lease becomes eligible for reclamation during the next
sweep. The sweep releases it when its port is unbound. If the port is still in
use, the lease remains as `expired_in_use` and is reconsidered by later sweeps.

This protects a caller that successfully binds before timeout while eventually
returning abandoned leases to the available pool.

### 5.4 `porta reserve`

Add keyed or unkeyed allocations to a directory reservation.

```text
porta reserve [-p PORT] [-k KEY]... [-c] [-e WHEN] [-j] [DIRECTORY]
```

`DIRECTORY` defaults to the current directory and must exist. It is resolved to
an absolute canonical path before it becomes the reservation identity.

Keyed example:

```console
$ porta reserve --port 6006 --key web --key api --key database
web=6006 api=6008 database=6009
```

In this example, port `6007` is already unavailable, so sparse allocation skips
it. Unkeyed example:

```console
$ porta reserve --port 6006 ../feature-worktree
6006
```

Expiring contiguous example:

```console
$ porta reserve -k web -k api -k database -c -e 2d3h5m
web=55000 api=55001 database=55002
```

Reservation behavior:

- Requests are additive; they never replace existing allocations implicitly.
- An existing key is idempotent and returns its current port.
- New keys in the same request receive new ports.
- The key count determines the allocation count. With no keys, the request adds
  one unkeyed allocation.
- `--port` selects the scan's base for new allocations. It does not pair a port
  with a key or relocate an existing keyed allocation.
- A directory may have both keyed and unkeyed allocations.
- Keys are unique within a reservation but may be reused by other directories.
- `--contiguous` makes automatically selected new allocations one uninterrupted
  ascending block beginning at or above the base.
- Existing idempotent keyed allocations are not relocated to satisfy a new
  `--contiguous` request; contiguity applies only to new allocations.
- `--expires` sets or replaces the expiration of the whole directory
  reservation. A duration is measured from successful commit time. An absolute
  timestamp must be in the future.
- Reservation expiration is a hard deadline: the next sweep removes the record
  even if one of its ports is live. Callers that need directory-lifetime
  ownership should omit `--expires` and release explicitly.
- The complete request is atomic. A conflict or insufficient capacity leaves
  both allocations and expiration unchanged.

### 5.5 `porta release`

Release all allocations or selected keyed allocations for a directory, or
release exact registry ports globally.

```text
porta release [-k KEY...] [DIRECTORY] [-j]
porta release -p PORT... [-j]
```

`DIRECTORY` defaults to the current directory. It does not need to exist, which
allows a deleted worktree to be released explicitly.

- With no keys, remove the entire directory reservation.
- With keys, remove only those allocations.
- Remove the reservation record when its final allocation is released.
- If any requested key is absent, make no changes and return `key_not_found`.

Global port mode:

- `--port/-p` is repeatable and identifies exact registry ports.
- A target may be a directory allocation or a lease. Releasing a bound
  port only removes registry ownership; it does not stop the owning process.
- Directory ownership is not considered, and `--port` cannot be combined with
  `--key` or `DIRECTORY`.
- The request is atomic. If any target is not registered, make no changes and
  return `port_not_found`.
- Duplicate targets return `duplicate_port` without modifying the registry.
- Remove a reservation record when its final allocation is released.

### 5.6 `porta get`

Return one reserved port for a directory.

```text
porta get [-k KEY] [DIRECTORY] [-j]
```

`DIRECTORY` defaults to the current directory and does not need to exist.

- With `--key`, return the allocation for that key.
- Without `--key`, return the port only when the reservation contains exactly
  one allocation.
- If several allocations exist and no key is supplied, return `key_required`
  rather than selecting an arbitrary port.
- Plain output contains only the port number.

### 5.7 `porta info`

Show registry and live socket information for one or more ports.

```text
porta info PORT [PORT...] [-j]
```

Input order is preserved. Every valid port produces a result:

- `reserved`: owned by a directory reservation.
- `leased`: held by an unexpired lease.
- `expired_in_use`: an expired lease whose port is still bound.
- `available`: absent from the registry and currently bindable.
- `in_use`: absent from the registry but currently not bindable.

Plain output emits one tab-separated row per requested port:

```text
PORT<TAB>STATE<TAB>IN-USE<TAB>KEY-OR-DASH<TAB>OWNER-OR-DASH<TAB>EXPIRES-OR-DASH
```

### 5.8 `porta listeners`

Inspect listening TCP sockets reported by the operating system and show their
owning processes. This command does not read or create Port Authority state.

```text
porta listeners [PORT...] [-o COLUMN] [-j]
```

With no ports, return every visible TCP listener. With one or more ports, return
only listeners bound to those ports. Repeated port arguments are treated as one
filter. A requested port without a visible TCP listener is not an error.

Plain output is borderless aligned columns under a `PORT ADDRESS FAMILY PID
PROCESS` header. IPv4 and IPv6 bindings are separate rows.

`--order/-o` selects the primary sort column and accepts `port`, `address`,
`family`, `pid`, or `process`, defaulting to `port`. Any other value is
rejected as `invalid_arguments`. Rows with equal primary values fall back to
the canonical cascade of port, address family, address, PID, and process name,
so output is fully deterministic for every ordering. Addresses sort numerically
rather than as text, and every IPv4 address sorts before every IPv6 address.
The ordering applies to JSON output as well as the plain table. If some requested ports are not
listening, append `Not listening: ` followed by their sorted port numbers. If
none match, print `No TCP listeners found for: ` followed by the requested
ports. An unfiltered empty result prints `No TCP listeners found.`.

Only TCP sockets in the operating system's listening state are included; UDP
and established connections are excluded. Results are limited to listeners and
process ownership visible under the caller's operating-system permissions.

### 5.9 `porta list`

List every directory allocation and lease in the registry.

```text
porta list [-j]
```

Plain output groups allocations under their reservation directory. A directory
line may append `(missing)` and `(expires ...)` annotations. Each indented
allocation row shows the key (or `-`), the port, the state, and an `in use`
note when the port is currently bound. Leases follow in a `leases` section
whose rows carry their own `expires` annotations:

```text
/path/to/worktree
  web  6006  reserved  in use
  api  6008  reserved

leases
  preview  62000  leased  expires 01/15/2030 12:30:00 PM -08:00
```

Directories are ordered by path with allocations by port, followed by leases
ordered by port. Live socket state is refreshed for listed ports.

`list` stats every reservation directory to report its status, and persists
what it observed: an absent directory that is not yet marked records
`missing_since`, and a directory that has returned has its mark cleared. This
starts the grace period at the moment the loss first becomes visible rather
than waiting for the next `clean`. `list` never reaps, so listing cannot
remove a reservation. When any directory is missing, plain output ends with a
line reporting the count and pointing at `porta clean`.

Human-readable expirations are converted to the system's local timezone and
formatted using its locale's date and time conventions, followed by an
explicit numeric UTC offset. Because this presentation varies by machine,
automation must use `--json`; JSON timestamps remain UTC RFC 3339 values.

`list` does not enumerate all unregistered free ports; that would mean scanning
the entire TCP range. An empty registry prints `No reservations or leases.`.

### 5.10 `porta clean`

Observe missing directories, restore returned directories, and reap reservations
whose missing grace period has elapsed.

```text
porta clean [--force] [-j]
```

Plain output is borderless aligned label and count columns, leading with the
number of reaped reservations so the destructive result is the first thing
read:

```text
reaped                  2
released leases         0
expired reservations    0
marked missing          0
restored                0
next automatic cleanup  30
```

Cleanup is conservative by default:

1. The first observation of an absent directory records `missing_since`.
2. A later cleanup removes it only when the configured grace period has elapsed.
3. If the directory reappears before reaping, `missing_since` is cleared.
4. A grace period of zero still requires two observations: one to mark and a
   subsequent one to reap.

`--force` overrides that model. Every reservation whose directory is absent at
that moment is reaped in a single pass, regardless of whether it was already
marked and regardless of `missing_for`. The count appears in `reaped`. Forcing
never touches a reservation whose directory exists, never removes files, and
never stops a process bound to a released port. There is no short form; the
flag is spelled `--force` so it cannot be typed by accident.

Every registry command performs the inexpensive expiration sweep needed for
leases and explicit reservation expirations. Missing-directory marking and
restoration also run during `list`, which already stats each directory.
Reaping runs explicitly through `clean` and automatically before `lease` or
`reserve` when the number of directory reservations is at least the registry's
current automatic cleanup trigger.

The trigger starts at `cleanup_trigger_start`, which defaults to 30 directory
reservations. After an allocation-triggered sweep reaps no more than 10
reservation records, increase the trigger by `cleanup_trigger_step`, defaulting
to 15 and constrained to `10..=15`. Never raise it above 100. If a sweep reaps
more than 10 records, leave the trigger unchanged; the lower remaining
reservation count naturally delays the next sweep. An explicit `porta clean`
does not change the trigger.

The current trigger is persistent registry state so separate invocations see the
same schedule. Configuration changes clamp it into the new valid range on the
next registry transaction: at least `cleanup_trigger_start` and at most 100.

### 5.11 `porta config`

List supported configuration or read and update one user configuration value.

```text
porta config [-j]
porta config get KEY [-j]
porta config set KEY VALUE [-j]
```

Examples:

```console
$ porta config
KEY                    EFFECTIVE  DEFAULT  STATUS
default_port           55000      55000    unset
lease_timeout          2m         2m       unset
missing_for            1w         1w       unset
cleanup_trigger_start  30         30       unset
cleanup_trigger_step   15         15       unset

$ porta config get missing_for
1w

$ porta config set cleanup_trigger_start 40
cleanup_trigger_start=40

$ porta config set cleanup_trigger_step 10 --json
{"version":1,"type":"config_set","key":"cleanup_trigger_step","value":10}
```

With no action, `config` returns every supported key in canonical order. Plain
output is borderless aligned columns under a `KEY EFFECTIVE DEFAULT STATUS`
header. `STATUS` is `set` when the key is present in `config.toml` and `unset`
when the effective value comes from the built-in default.

JSON returns a `settings` array. Each entry contains `key`, `value`, `default`,
and `is_set`. Integer settings remain JSON numbers and durations remain strings.
Explicitly setting a value equal to its default still produces `is_set: true`.

`get` returns the effective value, including a default when the key is absent
from `config.toml`. `set` parses and validates the value according to the key's
type before atomically replacing the configuration file. Unknown keys and
invalid values fail without changing the file. Configuration changes affect
commands that begin after the update commits; an already-running command keeps
the validated configuration snapshot with which it started.

Integer settings are JSON numbers. Duration settings are canonical duration
strings in both JSON and plain output.

Changing `missing_for` applies to already-marked reservations using their
existing `missing_since` timestamp; it does not restart their grace period.

## 6. Output contract

### 6.1 Plain output

- Successful `lease`: the leased port number.
- Successful unkeyed reservation: space-separated ports.
- Successful keyed reservation: space-separated `KEY=PORT` pairs.
- `get`: the selected port number only.
- `release`: `Released ` followed by space-separated ports.
- `info`: stable tab-separated records with UTC RFC 3339 timestamps.
- `listeners`: aligned columns of visible TCP listeners and owning processes.
- `list`: allocations grouped by directory, with locale-formatted local times,
  and a trailing missing-directory count when any reservation is missing.
- `clean`: aligned label and count columns, led by `reaped`.
- bare `config`: aligned columns of effective values, defaults, and set status.
- `config get`: the canonical value only.
- `config set`: `KEY=VALUE` using the canonical stored representation.
- Diagnostics and non-JSON errors go to standard error.
- Successful data goes to standard output.

### 6.2 JSON output

Every JSON data or error response is a single object followed by a newline.
JSON field names are stable API surface. Additive fields are allowed in minor
releases; removing or changing fields requires a major release. Help, version,
and bare top-level help remain human-readable text even when `--json` is present.

Every response includes a numeric envelope version and a type discriminator:

```json
{
  "version": 1,
  "type": "lease"
}
```

Errors use:

```json
{
  "version": 1,
  "type": "error",
  "error": {
    "code": "reservation_not_found",
    "message": "No reservation exists for /path/to/worktree"
  }
}
```

Allocation representation:

```json
{
  "port": 6006,
  "key": "web",
  "created_at": "2026-07-21T19:30:30.896925Z"
}
```

Lease representation:

```json
{
  "id": "28e5f42b-d26d-4974-9db9-b895433a94ef",
  "port": 6006,
  "key": "preview",
  "created_at": "2026-07-21T19:30:30.896925Z",
  "expires_at": "2026-07-21T19:32:30.896925Z",
  "status": "leased"
}
```

Port information representation:

```json
{
  "port": 6006,
  "state": "leased",
  "in_use": false,
  "key": "preview",
  "directory": null,
  "lease_id": "28e5f42b-d26d-4974-9db9-b895433a94ef",
  "expires_at": "2026-07-21T19:32:30.896925Z"
}
```

Fields that do not apply to a state are `null`, rather than omitted, in port
information objects. This gives batch consumers one stable shape.

Operating-system listener representation:

```json
{
  "port": 6006,
  "address": "127.0.0.1",
  "family": "ipv4",
  "pid": 4125,
  "process": "node"
}
```

Reservation representation:

```json
{
  "id": "41e65e15-0d15-407f-871b-707b084c4920",
  "directory": "/path/to/worktree",
  "created_at": "2026-07-21T19:30:30.896925Z",
  "expires_at": null,
  "missing_since": null,
  "status": "active",
  "ports": [6006, 6008],
  "keyed_ports": {"api": 6008, "web": 6006},
  "allocations": []
}
```

The stable success type vocabulary and remaining command-specific schemas are:

- `type: "lease"`: `port`, `key`, `lease_id`, `created_at`, `expires_at`.
- `type: "reserve"`: `directory`, `reservation_id`, `expires_at`, `ports`,
  `allocations`, `keyed_ports` when applicable, and `reused_keys` when
  non-empty.
- `type: "release"`: `ports`, `released`, and `directory` for directory/key
  releases.
- `type: "get"`: `directory`, `port`, `key`, `allocation`.
- `type: "info"`: `ports`, containing one information object per requested port.
- `type: "listeners"`: `listeners`, containing visible TCP listener objects, and
  `missing_ports`, containing sorted requested ports without a visible listener.
- `type: "list"`: `reservations`, `leases`.
- `type: "clean"`: `released_leases`, `expired_reservations`, `marked_missing`,
  `restored`, `reaped`, `cleanup_trigger`.
- `type: "config"`: `settings`, containing ordered objects with `key`,
  `value`, `default`, and `is_set`.
- `type: "config_get"` and `type: "config_set"`: `key`, `value`.

All errors use `type: "error"` and include `error.code` plus `error.message`.
`version` and `type` are reserved envelope fields and cannot be supplied by a
command payload.

### 6.3 Exit status

| Status | Meaning |
| --- | --- |
| `0` | Success |
| `1` | Valid request with no matching reservation, key, port, or capacity |
| `2` | Invalid CLI input, configuration, or registry data |
| `3` | State-directory, lock, socket, or registry I/O failure |

The same error code and exit status must be used in plain and JSON modes.

### 6.4 Error codes

The initial stable error vocabulary includes:

| Code | Meaning |
| --- | --- |
| `invalid_arguments` | CLI shape or option combination is invalid |
| `invalid_port` | A port is outside `1..=65535` |
| `invalid_duration` | A timeout or expiration cannot be parsed or is not positive |
| `duplicate_key` | A key occurs more than once |
| `duplicate_port` | A global release target occurs more than once |
| `no_available_ports` | Automatic allocation cannot satisfy the request |
| `reservation_not_found` | The directory has no reservation |
| `key_not_found` | The reservation has no requested key |
| `port_not_found` | No reservation or lease owns a requested global release port |
| `key_required` | `porta get` is ambiguous without a key |
| `invalid_config` | Configuration is unreadable or invalid |
| `invalid_config_key` | The requested configuration key is unknown |
| `invalid_config_value` | The value is invalid for the requested key |
| `config_busy` | The configuration lock could not be acquired within the bounded wait |
| `config_write_failed` | Atomic configuration persistence fails |
| `invalid_registry` | Registry data violates its schema or invariants |
| `registry_busy` | The registry could not be acquired within the bounded wait |
| `registry_lock_failed` | The registry lock cannot be acquired or released |
| `registry_write_failed` | Atomic registry persistence fails |
| `socket_probe_failed` | Availability cannot be determined reliably |
| `listeners_unsupported` | Operating-system listener inspection is unsupported on this platform |
| `listeners_query_failed` | Operating-system listener inspection failed |

## 7. State and configuration

### 7.1 State location

Default state directory:

```text
~/.porta/
```

Files:

```text
~/.porta/config.toml
~/.porta/config.lock
~/.porta/registry.json
~/.porta/registry.lock
```

Version 1 uses the strict JSON backend with a separate stable lock file. The
configuration filename and lock are independent of the registry transaction.

`PORTA_HOME` overrides the state directory. Tests must always use this override
to avoid touching developer state.

Permissions on Unix:

- State directory: `0700`.
- Configuration, registry, and lock files: `0600`.

The registry coordinates every `porta` process run by the same user. True
cross-user coordination would require a separately designed shared state path,
permission model, and privileged installation and is out of scope for v1.

### 7.2 Directory identity

Directory identity is deterministic even when a worktree has already been
deleted:

1. Expand a leading home-directory marker when it was not expanded by the shell.
2. Resolve a relative path against the process's current directory.
3. Canonicalize every existing path component, including symlinks.
4. Lexically normalize any remaining missing components without requiring the
   final directory to exist.
5. Store and compare the resulting absolute path.

`reserve` additionally requires the final path to be an existing directory.
`port` and `release` do not. Two paths that resolve to the same existing directory
must address the same reservation. Once a symlink itself has been deleted, an
alias that can no longer be resolved is not guaranteed to reproduce the stored
identity; callers should use the original worktree path.

### 7.3 Configuration schema

`config.toml` accepts any subset of the supported keys. A full example is:

```toml
default_port = 55000
lease_timeout = "2m"
missing_for = "7d"
cleanup_trigger_start = 30
cleanup_trigger_step = 15
```

Rules:

- `default_port` is an integer in `1..=65535`.
- `lease_timeout` is a positive duration using the CLI duration grammar.
- `missing_for` is a non-negative duration using the CLI duration grammar.
- `cleanup_trigger_start` is an integer in `1..=100` and counts directory
  reservation records, not allocated ports.
- `cleanup_trigger_step` is an integer in `10..=15`.
- The automatic trigger cap of 100 and low-yield boundary of 10 reaped
  reservations are fixed version 1 policy rather than configuration keys.
- Unknown fields are rejected so misspellings do not silently disable policy.
- A missing configuration file uses defaults.
- Omitted keys use defaults and are reported as unset by bare `porta config`.
- `config set` persists only explicitly set keys. Existing full files remain
  valid and every physically present key is reported as set.
- An unreadable or invalid configuration is an error; it is never ignored.

### 7.4 Configuration transactions

`porta config set` uses `config.lock` and the same locked read-modify-write,
temporary-file synchronization, atomic replacement, and parent-directory sync
principles as the JSON registry backend. The stable lock file is never replaced.
This prevents concurrent setters from losing one another's updates.

Configuration lock acquisition uses a bounded wait and reports `config_busy` on
timeout. Persistence failures report `config_write_failed` and leave the prior
complete configuration in place.

Other commands read and validate one coherent old-or-new configuration snapshot
at startup. They do not hold `config.lock` while waiting for the registry lock,
which avoids a cross-file lock cycle. A concurrent configuration update applies
to the next command invocation.

## 8. Registry format

The Porta registry is at logical schema version `2`. The example below is
the canonical data model and physical representation of the selected strict
JSON backend.

```json
{
  "schema_version": 2,
  "maintenance": {
    "cleanup_trigger": 30
  },
  "open_leases": [
    {
      "id": "28e5f42b-d26d-4974-9db9-b895433a94ef",
      "port": 62000,
      "key": "preview",
      "created_at": "2026-07-21T19:30:30.896925Z",
      "expires_at": "2026-07-21T19:32:30.896925Z"
    }
  ],
  "reservations": [
    {
      "id": "41e65e15-0d15-407f-871b-707b084c4920",
      "directory": "/absolute/canonical/path",
      "created_at": "2026-07-21T19:30:30.896925Z",
      "expires_at": null,
      "missing_since": null,
      "allocations": [
        {
          "port": 6006,
          "key": "web",
          "created_at": "2026-07-21T19:30:30.896925Z"
        }
      ]
    }
  ]
}
```

The registry file's `open_leases` field is schema-version-2 state. The 2026
CLI rename from `open` to `lease` does not change this on-disk name; renaming
it would require a new `schema_version` and an explicit migration for no
behavioral gain.

### 8.1 Registry invariants

- `schema_version` must be recognized.
- `maintenance.cleanup_trigger` is an integer in `1..=100`.
- Reservation and lease IDs are UUIDs and globally unique within the registry.
- Directory paths are absolute, canonical, and unique.
- A reservation contains at least one allocation.
- Ports are integers in `1..=65535` and unique across all directory allocations
  and leases.
- Keys are either non-empty strings or `null`.
- Non-null reservation keys are unique within one reservation. Non-null lease
  keys are unique across all leases.
- Timestamps are UTC RFC 3339 strings.
- Every `expires_at` is later than its record's `created_at`.
- Reservation `expires_at` is either `null` or a UTC RFC 3339 timestamp.
- `missing_since` is either `null` or a UTC RFC 3339 timestamp.
- The JSON backend rejects unknown fields within the current schema version. A
  database backend validates its physical schema and migration version before
  exposing the logical model.
- A registry that violates an invariant is not modified automatically.

Schema version 1 registries are read through a strict version 1 model and
migrated to version 2 inside the next locked registry transaction. Each existing
open lease receives `"key": null`; all other data is preserved. Version 2 is
then written atomically before the lock is released. Older binaries reject the
new schema version rather than silently discarding keyed lease identities.

### 8.2 Transactions and locking

The required consistency boundary covers selection as well as persistence.
Every registry operation follows this sequence:

1. Create the state directory if necessary.
2. Acquire the backend's exclusive write transaction or the JSON backend's
   exclusive advisory lock.
3. Only after acquiring it, read and fully validate the latest state, or
   initialize an empty registry.
4. Perform expiration sweeps, missing-directory cleanup when applicable,
   queries, socket probes, and mutations while holding the lock.
5. Commit the complete mutation atomically and durably according to the
   backend's documented guarantees.
6. Release probe sockets, then end the transaction or release the lock.

An implementation must never read a registry snapshot before locking and later
write a decision based on that stale snapshot. All cooperating writers are
serialized across the entire read-sweep-probe-update-commit sequence. A request
that cannot commit leaves no partial allocations and cannot discard a
concurrent process's update.

For the JSON-file backend, the concrete commit sequence is:

1. Open or create the dedicated `registry.lock` file and acquire an exclusive
   operating-system lock. The lock file remains at a stable path and is never
   replaced as part of a registry commit.
2. Reread `registry.json` after the lock is acquired.
3. Serialize the complete new registry to a uniquely named temporary file in
   the state directory.
4. Flush and sync the temporary file.
5. Atomically rename it over `registry.json`.
6. Sync the parent directory where supported.
7. Keep the lock held until all commit steps have completed or failed.

Holding probe sockets until the registry commit prevents two concurrent `porta`
processes from selecting the same port. It cannot prevent an unrelated process
from racing the reservation after the sockets are released.

Read-only commands also take the lock so they never observe a partially
completed cleanup or mutation. The initial implementation may use an exclusive
lock for all operations; shared read locks are a later optimization.

Lock acquisition must use a bounded wait and return the structured
`registry_busy` error when another process does not release the registry within
that bound. A lock-holder crash must release the operating-system lock; no stale
PID lockfile protocol is permitted. Advisory locking coordinates cooperating
`porta` processes only and does not make hand-editing the live registry safe.

Version 1 supports state stored on a local filesystem. Network, FUSE, and synced
filesystems are unsupported unless the selected backend's locking and atomic
commit behavior passes the same conformance suite there.

### 8.3 Storage backend boundary

CLI and allocation code must depend on a small registry-store abstraction, not
on JSON files or SQL directly. The abstraction must support:

- Reading a validated snapshot for display operations.
- Running a serialized transaction that may sweep, probe, and mutate.
- Atomically committing all requested ports or none.
- Reporting busy, corrupt, unsupported-version, and persistence failures with
  backend-independent error codes.
- Schema-version inspection and explicit migration.

The implementation selects strict JSON with operating-system locking and atomic
replacement for version 1. The store abstraction remains in place so a future
backend can be evaluated without coupling allocation or CLI code to storage.
The alternatives considered were:

| Backend | Advantages | Costs and limits |
| --- | --- | --- |
| Strict JSON, OS lock, and atomic rename | Human-inspectable; minimal implementation and dependency surface; likely smallest baseline for a small per-user registry | Every writer must honor the lock; rewrites the whole registry; correctness depends on supported filesystem lock, sync, and rename semantics |
| SQLite | Transactions, constraints, crash recovery, and serialized writes are built in; supports more complex queries and migrations | Bundled SQLite increases binary and build surface; system SQLite weakens standalone portability; still depends on filesystem locking and needs an explicit busy policy |
| Pure-Rust transactional embedded store | Can provide transactions without bundling a C library | Multiprocess behavior, crash recovery, maintenance maturity, binary size, and migration ergonomics must be verified for the selected crate |

SQLite is a robustness option, not a presumed binary-size optimization. Both a
bundled and system-linked build have distribution tradeoffs and must be measured
before selection.

### 8.4 Backend decision record

The JSON spike was retained because independent concurrent processes produced
unique ports without lost updates, concurrent configuration setters preserved
both updates, malformed state is rejected, and same-directory temporary-file
replacement preserves a complete old-or-new registry. The release binary does
not need SQLite or a C build dependency.

The choice must be revisited before a stable release if CI or stress testing
exposes any of these failures:

- Concurrent writers can produce a duplicate allocation or lost update.
- Terminating a writer at any commit stage can leave corrupt or partial state.
- Lock release after process death or atomic replacement is unreliable on a
  supported operating system and local filesystem.
- Registry-size or contention benchmarks show unacceptable whole-file rewrite
  latency.
- Required constraints or migrations cannot remain clear and safely testable.

Version 1 bounds registry input at 16 MiB, configuration input at 64 KiB, and
lock acquisition at five seconds. Release verification records the stripped
binary size. Correctness takes priority over artifact size if the backend is
reconsidered.

The locally built stripped `aarch64-apple-darwin` release binary for `0.1.0` is
1,351,024 bytes. CI is responsible for recording and validating the other
supported artifacts.

### 8.5 Existing prototype migration

Version 1 intentionally makes a clean break from the unpublished Python
prototype at `~/.portfree/registry`. It neither reads nor modifies prototype
state. A future explicit migration command may be added only when all of the
following are true:

- No Porta registry exists for the selected backend.
- The old registry is valid version 1 or version 2 data.
- The user explicitly approves migration, or passes a future non-interactive
  migration flag.

Migration must copy and convert data without deleting or modifying the old
registry. Automatic, silent migration is not permitted.

## 9. Port availability algorithm

For each candidate port in ascending order:

1. Skip it if it appears in any directory reservation or lease after the
   current sweep.
2. Attempt to bind a TCP listener to the IPv4 wildcard address.
3. Attempt to bind a TCP listener to the IPv6 wildcard address with IPv6-only
   behavior enabled where supported.
4. Treat the port as unavailable if a supported address family cannot bind it.
5. Retain successful probe sockets until enough candidates are found and any
   registry mutation is committed.
6. If the range is exhausted, close every probe socket and return
   `no_available_ports` without mutating the requested reservation.

An unavailable IPv6 stack does not make every port unavailable; unsupported
address-family errors are ignored after the IPv4 check succeeds. Unexpected
socket errors are reported rather than silently interpreted as occupancy.

The scan begins at the caller's `--port` value or the configured default. Sparse
allocation retains each available candidate until it has satisfied the request.
Contiguous allocation instead scans for the first complete block that satisfies
every socket and registry check.

Lease and reservation creation must be atomic from the caller's perspective. A
request for three ports either registers all three or none.

## 10. Rust implementation design

### 10.1 Project shape

The implementation is one binary crate with a reusable internal library:

```text
Cargo.toml
Cargo.lock
src/
  main.rs
  lib.rs
  cli.rs
  atomic.rs
  config.rs
  duration.rs
  error.rs
  listeners.rs
  lock.rs
  registry/
    mod.rs
    model.rs
    store.rs
  service.rs
  socket.rs
  state.rs
```

`cli.rs` translates command input into service operations and renders output.
Registry, allocation, sweeping, and socket behavior do not depend on terminal
rendering. `store.rs` defines the backend-independent transaction contract.

### 10.2 Dependencies

- `clap` with derive support for CLI parsing and generated help.
- `serde` and `serde_json` for public JSON output and the JSON-file backend.
- `toml` for configuration.
- `time` for UTC RFC 3339 timestamps.
- A small in-tree parser for the specified duration grammar.
- `uuid` for reservation identifiers.
- `thiserror` for typed domain and infrastructure errors.
- Rust standard-library `File` locking.
- `socket2` for IPv6-only wildcard probing.
- `listeners` for native operating-system TCP listener and process inspection.
- `tempfile` for same-directory atomic-write staging and tests.
The exact resolved versions are pinned through `Cargo.lock`. No SQLite,
directory-discovery, async-runtime, or duration-parser dependency is used.

### 10.3 Core types

```rust
struct Registry {
    schema_version: u32,
    maintenance: Maintenance,
    open_leases: Vec<OpenLease>,
    reservations: Vec<Reservation>,
}

struct Maintenance {
    cleanup_trigger: usize,
}

struct OpenLease {
    id: Uuid,
    port: u16,
    key: Option<String>,
    created_at: OffsetDateTime,
    expires_at: OffsetDateTime,
}

struct Reservation {
    id: Uuid,
    directory: PathBuf,
    created_at: OffsetDateTime,
    expires_at: Option<OffsetDateTime>,
    missing_since: Option<OffsetDateTime>,
    allocations: Vec<Allocation>,
}

struct Allocation {
    port: u16,
    key: Option<String>,
    created_at: OffsetDateTime,
}

struct Config {
    default_port: u16,
    lease_timeout: Duration,
    missing_for: Duration,
    cleanup_trigger_start: usize,
    cleanup_trigger_step: usize,
}
```

The logical model and public JSON representation remain contracts. Internal
types and the selected physical store may change only when schema compatibility
and migration behavior remain explicit.

## 11. Development tooling

### 11.1 mise

The repository uses mise to pin Rust and all development tooling and to
orchestrate repeatable tasks:

```toml
[tools]
rust = "1.93.1"
shellcheck = "latest"
"cargo:cargo-audit" = "latest"

[tasks.fmt]
run = "cargo fmt --all --check"

[tasks.lint]
run = "cargo clippy --all-targets --all-features -- -D warnings"

[tasks.test]
run = "cargo test --all-features"

[tasks.check]
depends = ["fmt", "lint", "test", "shellcheck"]

[tasks.build]
depends = ["check"]
run = "cargo build --release --locked"
```

The task graph also exposes `format`, `lint-stable`, `install`, `smoke`,
`audit`, and `package`. `lint-stable` checks the code with the moving stable
Rust toolchain in addition to the pinned minimum version. `install` runs
`cargo install --path . --locked --force` and respects `CARGO_INSTALL_ROOT`.
Every tool invocation in development and CI runs through mise. `mise.lock` pins
the resolved project tool artifacts across the supported platform matrix.

### 11.2 Quality gates

Required before a release:

- `cargo fmt --all --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- The same Clippy gate on the current stable Rust toolchain
- `cargo test --all-features`
- `cargo build --release --locked`
- `cargo audit` or an equivalent dependency vulnerability check
- End-to-end command tests against an isolated `PORTA_HOME`
- Package installation and smoke tests from the produced release artifact

## 12. Test strategy

### 12.1 Unit tests

- Base-port, key-count, and duration validation.
- Duplicate and empty key validation.
- Keyed idempotency and additive behavior.
- Sparse and contiguous allocation selection.
- Base-port fallback around live, reserved, and leased candidates.
- Registry invariant validation.
- Configuration defaults and strict validation.
- Configuration get/set parsing, canonical output, atomic replacement, and
  concurrent setter preservation.
- Automatic cleanup trigger initialization, low-yield scaling, cap behavior,
  persistence, and normalization after configuration changes.
- No automatic missing-directory sweep below 30 reservations by default; a
  sweep at the trigger; increases after 0 or 10 reaped records but not 11;
  repeated low-yield sweeps at the 100-record cap; and no scaling after manual
  `clean`.
- Lease expiry while unused and while bound.
- Hard reservation expiration.
- Missing, restored, and reaped cleanup transitions using an injected clock.
- JSON serialization snapshots for public response types.
- Stable plain-output formatting.

### 12.2 Integration tests

- Every command in plain and JSON modes.
- Current-directory and explicit-directory resolution.
- Lookup and release after the directory has been deleted.
- `get` selection with one allocation, a key, and ambiguous unkeyed input.
- Batched `info` output for reserved, leased, expired-in-use, available, and
  unregistered in-use ports.
- A live bound socket being skipped.
- A directory-reserved or open-leased inactive port being skipped.
- Mixed keyed and unkeyed allocations.
- Base-port sparse and contiguous keyed allocation.
- Full rollback when insufficient ports remain.
- Corrupt and unsupported registry versions.
- Invalid configuration.
- bare `config` defaults, explicit status, order, and typed JSON values.
- `config get` defaults and persisted values in plain and JSON modes.
- `config set` success, unknown keys, invalid values, and concurrent updates.
- Many concurrent reservers competing for one port produce exactly one winner.
- Concurrent distinct reservations preserve every committed update.
- A writer delayed after reading cannot commit a stale snapshot over a newer
  transaction.
- Atomic registry visibility during concurrent reads and writes.
- Bounded lock contention returns `registry_busy` without mutation.
- Terminating a lock holder releases the lock for a later process.
- Terminating a writer before, during, and after the durable commit boundary
  leaves either the complete old state or complete new state.
- Expiration sweeping on every registry command.
- Missing-directory cleanup at, below, and above the reservation threshold.
- IPv4-only environments and hosts with IPv6 enabled.

### 12.3 Registry backend conformance and stress tests

Run one backend-independent suite against every serious storage candidate:

- All invariant, transaction, sweep, and migration cases.
- Repeated randomized multi-process contention for the same and different
  ports and keys, with no duplicate ports or lost commits.
- Fault injection at every file-sync, rename, transaction-commit, and unlock
  boundary.
- Process termination while holding the lock or write transaction.
- Registry growth and contention benchmarks at documented test sizes.
- Release-mode binary-size and cold-start measurements for each backend build.

The suite must use independent operating-system processes, not only threads, and
must run on every supported operating system. The JSON candidate must also run
on each supported local filesystem used by the release matrix. A backend cannot
be selected based only on unit tests of its adapter.

### 12.4 Release smoke tests

For every supported target:

1. Install the produced archive or package into a clean temporary prefix.
2. Run `porta --version` and `porta --help`.
3. Confirm command help order.
4. Run `lease`, `reserve`, `get`, `listeners`, `info`, `list`, `release`,
   `clean`, and `config get/set` against a temporary directory and isolated
   state root.
5. Confirm structured failures for invalid arguments and missing records.

## 13. Packaging and distribution

### 13.1 GitHub releases

Each signed version tag should produce checksummed archives containing the
`porta` binary, license, and README for:

- macOS ARM64
- macOS x86-64
- Linux x86-64 GNU
- Linux ARM64 GNU

Musl and Windows targets may be added after platform behavior is specified and
tested. Release automation should generate provenance or attestations where the
chosen release tool supports them.

### 13.2 Cargo

The crates.io package name is `port-authority`; `porta` was already occupied by
an unrelated project. The binary name remains `porta`.

```console
$ cargo install port-authority --locked
```

Before publishing:

- Populate license, description, repository, homepage, readme, keywords, and
  categories in `Cargo.toml`.
- Run `cargo publish --dry-run` and inspect `cargo package --list`.

### 13.3 mise

The Cargo backend provides an immediate installation path after crates.io
publication:

```console
$ mise use -g rust
$ mise use -g cargo:port-authority@latest
```

Git-based installation can be documented before the first crates.io release.
Prebuilt GitHub artifacts should also be compatible with mise's `ubi` or GitHub
backend so users are not required to compile Rust.

After public releases are stable, submit a `porta` shorthand to the mise
registry, preferring a verified prebuilt backend with
`cargo:port-authority` as fallback.
The intended end-user command is:

```console
$ mise use -g porta@latest
```

No custom mise or asdf plugin should be created unless standard backends prove
insufficient.

### 13.4 Homebrew

Maintain a formula in a public tap initially:

```console
$ brew install OWNER/tap/porta
```

The formula installs immutable, checksummed release artifacts and tests both
`porta --version` and an isolated `porta lease` call. Submission to Homebrew Core
can be considered after the project meets its notability and maintenance
requirements.

### 13.5 Release automation

The release workflow should:

1. Require a clean, reviewed commit and passing CI.
2. Derive one SemVer version shared by the crate, tag, and binaries.
3. Build and test the supported target matrix.
4. Publish GitHub archives and checksums.
5. Publish the crate to crates.io.
6. Update the Homebrew tap.
7. Verify installation from Cargo, a release archive, and mise.

Publishing credentials must use scoped tokens or trusted publishing and must
never be stored in the repository.

## 14. Security and resilience

- Treat registry and configuration contents as untrusted input.
- Never execute values read from the registry.
- Do not follow registry-provided paths for writes; only test whether stored
  directory paths exist.
- Reject malformed or unsupported registry data without partially rewriting it.
- Prevent symlink substitution of temporary registry files where platform APIs
  allow it.
- Keep state private to the user by default.
- Avoid logging user environment variables or complete home-directory contents.
- Reject registry files larger than 16 MiB and configuration files larger than
  64 KiB before parsing.
- Surface lock poisoning, disk-full conditions, permission failures, and atomic
  rename failures as explicit errors.

## 15. Compatibility and versioning

- The executable follows Semantic Versioning.
- The JSON `version` identifies the response-envelope contract independently of
  the executable version. A breaking JSON change increments it.
- CLI removals, renamed commands, changed exit meanings, and incompatible JSON
  changes require a major version.
- Additive JSON fields and new optional flags may ship in minor versions.
- Registry changes require a new `schema_version` and an explicit migration.
- A newer unsupported registry version must never be downgraded or overwritten.
- Plain output intended for shell consumption is compatibility-sensitive even
  when it is not JSON.

## 16. Acceptance criteria for the first Rust release

- The product is named Port Authority and installs a binary named `porta`.
- All nine commands exist in the specified order and their options expose the
  specified short forms.
- Default automatic allocation starts at port `55000`.
- `lease` creates a two-minute lease by default and expired unused leases return
  to the pool on the next sweep.
- Base-port scans for `lease` and `reserve` skip live, reserved, and leased ports;
  sparse and contiguous reservation paths follow the specified selection rules.
- Keyed reservations are additive and idempotent per canonical directory.
- Reservation durations and RFC 3339 expirations are enforced by sweeps.
- Concurrent reservation processes cannot allocate the same registered port.
- Concurrent registry writes cannot lose another process's committed update.
- Registry transactions are bounded, atomic, and recover safely from
  interrupted writes and lock-holder termination.
- The selected backend passes the common multi-process conformance suite on all
  supported targets, and its measured release binary size is recorded.
- Missing-directory cleanup follows the two-observation grace-period model.
- bare `config` lists every documented key with effective/default values and
  explicit set status; `config get/set` atomically reads and updates each key.
- Allocation-triggered cleanup begins at 30 reservations, persists low-yield
  trigger increases of 10–15, and caps the trigger at 100.
- `get`, batched `info`, OS `listeners`, `list`, and `clean` match the specified
  query and lifecycle behavior.
- Plain and JSON outputs match the documented contracts.
- Formatting, lint, unit, integration, concurrency, and release smoke tests pass
  on supported macOS and Linux targets.
- GitHub release archives, Cargo installation, Homebrew tap installation, and
  explicit mise installation are documented and verified.

## 17. Remaining release decisions

- Which release builder will generate cross-platform binaries and checksums.
- Whether prebuilt mise installation should prefer `ubi`, `github`, or an Aqua
  registry entry.
- Whether Windows support belongs in v1 or a later release.
- The benchmark workloads and release-binary size target for the first public
  stable release.
- Whether a future privileged mode should coordinate reservations across users.

## 18. Reference documentation

- [Related work and design influences](docs/related-work.md)
- [Python prototype record](docs/python-prototype.md)
- [Rust `File` locking](https://doc.rust-lang.org/std/fs/struct.File.html)
- [SQLite isolation and serialized writes](https://www.sqlite.org/isolation.html)
- [SQLite atomic commit](https://www.sqlite.org/atomiccommit.html)
- [mise Cargo backend](https://mise.jdx.dev/dev-tools/backends/cargo.html)
- [mise backend architecture](https://mise.jdx.dev/dev-tools/backend_architecture)
- [Publishing on crates.io](https://doc.rust-lang.org/cargo/reference/publishing.html)
