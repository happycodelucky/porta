# Port Authority

![Rust 1.93+](https://img.shields.io/badge/Rust-1.93%2B-CE422B.svg?style=for-the-badge&logo=rust&logoColor=white)
![macOS](https://img.shields.io/badge/macOS-ARM64%20%7C%20x86--64-blue.svg?style=for-the-badge&logo=apple)
![Linux](https://img.shields.io/badge/Linux-ARM64%20%7C%20x86--64-FCC624.svg?style=for-the-badge&logo=linux&logoColor=black)
![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg?style=for-the-badge)

Reserve and lease local TCP ports so parallel worktrees, agents, and dev
servers stop colliding. The binary is `porta` — one small daemonless Rust
executable.

## Why

Running one application locally is easy. Running several copies is not: every
frontend, API, database, worker, and debugger wants a port, usually from the
same familiar defaults. Multiply that by a few git worktrees and a coding
agent or two, and something is always camped on 3000.

Port Authority is a small allocator for exactly that situation. Each worktree
asks for the ports it needs and gets numbers nobody else has claimed. State
lives in one per-user registry file, concurrent runs coordinate through an OS
file lock, and nothing runs in the background.

One honest caveat up front: reservations are advisory. `porta` hands out
numbers and remembers them, but it can't stop an uncooperative process from
binding a port after the command exits. Most dev servers have a strict-port
flag for exactly this — the recipes below use it.

## Install

> **Not released yet.** Building from source works today; the other channels
> below go live with the first tagged release.

### Homebrew

```bash
brew install happycodelucky/tap/porta
```

### mise

```bash
mise use -g cargo:port-authority@latest
```

Once the shorthand lands in the mise registry, `mise use -g porta@latest` will
do the same thing without the Cargo backend.

### Cargo

```bash
cargo install port-authority --locked
```

The crates.io package is `port-authority` because `porta` was already taken by
an unrelated project. The binary it installs is still `porta`.

### Prebuilt binaries

Checksummed archives for macOS and Linux (ARM64 and x86-64) are attached to
each [release](https://github.com/happycodelucky/porta/releases). Download,
verify, and drop `porta` somewhere on your `PATH` — no Rust toolchain needed.

### From source

```bash
git clone https://github.com/happycodelucky/porta.git
```

```bash
mise install && mise run install
```

`mise install` fetches the pinned toolchain; `mise run install` runs the full
check gate and installs `porta` into Cargo's binary directory, normally
`~/.cargo/bin` (`CARGO_INSTALL_ROOT` is respected). Without mise:

```bash
cargo install --path . --locked
```

Then confirm it landed:

```bash
porta --version
```

## Commands

```text
lease      lease an open port without a directory
reserve    reserve ports for a directory
get        return a reserved port for a directory
list       list directory reservations and leases
listeners  show OS TCP listeners and owning processes
info       show registry or socket information for ports
release    release reserved ports or leases
clean      sweep and clean the registry
config     list, read, or update configuration
```

### Grab a port right now

`lease` claims one open port for a limited time, no setup required:

```console
$ porta lease --port 62000 --timeout 1h
62000
```

The timeout defaults to two minutes. An expired lease is reclaimed by a later
command once its port is unbound; if you actually bound the port, the lease is
kept as `expired_in_use` and checked again later.

Give a lease a key and it becomes renewable — the same call returns the same
port and resets the clock:

```console
$ porta lease -k preview -t 2h
62001

$ porta lease -k preview -t 2h
62001
```

On `lease` and `reserve`, `--port/-p` is a starting preference, not a demand:
the scan begins there and walks upward past anything live, reserved, or
already leased. A key's port is chosen once; a later `--port` won't move it.

### Reserve named ports for a directory

`reserve` is the durable version. Allocations belong to a directory and stay
until you release them:

```console
$ porta reserve --port 6006 --key web --key api --key database
web=6006 api=6008 database=6009

$ porta get --key web
6006
```

Reserving is additive and idempotent: run it again with an extra key and the
existing keys keep their ports while only the new key allocates. Add
`--contiguous/-c` when new allocations must form one unbroken block, and
`--expires/-e` (a duration like `2d3h5m`, or an RFC 3339 timestamp) when the
whole reservation should have a deadline. The directory argument defaults to
the current directory.

`get` returns one port: pass `--key` when the reservation has several.

### See what's going on

Three read commands, three sources:

```console
$ porta list           # what porta is tracking
$ porta listeners      # what the OS says is actually listening
$ porta info 6006      # everything about specific ports, both sources
```

`listeners` asks the operating system for live TCP listeners — port, address,
family, PID, and process name. Pass ports to filter; JSON output also reports
requested ports with no listener in `missing_ports`. OS permissions decide
which listeners and owners you can see.

Sort it with `--order/-o` — `port` (the default), `address`, `family`, `pid`,
or `process`. Grouping by process is handy when you're hunting down which app
is hogging things:

```console
$ porta listeners -o process
PORT   ADDRESS    FAMILY  PID    PROCESS
5000   0.0.0.0    IPv4    631    ControlCenter
7000   0.0.0.0    IPv4    631    ControlCenter
5037   127.0.0.1  IPv4    69264  adb
```

`list` groups allocations under their directory, with leases in their own
section:

```console
$ porta list
/Users/paul/dev/app
  web  6006  reserved  in use
  api  6008  reserved

leases
  preview  62000  leased  expires 07/22/2026 04:12:09 PM -07:00
```

Expirations print in your local timezone. Scripts should use
`porta list --json`, which keeps timestamps as UTC RFC 3339.

`info` reports each requested port as `reserved`, `leased`, `expired_in_use`,
`available`, or `in_use`.

### Let go

```console
$ porta release                    # whole reservation for the current directory
$ porta release -k api             # just one key
$ porta release -p 6006 -p 6008    # exact ports, regardless of owner
$ porta clean                      # sweep expirations, age out deleted worktrees
```

Releasing by `--port` targets exact registry entries anywhere — it can't be
combined with `--key` or a directory, and it's atomic: if any requested port
isn't registered, nothing changes. Releasing never terminates the process
bound to a port.

Deleted worktrees aren't reaped on sight. `clean` marks a missing directory
with a timestamp and only reclaims it after a grace period (a week by
default). Cleanup also runs automatically once you accumulate enough
reservations.

### JSON for scripts and agents

Every data command accepts `--json/-j`, before or after the subcommand. Each
response carries `version: 1` and a `type` discriminator such as `lease`,
`listeners`, or `config_get`. Errors use `type: "error"` with a stable code,
and exit statuses match plain-text mode:

```json
{"version":1,"type":"error","error":{"code":"invalid_port","message":"Port must be between 1 and 65535"}}
```

## Recipes

These examples use Bash. Quote `"$PWD"` rather than backtick `pwd` so
directory-derived keys survive paths with spaces. Prefix the key with a
purpose like `storybook:` when more than one tool runs from the same
directory.

### Get the next open port for the current directory

```bash
port="$(porta lease -p 6006 -k "$PWD")"
printf 'Using port %s\n' "$port"
```

Returns the first available port at or above `6006`. The directory is the
lease identity, so running it again from the same place renews the same
lease. This is a timed lease, not a reservation — no `reserve` needed, and
missing-directory cleanup never touches it.

### Run one Storybook per worktree

```bash
storybook_port="$(porta lease -p 6006 -k "storybook:${PWD}" -t 8h)"
npm run storybook -- --port "$storybook_port" --exact-port
```

The purpose prefix keeps Storybook from sharing an identity with another
server in the same worktree. `--exact-port` makes Storybook fail loudly if
another process wins the advisory bind race instead of silently moving
elsewhere. See the
[Storybook CLI options](https://storybook.js.org/docs/10.5/api/cli-options).

### Run one Vite server per worktree

```bash
vite_port="$(porta lease -p 5173 -k "vite:${PWD}" -t 8h)"
npm run dev -- --port "$vite_port" --strictPort
```

Vite's `--strictPort` gives the same bind-race protection. See the
[Vite CLI options](https://vite.dev/guide/cli).

### Supply several ports to one development command

```bash
api_port="$(porta lease -p 3000 -k "api:${PWD}" -t 8h)"
web_port="$(porta lease -p 3000 -k "web:${PWD}" -t 8h)"

export API_PORT="$api_port"
export WEB_PORT="$web_port"
npm run dev
```

The two identities are independent and needn't be contiguous. Each scan
starts at `3000` and skips live sockets, reservations, and unexpired leases.

### Reserve stable named ports for a worktree

```bash
porta reserve -p 6006 -k storybook -k web -k api "$PWD"

storybook_port="$(porta get -k storybook "$PWD")"
web_port="$(porta get -k web "$PWD")"
api_port="$(porta get -k api "$PWD")"
```

Use `reserve` when the names should hold steady across commands and terminal
sessions. Repeating it returns existing keys and allocates only new ones.

### Reserve a contiguous block

```bash
porta reserve -p 7000 -c -k api -k metrics -k debugger "$PWD"
```

Finds the first three-port block at or above `7000` that's free in both the
registry and the operating system.

### Isolate parallel test workers

```bash
worker_id="${CI_NODE_INDEX:-0}"
test_port="$(porta lease -p 55000 -k "tests:${PWD}:${worker_id}" -t 30m)"
PORT="$test_port" npm test
```

Include the worker in the key so a retry renews its own lease without
colliding with the other workers.

### Consume a typed JSON response

```bash
set -euo pipefail

port="$(
  porta lease -p 6006 -k "preview:${PWD}" --json |
    jq -er 'select(.version == 1 and .type == "lease") | .port'
)"
```

`jq -e` fails the script when the version, type, or port doesn't match the
expected success shape.

### Find what owns a port, then release registry state

```bash
porta info 6006
porta listeners 6006

porta release -p 6006
```

`info` combines registry ownership with live availability; `listeners` names
the owning PID and process. `release -p` removes the registry entry — it
never kills the process on the port.

## State

Per-user state lives in `~/.porta/` (override with `PORTA_HOME`):

```text
~/.porta/registry.json
~/.porta/registry.lock
~/.porta/config.toml
~/.porta/config.lock
```

The registry is strict JSON guarded by an OS file lock. Every transaction
acquires the lock, rereads and validates state, sweeps expirations, probes
candidate ports while holding the probe sockets, atomically replaces the
file, and only then lets go. Lock waits are bounded at five seconds. Version
1 registries migrate atomically on their next operation.

## Configuration

```console
$ porta config
$ porta config get missing_for
$ porta config set default_port 56000
```

Bare `porta config` prints every key with its effective value, default, and
whether it's explicitly set. `--json` gives the same entries with typed
values.

| Key | Default | Constraint |
| --- | ---: | --- |
| `default_port` | `55000` | `1..=65535` |
| `lease_timeout` | `2m` | positive duration |
| `missing_for` | `7d` | non-negative duration |
| `cleanup_trigger_start` | `30` | `1..=100` directories |
| `cleanup_trigger_step` | `15` | `10..=15` |

Durations combine `s`, `m`, `h`, `d`, and `w` — `2d3h5m` is two days, three
hours, five minutes. The config file is sparse: `config set` writes only the
keys you've set, and everything else keeps its default.

## Development

Everything runs through mise:

```bash
mise install
mise run check     # fmt + clippy + tests + shellcheck
mise run smoke     # end-to-end CLI test against isolated state
```

The task graph also has `format`, `lint-stable` (Clippy on moving stable),
`build`, `install`, `audit`, and `package`. Tests isolate their state with
`PORTA_HOME` and include multi-process registry and configuration concurrency
coverage. [`mise.lock`](mise.lock) pins resolved tools for the supported
platforms.

## Design and prior work

- [Product and technical specification](SPEC.md) — the full CLI, JSON, and
  registry contract lives here, not in this README
- [Python prototype record](docs/python-prototype.md)
- [Related work and design influences](docs/related-work.md)

Port Authority is an independent open-source project and is not connected to
Akkio.

## License

Released under the [MIT License](LICENSE).
