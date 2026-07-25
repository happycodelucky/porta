//! Renders the man pages and shell completions that the packaged builds ship.
//!
//! Run it through mise as `mise run docs`, which writes into `target/dist`. The
//! release workflow calls the same task before it stages an archive or builds a
//! Debian package, so what ships is always generated from the commit being
//! released.
//!
//! Everything here comes out of the clap command tree in `port_authority::cli`,
//! the same tree that produces `--help`. Regenerating is the only way these
//! files change, so they cannot drift from the arguments porta accepts.
//!
//! This lives in `examples/` rather than `src/bin/` on purpose: `cargo install`
//! installs every binary target, and a documentation generator has no business
//! landing in anyone's `PATH`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use clap_complete::Shell;

/// Shells porta ships completions for. Each one has a conventional install
/// location that both Homebrew and Debian already know about, which is why the
/// list stops here rather than covering everything `Shell` can emit.
const SHELLS: [Shell; 3] = [Shell::Bash, Shell::Zsh, Shell::Fish];

fn main() -> io::Result<()> {
    let root = std::env::args_os()
        .nth(1)
        .map_or_else(|| PathBuf::from("target/dist"), PathBuf::from);

    let man = reset(&root.join("man"))?;
    let completions = reset(&root.join("completions"))?;

    // Writes porta.1 plus one page per subcommand, the same layout git and
    // cargo use. `man porta` covers the top level and points at the rest;
    // `man porta-lease` documents that subcommand's own flags.
    clap_mangen::generate_to(stamp_versions(port_authority::cli::command()), &man)?;
    report("man page", &man)?;

    for shell in SHELLS {
        let mut command = port_authority::cli::command();
        clap_complete::generate_to(shell, &mut command, "porta", &completions)?;
    }
    report("completion", &completions)?;

    Ok(())
}

/// Copies the crate version onto every subcommand.
///
/// `clap_mangen` puts `get_version()` in the page footer, and only the top-level
/// command carries one. Without this, `porta-listeners.1` footers as bare
/// "listeners" rather than "porta 0.9.0". The version flag is disabled as it is
/// applied so the generated page does not advertise a `--version` that the
/// subcommand would actually reject; the top level keeps its real one.
fn stamp_versions(command: clap::Command) -> clap::Command {
    let names: Vec<String> = command
        .get_subcommands()
        .map(|subcommand| subcommand.get_name().to_owned())
        .collect();

    names.into_iter().fold(command, |command, name| {
        command.mut_subcommand(name, |subcommand| {
            stamp_versions(subcommand)
                .version(env!("CARGO_PKG_VERSION"))
                .disable_version_flag(true)
        })
    })
}

/// Empties the directory before writing to it, so a renamed or removed command
/// cannot leave a stale page behind for the packaging steps to pick up.
fn reset(directory: &Path) -> io::Result<PathBuf> {
    if directory.exists() {
        fs::remove_dir_all(directory)?;
    }
    fs::create_dir_all(directory)?;
    Ok(directory.to_path_buf())
}

fn report(kind: &str, directory: &Path) -> io::Result<()> {
    let mut names: Vec<_> = fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<io::Result<_>>()?;
    names.sort_unstable();

    let plural = if names.len() == 1 { "" } else { "s" };
    let directory = directory.display();
    println!("{} {kind}{plural} in {directory}", names.len());
    for name in names {
        println!("  {}", name.to_string_lossy());
    }
    Ok(())
}
