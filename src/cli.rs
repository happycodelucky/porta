use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use chrono::{DateTime, FixedOffset, Local, Locale, Utc};
use clap::error::ErrorKind;
use clap::{Args, CommandFactory, Parser, Subcommand};
use comfy_table::Table;
use serde::Serialize;
use serde_json::{Value, json};

use crate::RESPONSE_VERSION;
use crate::config::ConfigEntry;
use crate::duration::format_timestamp;
use crate::error::{PortaError, Result};
use crate::listeners::{ListenerEntry, ListenerOrder, ListenersResult};
use crate::service::{ListResult, PortInfo, PortaService, ReserveRequest};

#[derive(Debug, Parser)]
#[command(
    name = "porta",
    version,
    about = "Reserve and lease local TCP ports so parallel worktrees, agents, and dev servers stop colliding",
    disable_help_subcommand = true
)]
struct Cli {
    #[arg(short = 'j', long, global = true, help = "Emit machine-readable JSON")]
    json: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(about = "Lease an open port without a directory")]
    Lease(LeaseArgs),
    #[command(about = "Reserve ports for a directory")]
    Reserve(ReserveArgs),
    #[command(about = "Return a reserved port for a directory")]
    Get(GetArgs),
    #[command(about = "List directory reservations and leases")]
    List,
    #[command(about = "Show OS TCP listeners and owning processes")]
    Listeners(ListenersArgs),
    #[command(about = "Show registry or socket information for ports")]
    Info(InfoArgs),
    #[command(about = "Release reserved ports or leases")]
    Release(ReleaseArgs),
    #[command(about = "Sweep and clean the registry")]
    Clean,
    #[command(about = "List, read, or update configuration")]
    Config(ConfigArgs),
}

#[derive(Debug, Args)]
struct LeaseArgs {
    #[arg(short = 'p', long, value_name = "PORT", help = "Preferred base port")]
    port: Option<u32>,
    #[arg(
        short = 't',
        long,
        value_name = "DURATION",
        help = "Lease lifetime (default: 2m)"
    )]
    timeout: Option<String>,
    #[arg(
        short = 'k',
        long,
        value_name = "KEY",
        help = "Lease identity to allocate or renew"
    )]
    key: Option<String>,
}

#[derive(Debug, Args)]
struct ReserveArgs {
    #[arg(short = 'p', long, value_name = "PORT", help = "Preferred base port")]
    port: Option<u32>,
    #[arg(
        short = 'k',
        long,
        value_name = "KEY",
        action = clap::ArgAction::Append,
        help = "Allocation key; repeatable"
    )]
    key: Vec<String>,
    #[arg(short = 'c', long, help = "Require new allocations to be contiguous")]
    contiguous: bool,
    #[arg(
        short = 'e',
        long,
        value_name = "WHEN",
        help = "Reservation duration or RFC 3339 expiration"
    )]
    expires: Option<String>,
    #[arg(default_value = ".", help = "Reservation directory (default: cwd)")]
    directory: PathBuf,
}

#[derive(Debug, Args)]
struct ReleaseArgs {
    #[arg(
        short = 'p',
        long,
        value_name = "PORT",
        action = clap::ArgAction::Append,
        conflicts_with_all = ["key", "directory"],
        help = "Exact registry port to release; repeatable"
    )]
    port: Vec<u32>,
    #[arg(
        short = 'k',
        long,
        value_name = "KEY",
        action = clap::ArgAction::Append,
        conflicts_with = "port",
        help = "Allocation key to release; repeatable"
    )]
    key: Vec<String>,
    #[arg(conflicts_with = "port", help = "Reservation directory (default: cwd)")]
    directory: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct GetArgs {
    #[arg(short = 'k', long, value_name = "KEY", help = "Allocation key")]
    key: Option<String>,
    #[arg(default_value = ".", help = "Reservation directory (default: cwd)")]
    directory: PathBuf,
}

#[derive(Debug, Args)]
struct InfoArgs {
    #[arg(required = true, num_args = 1.., value_name = "PORT")]
    ports: Vec<u32>,
}

#[derive(Debug, Args)]
struct ListenersArgs {
    #[arg(num_args = 0.., value_name = "PORT", help = "Optional ports to inspect")]
    ports: Vec<u32>,
    #[arg(
        short = 'o',
        long,
        value_enum,
        value_name = "COLUMN",
        default_value = "port",
        help = "Order rows by column"
    )]
    order: ListenerOrder,
}

#[derive(Debug, Args)]
struct ConfigArgs {
    #[command(subcommand)]
    action: Option<ConfigAction>,
}

#[derive(Debug, Subcommand)]
enum ConfigAction {
    #[command(about = "Read one effective configuration value")]
    Get { key: String },
    #[command(about = "Set one configuration value")]
    Set { key: String, value: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResponseType {
    Lease,
    Reserve,
    Release,
    Get,
    Info,
    Listeners,
    List,
    Clean,
    Config,
    ConfigGet,
    ConfigSet,
    Error,
}

impl ResponseType {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Lease => "lease",
            Self::Reserve => "reserve",
            Self::Release => "release",
            Self::Get => "get",
            Self::Info => "info",
            Self::Listeners => "listeners",
            Self::List => "list",
            Self::Clean => "clean",
            Self::Config => "config",
            Self::ConfigGet => "config_get",
            Self::ConfigSet => "config_set",
            Self::Error => "error",
        }
    }
}

#[must_use]
pub fn main() -> ExitCode {
    let arguments: Vec<OsString> = std::env::args_os().collect();
    let json_requested = arguments
        .iter()
        .any(|argument| argument == "--json" || argument == "-j");
    let cli = match Cli::try_parse_from(&arguments) {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            let _ = error.print();
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            let failure = PortaError::invalid("invalid_arguments", error.to_string());
            emit_error(&failure, json_requested);
            return ExitCode::from(failure.exit_code);
        }
    };

    let Some(command) = cli.command else {
        let mut command = Cli::command();
        let _ = command.print_help();
        println!();
        return ExitCode::SUCCESS;
    };

    let command = match command {
        Command::Listeners(arguments) => {
            return finish(execute_listeners(arguments, cli.json), cli.json);
        }
        command => command,
    };

    let service = match PortaService::discover() {
        Ok(service) => service,
        Err(error) => {
            emit_error(&error, cli.json);
            return ExitCode::from(error.exit_code);
        }
    };
    finish(execute(&service, command, cli.json), cli.json)
}

fn finish(result: Result<()>, json_output: bool) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            emit_error(&error, json_output);
            ExitCode::from(error.exit_code)
        }
    }
}

fn execute_listeners(arguments: ListenersArgs, json_output: bool) -> Result<()> {
    let ports = arguments
        .ports
        .into_iter()
        .map(valid_port)
        .collect::<Result<Vec<_>>>()?;
    let result = crate::listeners::inspect(&ports, arguments.order)?;
    if json_output {
        emit_success(ResponseType::Listeners, &result)
    } else {
        print_listeners(&result);
        Ok(())
    }
}

#[allow(clippy::too_many_lines)]
fn execute(service: &PortaService, command: Command, json_output: bool) -> Result<()> {
    match command {
        Command::Lease(arguments) => {
            let result = service.lease(
                arguments.port.map(valid_port).transpose()?,
                arguments.timeout.as_deref(),
                arguments.key.as_deref(),
            )?;
            if json_output {
                emit_success(ResponseType::Lease, &result)?;
            } else {
                println!("{}", result.port);
            }
        }
        Command::Reserve(arguments) => {
            let result = service.reserve(&ReserveRequest {
                directory: arguments.directory,
                base: arguments.port.map(valid_port).transpose()?,
                keys: arguments.key,
                contiguous: arguments.contiguous,
                expires: arguments.expires,
            })?;
            if json_output {
                emit_success(ResponseType::Reserve, &result)?;
            } else if result.allocations.iter().any(|item| item.key.is_some()) {
                println!(
                    "{}",
                    result
                        .allocations
                        .iter()
                        .map(|item| format!("{}={}", item.key.as_deref().unwrap_or("-"), item.port))
                        .collect::<Vec<_>>()
                        .join(" ")
                );
            } else {
                println!(
                    "{}",
                    result
                        .ports
                        .iter()
                        .map(u16::to_string)
                        .collect::<Vec<_>>()
                        .join(" ")
                );
            }
        }
        Command::Release(arguments) => {
            if arguments.port.is_empty() {
                let directory = arguments.directory.unwrap_or_else(|| PathBuf::from("."));
                let result = service.release(&directory, &arguments.key)?;
                if json_output {
                    emit_success(ResponseType::Release, &result)?;
                } else {
                    print_released(&result.ports);
                }
            } else {
                let ports = arguments
                    .port
                    .into_iter()
                    .map(valid_port)
                    .collect::<Result<Vec<_>>>()?;
                let result = service.release_ports(&ports)?;
                if json_output {
                    emit_success(ResponseType::Release, &result)?;
                } else {
                    print_released(&result.ports);
                }
            }
        }
        Command::Get(arguments) => {
            let result = service.get(&arguments.directory, arguments.key.as_deref())?;
            if json_output {
                emit_success(ResponseType::Get, &result)?;
            } else {
                println!("{}", result.port);
            }
        }
        Command::Info(arguments) => {
            let ports = arguments
                .ports
                .into_iter()
                .map(valid_port)
                .collect::<Result<Vec<_>>>()?;
            let ports = service.info(&ports)?;
            if json_output {
                emit_success(ResponseType::Info, &json!({ "ports": ports }))?;
            } else {
                for port in ports {
                    println!("{}", format_port_info(&port)?);
                }
            }
        }
        Command::Listeners(_) => unreachable!("listeners is executed before state discovery"),
        Command::List => {
            let result = service.list()?;
            if json_output {
                emit_success(ResponseType::List, &result)?;
            } else if result.reservations.is_empty() && result.leases.is_empty() {
                println!("No reservations or leases.");
            } else {
                println!("{}", format_list(&result)?);
            }
        }
        Command::Clean => {
            let result = service.clean()?;
            if json_output {
                emit_success(ResponseType::Clean, &result)?;
            } else {
                println!(
                    "Released leases: {}; expired reservations: {}; marked missing: {}; restored: {}; reaped: {}; next automatic cleanup: {}",
                    result.released_leases,
                    result.expired_reservations,
                    result.marked_missing,
                    result.restored,
                    result.reaped,
                    result.cleanup_trigger
                );
            }
        }
        Command::Config(arguments) => match arguments.action {
            None => {
                let settings = service.config_list()?;
                if json_output {
                    emit_success(ResponseType::Config, &json!({ "settings": settings }))?;
                } else {
                    println!("{}", format_config_table(&settings));
                }
            }
            Some(ConfigAction::Get { key }) => {
                let value = service.config_get(&key)?;
                if json_output {
                    emit_success(
                        ResponseType::ConfigGet,
                        &json!({ "key": key, "value": value.json() }),
                    )?;
                } else {
                    println!("{}", value.plain());
                }
            }
            Some(ConfigAction::Set { key, value }) => {
                let value = service.config_set(&key, &value)?;
                if json_output {
                    emit_success(
                        ResponseType::ConfigSet,
                        &json!({ "key": key, "value": value.json() }),
                    )?;
                } else {
                    println!("{}={}", key, value.plain());
                }
            }
        },
    }
    Ok(())
}

fn format_config_table(settings: &[ConfigEntry]) -> String {
    let mut table = borderless_table(&["KEY", "EFFECTIVE", "DEFAULT", "STATUS"]);
    for setting in settings {
        table.add_row([
            setting.key.to_owned(),
            setting.value.plain(),
            setting.default.plain(),
            if setting.is_set { "set" } else { "unset" }.to_owned(),
        ]);
    }
    render_columns(&mut table)
}

fn borderless_table(headers: &[&str]) -> Table {
    let mut table = Table::new();
    table.load_preset(comfy_table::presets::NOTHING);
    if !headers.is_empty() {
        table.set_header(headers.to_vec());
    }
    table
}

fn render_columns(table: &mut Table) -> String {
    let last = table.column_iter().count().saturating_sub(1);
    for (index, column) in table.column_iter_mut().enumerate() {
        column.set_padding(if index == last { (0, 0) } else { (0, 2) });
    }
    table
        .to_string()
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_port_info(info: &PortInfo) -> Result<String> {
    let key = info.key.as_deref().unwrap_or("-");
    let directory = info
        .directory
        .as_ref()
        .map_or_else(|| "-".to_owned(), |path| path.display().to_string());
    let expires = info
        .expires_at
        .map(format_timestamp)
        .transpose()?
        .unwrap_or_else(|| "-".to_owned());
    Ok(format!(
        "{}\t{}\t{}\t{}\t{}\t{}",
        info.port, info.state, info.in_use, key, directory, expires
    ))
}

fn print_released(ports: &[u16]) {
    println!(
        "Released {}",
        ports
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(" ")
    );
}

fn print_listeners(result: &ListenersResult) {
    if result.listeners.is_empty() {
        if result.missing_ports.is_empty() {
            println!("No TCP listeners found.");
        } else {
            println!(
                "No TCP listeners found for: {}",
                format_ports(&result.missing_ports)
            );
        }
        return;
    }

    println!("{}", format_listeners_table(&result.listeners));
    if !result.missing_ports.is_empty() {
        println!("Not listening: {}", format_ports(&result.missing_ports));
    }
}

fn format_listeners_table(entries: &[ListenerEntry]) -> String {
    let mut table = borderless_table(&["PORT", "ADDRESS", "FAMILY", "PID", "PROCESS"]);
    for entry in entries {
        table.add_row([
            entry.port.to_string(),
            entry.address.to_string(),
            match entry.family {
                crate::listeners::AddressFamily::Ipv4 => "IPv4".to_owned(),
                crate::listeners::AddressFamily::Ipv6 => "IPv6".to_owned(),
            },
            entry.pid.to_string(),
            entry.process.clone(),
        ]);
    }
    render_columns(&mut table)
}

fn format_ports(ports: &[u16]) -> String {
    ports
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_list(result: &ListResult) -> Result<String> {
    let mut sections = Vec::with_capacity(result.reservations.len() + 1);
    for reservation in &result.reservations {
        let mut header = reservation.directory.display().to_string();
        let mut notes = Vec::new();
        if reservation.status == "missing" {
            notes.push("missing".to_owned());
        }
        if let Some(expires_at) = reservation.expires_at {
            notes.push(format!("expires {}", format_local_timestamp(expires_at)?));
        }
        if !notes.is_empty() {
            header = format!("{header}  ({})", notes.join(", "));
        }
        let mut table = borderless_table(&[]);
        for allocation in &reservation.allocations {
            table.add_row([
                allocation.key.as_deref().unwrap_or("-").to_owned(),
                allocation.port.to_string(),
                "reserved".to_owned(),
                port_notes(result, allocation.port, None)?,
            ]);
        }
        sections.push(format!("{header}\n{}", indent(&render_columns(&mut table))));
    }
    if !result.leases.is_empty() {
        let mut table = borderless_table(&[]);
        for lease in &result.leases {
            table.add_row([
                lease.key.as_deref().unwrap_or("-").to_owned(),
                lease.port.to_string(),
                lease.status.clone(),
                port_notes(result, lease.port, Some(lease.expires_at))?,
            ]);
        }
        sections.push(format!("leases\n{}", indent(&render_columns(&mut table))));
    }
    Ok(sections.join("\n\n"))
}

fn port_notes(
    result: &ListResult,
    port: u16,
    expires_at: Option<time::OffsetDateTime>,
) -> Result<String> {
    let mut notes = Vec::new();
    if result.in_use.get(&port).copied().unwrap_or(false) {
        notes.push("in use".to_owned());
    }
    if let Some(expires_at) = expires_at {
        notes.push(format!("expires {}", format_local_timestamp(expires_at)?));
    }
    Ok(notes.join(", "))
}

fn indent(text: &str) -> String {
    text.lines()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_local_timestamp(value: time::OffsetDateTime) -> Result<String> {
    let utc = DateTime::<Utc>::from_timestamp(value.unix_timestamp(), value.nanosecond())
        .ok_or_else(|| {
            PortaError::infrastructure("timestamp_failed", "timestamp is outside the local range")
        })?;
    let local = utc.with_timezone(&Local).fixed_offset();
    Ok(format_localized_datetime(local, system_locale()))
}

fn format_localized_datetime(value: DateTime<FixedOffset>, locale: Locale) -> String {
    value.format_localized("%x %X %:z", locale).to_string()
}

fn system_locale() -> Locale {
    sys_locale::get_locales()
        .find_map(|locale| parse_locale(&locale))
        .unwrap_or_else(|| resolve_locale(None))
}

fn resolve_locale(locale: Option<&str>) -> Locale {
    locale.and_then(parse_locale).unwrap_or(Locale::en_US)
}

fn parse_locale(locale: &str) -> Option<Locale> {
    let locale = locale.trim();
    if locale.eq_ignore_ascii_case("C") || locale.eq_ignore_ascii_case("POSIX") {
        return Some(Locale::POSIX);
    }

    let base = locale.split('.').next().unwrap_or(locale);
    let normalized = base.replace(['-', '@'], "_");
    Locale::try_from(normalized.as_str()).ok().or_else(|| {
        let mut components = normalized.split('_');
        let language = components.next()?;
        let territory = components.next()?;
        let language_territory = format!("{language}_{territory}");
        Locale::try_from(language_territory.as_str()).ok()
    })
}

fn valid_port(value: u32) -> Result<u16> {
    let port = u16::try_from(value)
        .map_err(|_| PortaError::invalid("invalid_port", "Port must be between 1 and 65535"))?;
    if port == 0 {
        return Err(PortaError::invalid(
            "invalid_port",
            "Port must be between 1 and 65535",
        ));
    }
    Ok(port)
}

fn emit_success<T: Serialize>(response_type: ResponseType, payload: &T) -> Result<()> {
    println!("{}", envelope_value(response_type, payload)?);
    Ok(())
}

fn envelope_value<T: Serialize>(response_type: ResponseType, payload: &T) -> Result<Value> {
    let value = serde_json::to_value(payload).map_err(|error| output_error(&error))?;
    let Value::Object(mut object) = value else {
        return Err(PortaError::infrastructure(
            "output_failed",
            "JSON response payload must be an object",
        ));
    };
    if object.contains_key("version") || object.contains_key("type") {
        return Err(PortaError::infrastructure(
            "output_failed",
            "JSON response payload contains a reserved envelope field",
        ));
    }
    object.insert("version".to_owned(), Value::from(RESPONSE_VERSION));
    object.insert("type".to_owned(), Value::from(response_type.as_str()));
    Ok(Value::Object(object))
}

fn emit_error(error: &PortaError, json_output: bool) {
    if json_output {
        println!(
            "{}",
            json!({
                "version": RESPONSE_VERSION,
                "type": ResponseType::Error.as_str(),
                "error": { "code": error.code, "message": error.message }
            })
        );
    } else {
        let _ = writeln!(io::stderr(), "porta: {}", error.message);
    }
}

fn output_error(error: &serde_json::Error) -> PortaError {
    PortaError::infrastructure("output_failed", error.to_string())
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Locale};
    use serde_json::json;

    use super::{ResponseType, envelope_value, format_localized_datetime, resolve_locale};

    #[test]
    fn builds_typed_response_envelopes_and_rejects_invalid_payloads() {
        let response = envelope_value(ResponseType::Lease, &json!({ "port": 6006 }))
            .expect("response envelope");
        assert_eq!(response["version"], 1);
        assert_eq!(response["type"], "lease");
        assert_eq!(response["port"], 6006);

        for payload in [json!({ "version": 2 }), json!({ "type": "other" })] {
            let error = envelope_value(ResponseType::Lease, &payload)
                .expect_err("reserved field should fail");
            assert_eq!(error.code, "output_failed");
        }

        let error =
            envelope_value(ResponseType::Lease, &6006).expect_err("scalar payload should fail");
        assert_eq!(error.code, "output_failed");
    }

    #[test]
    fn resolves_common_system_locale_forms() {
        assert_eq!(resolve_locale(Some("en-US")), Locale::en_US);
        assert_eq!(resolve_locale(Some("de_DE.UTF-8")), Locale::de_DE);
        assert_eq!(resolve_locale(Some("C")), Locale::POSIX);
        assert_eq!(resolve_locale(Some("unsupported")), Locale::en_US);
        assert_eq!(resolve_locale(None), Locale::en_US);
    }

    #[test]
    fn formats_date_and_time_for_the_selected_locale() {
        let timestamp =
            DateTime::parse_from_rfc3339("2030-01-15T12:30:00-08:00").expect("fixed timestamp");

        assert_eq!(
            format_localized_datetime(timestamp, Locale::en_US),
            "01/15/2030 12:30:00 PM -08:00"
        );
        assert_eq!(
            format_localized_datetime(timestamp, Locale::de_DE),
            "15.01.2030 12:30:00 -08:00"
        );
    }
}
