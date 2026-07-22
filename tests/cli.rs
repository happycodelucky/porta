use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::net::{IpAddr, Ipv4Addr, SocketAddrV4, TcpListener};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use serde_json::Value;
use tempfile::TempDir;

static LEASE_RANGE_LOCK: Mutex<()> = Mutex::new(());

fn porta(state: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_porta"));
    command.env("PORTA_HOME", state);
    command
}

fn run(command: &mut Command) -> Output {
    let output = command.output().expect("porta should execute");
    assert!(
        output.status.success(),
        "porta failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn json_output(command: &mut Command) -> Value {
    let output = run(command);
    serde_json::from_slice(&output.stdout).expect("valid JSON output")
}

fn assert_json_envelope(value: &Value, expected_type: &str) {
    assert_eq!(value["version"], 1);
    assert_eq!(value["type"], expected_type);
    assert!(value.get("api_version").is_none());
    assert!(value.get("ok").is_none());
}

fn free_base() -> u16 {
    TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .expect("bind ephemeral port")
        .local_addr()
        .expect("ephemeral address")
        .port()
}

fn free_pair_from(start: u16) -> u16 {
    let selection = port_authority::socket::select_ports(start, 2, true, &HashSet::new())
        .expect("two adjacent free ports");
    selection.ports[0]
}

fn fixture() -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let state = temporary.path().join("state");
    let workspace = temporary.path().join("workspace");
    fs::create_dir(&workspace).expect("workspace directory");
    (temporary, state, workspace)
}

fn lease_and_bind(state: &Path) -> (u16, TcpListener) {
    for _ in 0..5 {
        let base = free_base();
        let lease = json_output(porta(state).args([
            "lease",
            "-p",
            &base.to_string(),
            "-t",
            "1s",
            "--json",
        ]));
        let port = u16::try_from(lease["port"].as_u64().expect("lease port")).expect("u16 port");
        if let Ok(listener) = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port)) {
            return (port, listener);
        }
    }
    panic!("could not bind an advisory leased port after five attempts");
}

#[test]
fn help_and_version_expose_the_contract() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let help = run(porta(temporary.path()).arg("--help"));
    let help = String::from_utf8(help.stdout).expect("UTF-8 help");
    let mut previous = 0;
    for command in [
        "lease",
        "reserve",
        "get",
        "list",
        "listeners",
        "info",
        "release",
        "clean",
        "config",
    ] {
        let marker = format!("\n  {command}");
        let position = help.find(&marker).expect("command in help");
        assert!(
            position >= previous,
            "commands should retain specification order"
        );
        previous = position;
    }
    let version = run(porta(temporary.path()).arg("--version"));
    assert!(String::from_utf8_lossy(&version.stdout).starts_with("porta 0.1.0"));

    let lease_help = run(porta(temporary.path()).args(["lease", "--help"]));
    let lease_help = String::from_utf8(lease_help.stdout).expect("UTF-8 lease help");
    assert!(lease_help.contains("-k, --key <KEY>"));

    let release_help = run(porta(temporary.path()).args(["release", "--help"]));
    let release_help = String::from_utf8(release_help.stdout).expect("UTF-8 release help");
    assert!(release_help.contains("-p, --port <PORT>"));
}

#[test]
fn json_responses_use_versioned_type_discriminators() {
    let (_temporary, state, workspace) = fixture();

    let config = json_output(porta(&state).args(["config", "--json"]));
    assert_json_envelope(&config, "config");
    let config_get = json_output(porta(&state).args(["config", "get", "default_port", "--json"]));
    assert_json_envelope(&config_get, "config_get");
    let config_set =
        json_output(porta(&state).args(["config", "set", "cleanup_trigger_start", "40", "--json"]));
    assert_json_envelope(&config_set, "config_set");

    let lease = json_output(porta(&state).args(["lease", "--json"]));
    assert_json_envelope(&lease, "lease");
    let reserve = json_output(porta(&state).arg("reserve").arg(&workspace).arg("--json"));
    assert_json_envelope(&reserve, "reserve");
    let get = json_output(porta(&state).arg("get").arg(&workspace).arg("--json"));
    assert_json_envelope(&get, "get");
    let reserved_port = get["port"].as_u64().expect("reserved port").to_string();
    let info = json_output(porta(&state).args(["info", &reserved_port, "--json"]));
    assert_json_envelope(&info, "info");
    let listeners = json_output(porta(&state).args(["listeners", &reserved_port, "--json"]));
    assert_json_envelope(&listeners, "listeners");
    let list = json_output(porta(&state).args(["list", "--json"]));
    assert_json_envelope(&list, "list");
    let clean = json_output(porta(&state).args(["clean", "--json"]));
    assert_json_envelope(&clean, "clean");
    let release = json_output(porta(&state).arg("release").arg(&workspace).arg("--json"));
    assert_json_envelope(&release, "release");

    let output = porta(&state)
        .args(["lease", "--port", "70000", "--json"])
        .output()
        .expect("porta should execute");
    assert_eq!(output.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&output.stdout).expect("valid JSON error");
    assert_json_envelope(&error, "error");
    assert_eq!(error["error"]["code"], "invalid_port");

    let output = porta(&state)
        .args(["unknown-command", "--json"])
        .output()
        .expect("porta should execute");
    assert_eq!(output.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&output.stdout).expect("valid JSON error");
    assert_json_envelope(&error, "error");
    assert_eq!(error["error"]["code"], "invalid_arguments");
}

#[test]
fn listeners_reports_a_filtered_tcp_listener_and_owning_process() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let listener =
        TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).expect("bind test listener");
    let port = listener.local_addr().expect("listener address").port();
    let pid = std::process::id();

    let json =
        json_output(porta(temporary.path()).args(["listeners", &port.to_string(), "--json"]));
    let listeners = json["listeners"].as_array().expect("listeners");
    let owner = listeners
        .iter()
        .find(|entry| entry["port"] == port && entry["pid"] == pid)
        .expect("test process owns the listener");
    assert_eq!(owner["address"], "127.0.0.1");
    assert!(!owner["process"].as_str().expect("process name").is_empty());

    let plain = run(porta(temporary.path()).args(["listeners", &port.to_string()]));
    let plain = String::from_utf8(plain.stdout).expect("UTF-8 listeners output");
    assert!(plain.contains("PORT"));
    assert!(plain.contains("ADDRESS"));
    assert!(plain.contains("PID"));
    assert!(plain.contains("PROCESS"));
    assert!(!plain.contains('|'));
    assert!(plain.contains(&port.to_string()));
    assert!(plain.contains(&pid.to_string()));

    drop(listener);
}

#[test]
fn listeners_is_registry_independent_and_reports_requested_ports_not_listening() {
    let listener =
        TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).expect("bind test listener");
    let listening_port = listener.local_addr().expect("listener address").port();
    let occupied: HashSet<u16> = listeners::get_all()
        .expect("inspect OS listeners")
        .into_iter()
        .filter(|listener| {
            listener.protocol == listeners::Protocol::TCP
                && listener.state == listeners::SocketState::Listen
        })
        .map(|listener| listener.socket.port())
        .collect();
    let missing_port = (1..1024)
        .find(|port| !occupied.contains(port))
        .expect("unused low port");

    let mut command = Command::new(env!("CARGO_BIN_EXE_porta"));
    command.env_remove("HOME").env_remove("PORTA_HOME").args([
        "listeners",
        &listening_port.to_string(),
        &listening_port.to_string(),
        &missing_port.to_string(),
        "--json",
    ]);
    let json = json_output(&mut command);
    assert_json_envelope(&json, "listeners");
    assert_eq!(json["missing_ports"], serde_json::json!([missing_port]));
    let owned = json["listeners"]
        .as_array()
        .expect("listeners")
        .iter()
        .any(|entry| entry["port"] == listening_port && entry["pid"] == std::process::id());
    assert!(owned);
}

fn parse_ip(value: &Value) -> IpAddr {
    value
        .as_str()
        .expect("address string")
        .parse()
        .expect("valid IP address")
}

#[test]
fn listeners_orders_rows_by_the_requested_column() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let listener =
        TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).expect("bind test listener");

    // Asserts the primary column is non-decreasing rather than pinning exact
    // rows, since the visible listeners depend on whatever the host is running.
    for (order, column) in [
        ("port", "port"),
        ("address", "address"),
        ("family", "family"),
        ("pid", "pid"),
        ("process", "process"),
    ] {
        let json = json_output(porta(temporary.path()).args(["listeners", "-o", order, "--json"]));
        let entries = json["listeners"].as_array().expect("listeners");
        assert!(
            entries.len() > 1,
            "host should expose several listeners to order"
        );
        for pair in entries.windows(2) {
            let (left, right) = (&pair[0][column], &pair[1][column]);
            let ordered = if column == "address" {
                // Addresses order numerically, so text comparison would be wrong.
                parse_ip(left) <= parse_ip(right)
            } else if let (Some(left), Some(right)) = (left.as_u64(), right.as_u64()) {
                left <= right
            } else {
                left.as_str().expect("column value") <= right.as_str().expect("column value")
            };
            assert!(ordered, "{order} ordering regressed: {left} then {right}");
        }
    }

    let output = porta(temporary.path())
        .args(["listeners", "-o", "hostname", "--json"])
        .output()
        .expect("porta should execute");
    assert_eq!(output.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&output.stdout).expect("valid JSON error");
    assert_json_envelope(&error, "error");
    assert_eq!(error["error"]["code"], "invalid_arguments");

    drop(listener);
}

#[test]
fn listeners_rejects_invalid_ports() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    for invalid in ["0", "65536"] {
        let output = porta(temporary.path())
            .args(["listeners", invalid, "--json"])
            .output()
            .expect("porta should execute");
        assert_eq!(output.status.code(), Some(2));
        let json: Value = serde_json::from_slice(&output.stdout).expect("valid JSON error");
        assert_json_envelope(&json, "error");
        assert_eq!(json["error"]["code"], "invalid_port");
    }
}

#[test]
fn keyed_reservation_workflow_supports_plain_and_json() {
    let (_temporary, state, workspace) = fixture();
    let base = free_base();
    let reserve = json_output(
        porta(&state)
            .args(["--json", "reserve", "-p", &base.to_string(), "-k", "web"])
            .args(["-k", "api"])
            .arg(&workspace),
    );
    assert_json_envelope(&reserve, "reserve");
    let web = reserve["keyed_ports"]["web"].as_u64().expect("web port");
    let api = reserve["keyed_ports"]["api"].as_u64().expect("api port");
    assert_ne!(web, api);

    let selected = run(porta(&state).args(["get", "-k", "web"]).arg(&workspace));
    assert_eq!(
        String::from_utf8_lossy(&selected.stdout).trim(),
        web.to_string()
    );

    let grouped = run(porta(&state).arg("list"));
    let grouped = String::from_utf8(grouped.stdout).expect("UTF-8 list output");
    assert!(grouped.contains("workspace"));
    assert!(grouped.contains("web"));
    assert!(grouped.contains("reserved"));
    assert!(!grouped.contains('|'));

    let info =
        json_output(porta(&state).args(["info", &web.to_string(), &api.to_string(), "--json"]));
    assert_eq!(info["ports"].as_array().expect("ports").len(), 2);
    assert_eq!(info["ports"][0]["state"], "reserved");

    let listed = json_output(porta(&state).args(["list", "--json"]));
    assert_eq!(
        listed["reservations"]
            .as_array()
            .expect("reservations")
            .len(),
        1
    );

    let released = json_output(
        porta(&state)
            .args(["release", "-k", "api", "--json"])
            .arg(&workspace),
    );
    assert_eq!(released["released"], 1);
    run(porta(&state).arg("release").arg(&workspace));
    let empty = run(porta(&state).arg("list"));
    assert_eq!(
        String::from_utf8_lossy(&empty.stdout).trim(),
        "No reservations or leases."
    );
}

#[test]
fn release_by_port_is_repeatable_and_ignores_directory_ownership() {
    let (temporary, state, first_workspace) = fixture();
    let second_workspace = temporary.path().join("second-workspace");
    fs::create_dir(&second_workspace).expect("second workspace");
    let base = free_base();

    let first = json_output(
        porta(&state)
            .args(["reserve", "-p", &base.to_string(), "-k", "web"])
            .args(["-k", "api", "--json"])
            .arg(&first_workspace),
    );
    let second = json_output(
        porta(&state)
            .args(["reserve", "-p", &base.to_string(), "-k", "web"])
            .args(["-k", "api", "--json"])
            .arg(&second_workspace),
    );
    let first_web = first["keyed_ports"]["web"].as_u64().expect("first web");
    let second_api = second["keyed_ports"]["api"].as_u64().expect("second api");
    let first_api = first["keyed_ports"]["api"].as_u64().expect("first api");
    let second_web = second["keyed_ports"]["web"].as_u64().expect("second web");

    let released = json_output(
        porta(&state)
            .current_dir(&second_workspace)
            .args(["release", "-p", &first_web.to_string()])
            .args(["--port", &second_api.to_string(), "--json"]),
    );
    assert_eq!(released["released"], 2);
    assert_eq!(
        released["ports"],
        serde_json::json!([first_web, second_api])
    );
    assert!(released.get("directory").is_none());

    let listed = json_output(porta(&state).args(["list", "--json"]));
    assert_eq!(
        listed["reservations"]
            .as_array()
            .expect("reservations")
            .len(),
        2
    );
    let registered: HashSet<u64> = listed["reservations"]
        .as_array()
        .expect("reservations")
        .iter()
        .flat_map(|reservation| {
            reservation["allocations"]
                .as_array()
                .expect("allocations")
                .iter()
                .map(|allocation| allocation["port"].as_u64().expect("allocation port"))
        })
        .collect();
    assert!(!registered.contains(&first_web));
    assert!(!registered.contains(&second_api));
    assert!(registered.contains(&first_api));
    assert!(registered.contains(&second_web));

    run(porta(&state).args([
        "release",
        "-p",
        &first_api.to_string(),
        "-p",
        &second_web.to_string(),
    ]));
    let empty = json_output(porta(&state).args(["list", "--json"]));
    assert!(
        empty["reservations"]
            .as_array()
            .expect("reservations")
            .is_empty()
    );
}

#[test]
fn release_by_port_removes_leases_and_preserves_plain_output() {
    let (_temporary, state, _workspace) = fixture();
    let base = free_base();
    let lease = json_output(porta(&state).args([
        "lease",
        "-p",
        &base.to_string(),
        "-k",
        "preview",
        "--json",
    ]));
    let port = lease["port"].as_u64().expect("lease port");

    let released = run(porta(&state).args(["release", "-p", &port.to_string()]));
    assert_eq!(
        String::from_utf8_lossy(&released.stdout),
        format!("Released {port}\n")
    );
    let listed = json_output(porta(&state).args(["list", "--json"]));
    assert!(listed["leases"].as_array().expect("leases").is_empty());
}

#[test]
fn release_by_port_is_atomic_when_a_port_is_missing() {
    let (_temporary, state, workspace) = fixture();
    let reservation = json_output(
        porta(&state)
            .args(["reserve", "-k", "web", "--json"])
            .arg(&workspace),
    );
    let port = reservation["ports"][0].as_u64().expect("reserved port");
    let missing = if port == 65_535 { 1 } else { 65_535 };

    let duplicate = porta(&state)
        .args([
            "release",
            "-p",
            &port.to_string(),
            "-p",
            &port.to_string(),
            "--json",
        ])
        .output()
        .expect("porta should execute");
    assert_eq!(duplicate.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&duplicate.stdout).expect("valid JSON error");
    assert_eq!(error["error"]["code"], "duplicate_port");

    let output = porta(&state)
        .args([
            "release",
            "-p",
            &port.to_string(),
            "-p",
            &missing.to_string(),
            "--json",
        ])
        .output()
        .expect("porta should execute");
    assert_eq!(output.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&output.stdout).expect("valid JSON error");
    assert_eq!(error["error"]["code"], "port_not_found");

    let info = json_output(porta(&state).args(["info", &port.to_string(), "--json"]));
    assert_eq!(info["ports"][0]["state"], "reserved");
}

#[test]
fn release_by_port_rejects_directory_and_key_scopes() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let commands = [
        vec!["release", "-p", "6008", "-k", "web", "--json"],
        vec!["release", "-p", "6008", ".", "--json"],
    ];
    for arguments in commands {
        let output = porta(temporary.path())
            .args(arguments)
            .output()
            .expect("porta should execute");
        assert_eq!(output.status.code(), Some(2));
        let error: Value = serde_json::from_slice(&output.stdout).expect("valid JSON error");
        assert_eq!(error["error"]["code"], "invalid_arguments");
    }
}

#[test]
fn config_and_missing_directory_cleanup_are_persistent() {
    let (_temporary, state, workspace) = fixture();
    run(porta(&state).args(["config", "set", "missing_for", "0s"]));
    let configured = run(porta(&state).args(["config", "get", "missing_for"]));
    assert_eq!(String::from_utf8_lossy(&configured.stdout).trim(), "0s");

    run(porta(&state).arg("reserve").arg(&workspace));
    fs::remove_dir(&workspace).expect("remove disposable workspace");
    let first = json_output(porta(&state).args(["clean", "--json"]));
    assert_eq!(first["marked_missing"], 1);
    assert_eq!(first["reaped"], 0);
    let second = json_output(porta(&state).args(["clean", "--json"]));
    assert_eq!(second["reaped"], 1);
}

#[test]
fn bare_config_lists_effective_defaults_and_explicit_settings() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let state = temporary.path().join("state");

    let defaults = json_output(porta(&state).args(["config", "--json"]));
    assert_eq!(
        defaults["settings"],
        serde_json::json!([
            {"key": "default_port", "value": 55000, "default": 55000, "is_set": false},
            {"key": "lease_timeout", "value": "2m", "default": "2m", "is_set": false},
            {"key": "missing_for", "value": "1w", "default": "1w", "is_set": false},
            {"key": "cleanup_trigger_start", "value": 30, "default": 30, "is_set": false},
            {"key": "cleanup_trigger_step", "value": 15, "default": 15, "is_set": false},
        ])
    );

    run(porta(&state).args(["config", "set", "missing_for", "1d"]));
    run(porta(&state).args(["config", "set", "default_port", "55000"]));
    let configured = json_output(porta(&state).args(["config", "--json"]));
    assert_eq!(configured["settings"][0]["is_set"], true);
    assert_eq!(configured["settings"][2]["value"], "1d");
    assert_eq!(configured["settings"][2]["is_set"], true);
    assert_eq!(configured["settings"][1]["is_set"], false);

    let plain = run(porta(&state).arg("config"));
    let plain = String::from_utf8(plain.stdout).expect("UTF-8 config output");
    for header in ["KEY", "EFFECTIVE", "DEFAULT", "STATUS"] {
        assert!(plain.contains(header));
    }
    assert!(plain.contains("default_port"));
    assert!(plain.contains("missing_for"));
    assert!(plain.contains("set"));
    assert!(plain.contains("default"));
}

#[test]
fn lease_reports_expired_in_use_then_reaps() {
    let (_temporary, state, _workspace) = fixture();
    let (port, listener) = lease_and_bind(&state);
    thread::sleep(Duration::from_millis(1_100));
    let in_use = json_output(porta(&state).args(["info", &port.to_string(), "--json"]));
    assert_eq!(in_use["ports"][0]["state"], "expired_in_use");
    drop(listener);
    let available = json_output(porta(&state).args(["info", &port.to_string(), "--json"]));
    assert_eq!(available["ports"][0]["state"], "available");
}

#[test]
fn list_groups_leases_and_localizes_local_times() {
    let (temporary, state, _workspace) = fixture();
    fs::create_dir(&state).expect("state directory");
    fs::write(
        state.join("registry.json"),
        r#"{
  "schema_version": 2,
  "maintenance": {"cleanup_trigger": 30},
  "open_leases": [{
    "id": "28e5f42b-d26d-4974-9db9-b895433a94ef",
    "port": 62000,
    "key": "preview",
    "created_at": "2030-01-15T20:00:00Z",
    "expires_at": "2030-01-15T20:30:00Z"
  }],
  "reservations": []
}"#,
    )
    .expect("registry fixture");

    let output = run(porta(&state)
        .arg("list")
        .env("TZ", "America/Los_Angeles")
        .env("LANG", "en_US.UTF-8")
        .env("LC_ALL", "en_US.UTF-8"));
    let output = String::from_utf8(output.stdout).expect("UTF-8 list output");

    assert!(output.contains("leases"));
    assert!(output.contains("preview"));
    assert!(output.contains("62000"));
    assert!(output.contains("leased"));
    assert!(output.contains("expires"));
    assert!(!output.contains('|'));
    assert!(output.contains("12:30:00"));
    assert!(output.contains("-08:00"));
    assert!(!output.contains("2030-01-15T20:30:00Z"));

    let json = json_output(porta(&state).args(["list", "--json"]));
    assert_eq!(json["leases"][0]["expires_at"], "2030-01-15T20:30:00Z");

    drop(temporary);
}

#[test]
fn lease_skips_an_unexpired_lease_from_the_same_base() {
    let _range_guard = LEASE_RANGE_LOCK.lock().expect("lease range lock");
    let (_temporary, state, _workspace) = fixture();
    let base = free_pair_from(6_008);
    let first = run(porta(&state).args(["lease", "-p", &base.to_string(), "-t", "10s"]));
    let second = run(porta(&state).args(["lease", "-p", &base.to_string(), "-t", "10s"]));

    assert_eq!(
        String::from_utf8_lossy(&first.stdout).trim(),
        base.to_string()
    );
    assert_eq!(
        String::from_utf8_lossy(&second.stdout).trim(),
        (base + 1).to_string()
    );

    let listed = json_output(porta(&state).args(["list", "--json"]));
    assert_eq!(listed["leases"].as_array().expect("leases").len(), 2);
}

#[test]
fn keyed_lease_renews_the_same_identity_and_remains_visible() {
    let _range_guard = LEASE_RANGE_LOCK.lock().expect("lease range lock");
    let (_temporary, state, _workspace) = fixture();
    let base = free_pair_from(6_008);
    let first = json_output(porta(&state).args([
        "lease",
        "-p",
        &base.to_string(),
        "-t",
        "10s",
        "-k",
        "preview",
        "--json",
    ]));
    let unkeyed =
        json_output(porta(&state).args(["lease", "-p", &base.to_string(), "-t", "10s", "--json"]));
    thread::sleep(Duration::from_millis(20));
    let renewed = json_output(
        porta(&state).args(["lease", "-p", "1", "-t", "1h", "-k", "preview", "--json"]),
    );

    assert_eq!(first["port"], base);
    assert_eq!(first["key"], "preview");
    assert_eq!(unkeyed["port"], base + 1);
    assert!(unkeyed["key"].is_null());
    assert_eq!(renewed["port"], first["port"]);
    assert_eq!(renewed["lease_id"], first["lease_id"]);
    assert_eq!(renewed["created_at"], first["created_at"]);
    assert_ne!(renewed["expires_at"], first["expires_at"]);

    let info = json_output(porta(&state).args(["info", &base.to_string(), "--json"]));
    assert_eq!(info["ports"][0]["key"], "preview");
    let listed = json_output(porta(&state).args(["list", "--json"]));
    assert_eq!(listed["leases"].as_array().expect("leases").len(), 2);
    assert_eq!(listed["leases"][0]["key"], "preview");

    let plain = run(porta(&state).args(["lease", "-k", "preview"]));
    assert_eq!(
        String::from_utf8_lossy(&plain.stdout).trim(),
        base.to_string()
    );
}

#[test]
fn concurrent_keyed_lease_calls_converge_on_one_lease() {
    let _range_guard = LEASE_RANGE_LOCK.lock().expect("lease range lock");
    let temporary = tempfile::tempdir().expect("temporary directory");
    let state = temporary.path().join("state");
    let base = free_pair_from(6_008);
    let mut children = Vec::new();
    for _ in 0..8 {
        children.push(
            porta(&state)
                .args([
                    "lease",
                    "-p",
                    &base.to_string(),
                    "-t",
                    "10s",
                    "-k",
                    "shared",
                    "--json",
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn keyed lease"),
        );
    }

    let mut ports = HashSet::new();
    let mut ids = HashSet::new();
    for child in children {
        let output = child.wait_with_output().expect("wait for keyed lease");
        assert!(output.status.success(), "keyed lease should succeed");
        let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON output");
        ports.insert(value["port"].as_u64().expect("port"));
        ids.insert(value["lease_id"].as_str().expect("lease ID").to_owned());
    }
    assert_eq!(ports.len(), 1);
    assert_eq!(ids.len(), 1);

    let listed = json_output(porta(&state).args(["list", "--json"]));
    assert_eq!(listed["leases"].as_array().expect("leases").len(), 1);
    assert_eq!(listed["leases"][0]["key"], "shared");
}

#[test]
fn concurrent_processes_do_not_receive_duplicate_ports() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let state = temporary.path().join("state");
    let base = free_base();
    let mut children = Vec::new();
    for index in 0..12 {
        let workspace = temporary.path().join(format!("workspace-{index}"));
        fs::create_dir(&workspace).expect("workspace directory");
        children.push(
            porta(&state)
                .args(["reserve", "-p", &base.to_string(), "-k", "web", "--json"])
                .arg(workspace)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn porta"),
        );
    }

    let mut ports = HashSet::new();
    for child in children {
        let output = child.wait_with_output().expect("wait for porta");
        assert!(
            output.status.success(),
            "concurrent reservation should succeed"
        );
        let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON output");
        let port = value["keyed_ports"]["web"].as_u64().expect("web port");
        assert!(ports.insert(port), "ports must be unique across processes");
    }
    assert_eq!(ports.len(), 12);
}

#[test]
fn contiguous_and_expiring_reservations_follow_short_options() {
    let (_temporary, state, workspace) = fixture();
    let base = free_base();
    let result = json_output(
        porta(&state)
            .args([
                "reserve",
                "-p",
                &base.to_string(),
                "-k",
                "web",
                "-k",
                "api",
                "-k",
                "worker",
                "-c",
                "-e",
                "1s",
                "--json",
            ])
            .arg(&workspace),
    );
    let ports = result["ports"].as_array().expect("ports");
    assert_eq!(ports.len(), 3);
    assert_eq!(ports[1].as_u64(), ports[0].as_u64().map(|port| port + 1));
    assert_eq!(ports[2].as_u64(), ports[1].as_u64().map(|port| port + 1));

    thread::sleep(Duration::from_millis(1_100));
    let output = porta(&state)
        .args(["get", "-k", "web", "--json"])
        .arg(&workspace)
        .output()
        .expect("porta should execute");
    assert_eq!(output.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&output.stdout).expect("valid JSON error");
    assert_eq!(error["error"]["code"], "reservation_not_found");
}

#[test]
fn invalid_ports_have_the_documented_structured_error() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    for arguments in [
        ["lease", "--port", "70000", "--json"],
        ["release", "--port", "70000", "--json"],
    ] {
        let output = porta(temporary.path())
            .args(arguments)
            .output()
            .expect("porta should execute");
        assert_eq!(output.status.code(), Some(2));
        let error: Value = serde_json::from_slice(&output.stdout).expect("valid JSON error");
        assert_json_envelope(&error, "error");
        assert_eq!(error["error"]["code"], "invalid_port");
    }
}

#[test]
fn concurrent_config_setters_preserve_distinct_updates() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let state = temporary.path().join("state");
    let mut first = porta(&state)
        .args(["config", "set", "default_port", "56000", "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn first setter");
    let mut second = porta(&state)
        .args(["config", "set", "cleanup_trigger_start", "40", "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn second setter");
    assert!(first.wait().expect("first setter").success());
    assert!(second.wait().expect("second setter").success());

    let port = run(porta(&state).args(["config", "get", "default_port"]));
    let trigger = run(porta(&state).args(["config", "get", "cleanup_trigger_start"]));
    assert_eq!(String::from_utf8_lossy(&port.stdout).trim(), "56000");
    assert_eq!(String::from_utf8_lossy(&trigger.stdout).trim(), "40");

    let settings = json_output(porta(&state).args(["config", "--json"]));
    let settings = settings["settings"].as_array().expect("config settings");
    assert!(settings[0]["is_set"].as_bool().expect("default port set"));
    assert!(settings[3]["is_set"].as_bool().expect("trigger set"));
    assert!(
        settings
            .iter()
            .enumerate()
            .all(|(index, setting)| matches!(index, 0 | 3)
                || !setting["is_set"].as_bool().expect("setting status"))
    );
}

#[test]
fn lock_holder_helper() {
    let Some(lock_path) = std::env::var_os("PORTA_TEST_HOLD_LOCK") else {
        return;
    };
    let marker = std::env::var_os("PORTA_TEST_LOCK_READY").expect("ready marker");
    let lock_path = std::path::PathBuf::from(lock_path);
    fs::create_dir_all(lock_path.parent().expect("lock parent")).expect("state directory");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .expect("lock file");
    file.lock().expect("exclusive lock");
    fs::write(marker, b"ready").expect("ready marker");
    thread::sleep(Duration::from_mins(1));
}

#[test]
fn lock_contention_is_bounded_and_process_death_releases_the_lock() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let state = temporary.path().join("state");
    let lock = state.join("registry.lock");
    let marker = temporary.path().join("lock-ready");
    let mut holder = Command::new(std::env::current_exe().expect("test executable"))
        .args(["--exact", "lock_holder_helper", "--nocapture"])
        .env("PORTA_TEST_HOLD_LOCK", &lock)
        .env("PORTA_TEST_LOCK_READY", &marker)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn lock holder");

    for _ in 0..100 {
        if marker.exists() {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    if !marker.exists() {
        let _ = holder.kill();
        let _ = holder.wait();
        panic!("lock holder did not become ready");
    }

    let blocked = porta(&state)
        .args(["lease", "--json"])
        .output()
        .expect("blocked porta should execute");
    holder.kill().expect("terminate lock holder");
    holder.wait().expect("reap lock holder");

    assert_eq!(blocked.status.code(), Some(3));
    let error: Value = serde_json::from_slice(&blocked.stdout).expect("valid JSON error");
    assert_eq!(error["error"]["code"], "registry_busy");

    let recovered = json_output(porta(&state).args(["lease", "--json"]));
    assert_json_envelope(&recovered, "lease");
}
