# Related Work and Design Influences

Last reviewed: 2026-07-22

Port Authority is not the first tool to find or coordinate local ports. This
document records the projects considered during its design, the problems they
solve, and why Port Authority still explores a different set of tradeoffs.

This is a technical comparison, not a claim that one approach is universally
better. Port selection for a test process, durable worktree reservations, and a
shared local allocation service have different requirements.

## Evaluation criteria

The original use case was many application instances running concurrently under
humans and coding agents. Each instance may need several stable ports, may stop
and restart, and may live in a disposable worktree. The review therefore focused
on:

- Live socket availability checks.
- Durable reservations when no application is running.
- Multiple ports in one request.
- Stable key-to-port mappings.
- Coordination between concurrent processes.
- Directory or worktree ownership.
- Cleanup after directories disappear.
- Daemon and external-service requirements.
- Machine-readable command output.
- Installation as a standalone local-development tool.

## Comparison

| Project | Primary model | Coordination | Durable ownership | Port Authority distinction |
| --- | --- | --- | --- | --- |
| [python_portpicker](https://github.com/google/python_portpicker) | Python test helper and CLI | In-process checks or optional port-server daemon | Process identity through the optional server | Directory-scoped groups persist across process restarts without a daemon |
| [get-port](https://github.com/sindresorhus/get-port) | Node.js library with a related CLI | Lightweight process-local locking | No directory registry | Registry-visible timed leases plus durable keyed reservations |
| [PortKeeper](https://github.com/dynapsys/portkeeper) | Python library and CLI | File locking and optional held sockets | Owner and host records in a local registry | Canonical directory identity, additive keys, and directory-aging policy |
| [port-registry](https://github.com/n3r/port-registry) | Go server plus CLI and REST API | Local daemon backed by SQLite WAL | App, instance, and service records | Daemonless per-user transactions with directory lifecycle cleanup |

## python_portpicker

[Google's python_portpicker](https://github.com/google/python_portpicker) was
created to obtain unused network ports for tests. Its direct API checks for an
available port and returns the number. Its documentation explicitly warns about
the race between selecting a port and binding it.

It also includes an optional port-server design that coordinates allocations by
process ID through a Unix socket and reclaims them when processes exit. That is
useful for dense test infrastructure, where process lifetime is the appropriate
ownership boundary.

Lessons for Port Authority:

- Never describe an unbound returned port as guaranteed.
- Holding a bound socket or using a coordinating service is stronger than merely
  returning a number.
- Process lifetime is useful for tests but does not preserve stable mappings for
  stopped worktrees.

The repository was archived in April 2026, so it remains prior art rather than a
dependency candidate for the Rust implementation.

## get-port

[get-port](https://github.com/sindresorhus/get-port) is a focused Node.js library
for finding an available TCP port. It accepts preferred ports and ranges and can
temporarily lock returned ports against reuse within cooperating code. Its
documentation also acknowledges the unavoidable multi-process race before the
caller binds.

This is a strong fit when application code needs one available port now. Port
Authority instead targets coordination across independent CLI invocations and
across periods when the owning application is stopped.

Lessons for Port Authority:

- Preferred ranges and ascending sparse selection are a useful, unsurprising API.
- The race between discovery and binding must remain visible in documentation.
- Timed leases and durable directory reservations should be clearly
  distinguished.

## PortKeeper

[PortKeeper](https://github.com/dynapsys/portkeeper) is a Python library and CLI
that keeps a local registry, uses file locking, can reserve multiple ports, and
can hold sockets open. It also contains helpers for writing `.env` and JSON
configuration files and for launching commands with allocated values.

Port Authority deliberately keeps a smaller boundary. It returns stable data to
the caller but does not edit application configuration or supervise commands.
Its ownership and cleanup semantics are centered on canonical directories and
deleted worktrees.

Lessons for Port Authority:

- File locks can support a daemonless cooperating-process model when the entire
  read-probe-write transaction is locked and passes multi-process stress tests.
- Holding probe sockets through a registry transaction reduces allocator races.
- Application configuration-file mutation such as editing `.env` files is
  useful but belongs outside the first release.

There are multiple unrelated projects named PortKeeper. This comparison refers
specifically to the Python project at `dynapsys/portkeeper`.

## port-registry

[n3r/port-registry](https://github.com/n3r/port-registry) is the closest effort
to Port Authority's motivating use case. It explicitly supports multi-project AI
development, derives application and instance identity from Git repositories and
worktrees, and ships an agent skill. Its Go implementation uses a localhost
daemon, a versioned REST API, and SQLite in WAL mode.

That architecture is appropriate when a continuously available local API,
central service, and multiple client types are desirable. Port Authority chooses
a different operational boundary:

- No background process or service health lifecycle.
- One binary performs each transaction directly.
- Short-lived leases expire through command-triggered sweeps.
- Directory paths, rather than inferred app and branch labels, are the primary
  durable identity.
- Reservations age out only after the directory has been observed missing for a
  configured period.
- One request can atomically add several keyed or unkeyed ports to a directory.

The overlap should remain acknowledged in public documentation. Port Authority's
reason to exist is the daemonless directory-lifecycle model, not the claim that
agent-oriented port registries do not already exist.

Port Authority selected locked strict JSON for version 1 and does not adopt the
daemon or REST API. The storage decision remains separate from the process
model: JSON minimizes dependencies and is easy to inspect, while SQLite would
provide native transactions and constraints at the cost of a larger build and
distribution surface. The registry-store boundary remains so SQLite or another
backend can be reconsidered if cross-platform stress or recovery tests expose a
failure in the file design.

## Other approaches

### Bind port zero

When an application can accept an already-bound socket, asking the operating
system to bind port `0` is safer than choosing a number and binding later. Port
Authority is intended for applications and configuration systems that require a
port number before startup, often across several independent processes.

### Deterministic hashing

A project name can be hashed into the dynamic port range. This is simple and
stable but cannot by itself account for occupied ports or collisions with other
names. A registry makes conflicts explicit and allows reassignment.

### Shell scripts and process inspection

Commands based on `lsof`, `ss`, or `netstat` can find currently listening ports.
They cannot see an inactive reservation for a stopped worktree unless paired
with durable state.

### Containers and service discovery

Container runtimes can publish random host ports, and reverse proxies can avoid
exposing unique ports to users. Those are excellent deployment patterns, but
many local tools, debuggers, callbacks, and configuration files still require
known host port numbers.

## Design boundary

Port Authority should remain focused on:

- Local TCP port numbers.
- Per-user, per-host coordination.
- Canonical directory ownership.
- Keyed and unkeyed groups.
- Explicit release and conservative cleanup.
- Plain and JSON CLI contracts.

Potential features such as process termination, `.env` editing, application
launching, a GUI, remote synchronization, or a REST daemon should be evaluated
as separate layers rather than assumed to belong in the core tool.

## Review policy

Related-work claims can become stale. Recheck upstream documentation before a
public launch and before making comparative claims in release materials. Add new
projects when they materially overlap the directory-scoped, agent-oriented, or
daemonless reservation model.
