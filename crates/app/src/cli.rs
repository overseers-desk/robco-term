//! Command-line parsing for the application shell.
//!
//! This is a hand-rolled parser rather than clap, for one reason that runs
//! through the whole contract: `-e` **catches every argument after it**,
//! including things that look like our own options, because the point of
//! `-e` is to hand a command line to a child program: "This option will
//! catch all following arguments, so use it as the last option" -- what every
//! terminal emulator's `-e` does. Expressing it in clap means
//! `trailing_var_arg` plus `allow_hyphen_values` plus a subcommand-free
//! escape hatch; expressing it here is one `if`.
//!
//! The option set below is what `xtask contract` treats as normative:
//!
//! ```text
//! --default-settings   ignore user config, start from built-in defaults
//! --workdir <dir>      working directory for the session
//! --program <prog>     program to run instead of the user's shell
//! -e <cmd> [args...]   command to execute; catches all following arguments
//! -p, --profile <prof> start from the named built-in profile
//! --fullscreen         start fullscreen
//! --verbose            print profiles/settings decisions to stderr
//! -h, --help           print help
//! -v, --version        print version
//! ```
//!
//! Six flags beyond that list are this rebuild's own, and none changes
//! what `xtask verify`'s flag-presence check expects of the documented
//! contract items above. `--frame-stats` is the live-preview throughput
//! instrument's CLI switch: it logs a p50/p99 line periodically and does
//! nothing when absent. `--ssh [user@]host[:port]` opens the first channel
//! on an SSH connection this program owns (#14) instead of a shell; the
//! spelling is validated here, so a bad one fails before a window exists.
//! `--dump-settings` prints the settings dump (defaults, presets, the
//! bundled font catalogue, enum value lists; see `config::dump`) on stdout
//! and exits, the query external settings tools build their view from.
//! `--list-renderable-fonts` is the machine's own monospace families as this
//! terminal's renderer can resolve them, asked for separately because that
//! answer costs a walk of the platform's font directories and the settings
//! dump does not. `--settings`
//! opens the settings window and nothing else: where the window is linked
//! into this binary this process becomes it, and everywhere else the
//! companion application is started and waited on, so the flag means the
//! same thing wherever it is typed. `--settings-selftest` is that same door
//! with the interpreter asked to prove it came up and then leave; it is a
//! build proof rather than a contract item, so it stays out of the help
//! text.

use std::ffi::OsString;
use std::path::PathBuf;

/// Everything the shell needs off the command line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// `--default-settings`: start from built-in defaults, never touching
    /// the user's real config file. Contract item 1.
    pub default_settings: bool,
    /// `-p`/`--profile <name>`: the built-in screen profile to seed from.
    /// `None` means "whatever the config says".
    pub profile: Option<String>,
    /// `--workdir <dir>`. `None` means the process's current directory.
    pub workdir: Option<PathBuf>,
    /// `--program <prog>`: the program to run instead of the user's shell.
    pub program: Option<OsString>,
    /// `-e <cmd> [args...]`, split into the executable and its arguments.
    ///
    /// "No `-e`" (fall back to the shell) is a distinct case from "`-e cmd`
    /// with no arguments of its own" (a command plus an *empty* argument
    /// list). That distinction is `Option<Command>` here, with `args`
    /// possibly empty.
    pub command: Option<Command>,
    /// `--fullscreen`. Also a payload of the new-window IPC message.
    pub fullscreen: bool,
    /// `--ssh [user@]host[:port]`: channel 1 dials this instead of opening
    /// a shell. Validated here so a bad spelling fails before a window
    /// exists; the parsed form is [`crate::ssh::SshRequest`].
    pub ssh: Option<String>,
    /// `--verbose`.
    pub verbose: bool,
    /// `--frame-stats`: the live-preview throughput instrument, off
    /// the CLI's beaten path (see the module doc). Logs p50/p99 GPU pass
    /// timings and the present interval every few seconds.
    pub frame_stats: bool,
    /// `--dump-settings`: print the settings dump on stdout and exit
    /// without opening a window (see the module doc).
    pub dump_settings: bool,
    /// `--list-renderable-fonts`: print the machine's monospace families
    /// on stdout and exit -- the ones this terminal's renderer can resolve,
    /// which is the list's whole point. A generic enumerator (fontconfig,
    /// Tk's `font families`) also lists faces this renderer cannot load,
    /// and a settings window offering one of those would write a
    /// `font_name` the terminal silently falls back from. Only the
    /// renderer's own database can name the set it will honour. The one
    /// flag that asks for a font scan, which is why it is its own flag
    /// rather than a part of the settings dump.
    pub list_renderable_fonts: bool,
    /// `--font <family>`: the face the glass draws in for this run,
    /// whatever the profile says. Looked up in the machine's catalogue,
    /// which holds the bundled faces too, so either kind of name works.
    /// A run-scoped override rather than an edit: it outlives a live
    /// config reload and leaves the user's file alone.
    pub font: Option<String>,
    /// `--prose-font <family>`: a second face, in which a row with nothing
    /// to line up is set with each character at its own width. The `--font`
    /// face stays the grid's ruler and sets the rows that have something to
    /// line up. A family name off the machine, not a catalogue name: the
    /// catalogue offers fixed-pitch faces only, and this one is meant not
    /// to be.
    pub prose_font: Option<String>,
    /// `--settings`: open the settings window instead of a terminal. No
    /// glass, no instance lock, no config watch; this process is the
    /// settings application for as long as it lives.
    pub settings: bool,
    /// `--settings-selftest`: the same door with the interpreter asked to
    /// prove it came up and then exit. Out of the help text on purpose (see
    /// the module doc).
    pub settings_selftest: bool,
}

/// An `-e` command: the program and the arguments after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub program: OsString,
    pub args: Vec<OsString>,
}

/// What `parse` decided the process should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Run the shell with these options.
    Run(Options),
    /// Print this text on stdout and exit 0 (`--help`, `--version`).
    Print(String),
    /// Print this on stderr and exit non-zero.
    Fail(String),
}

/// Usage text.
pub fn help(program_name: &str) -> String {
    format!(
        "Usage: {program_name} [--default-settings] [--workdir <dir>] [--program <prog>] \
[-p|--profile <prof>] [--fullscreen] [-h|--help]\n\
\x20 --default-settings  Run {program_name} with the default settings\n\
\x20 --font <family>     Draw the glass in this font family for this run.\n\
\x20 --prose-font <family> Set rows with nothing to line up in this family, each character at its own width.\n\
\x20 --workdir <dir>     Change working directory to 'dir'\n\
\x20 -e <cmd>            Command to execute. This option will catch all following arguments, so use it as the last option.\n\
\x20 --program <prog>    Program to run instead of the user's shell.\n\
\x20 --fullscreen        Run {program_name} in fullscreen.\n\
\x20 --ssh [user@]host[:port]  Open the first channel on an SSH connection to 'host' (outranks the config's [ssh] default).\n\
\x20 -p|--profile <prof> Run {program_name} with the given profile.\n\
\x20 -h|--help           Print this help.\n\
\x20 --verbose           Print additional information such as profiles and settings.\n\
\x20 --frame-stats       Log GPU frame-timing p50/p99 periodically (this rebuild only).\n\
\x20 --dump-settings     Print defaults, presets, bundled fonts and value lists as TOML, then exit.\n\
\x20 --list-renderable-fonts Print the machine's monospace families this terminal can render, as TOML, then exit.\n\
\x20 --settings          Open the settings window instead of a terminal.\n"
    )
}

/// Parse an argument list *without* argv[0]. `program_name` is the
/// binary's basename, used only for the help and version text.
pub fn parse<I, S>(program_name: &str, args: I) -> Outcome
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    let mut opts = Options::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        let text = arg.to_string_lossy().into_owned();
        match text.as_str() {
            // Answering -h/--help and -v/--version wherever they appear
            // (before any `-e`), rather than only when they are the very
            // first argument, is strictly friendlier and breaks nothing:
            // `-e` swallowing is still checked first by position.
            "-h" | "--help" => return Outcome::Print(help(program_name)),
            "-v" | "--version" => {
                return Outcome::Print(format!("{program_name} {}\n", crate::VERSION))
            }
            "--default-settings" => opts.default_settings = true,
            "--fullscreen" => opts.fullscreen = true,
            "--verbose" => opts.verbose = true,
            "--frame-stats" => opts.frame_stats = true,
            "--dump-settings" => opts.dump_settings = true,
            "--list-renderable-fonts" => opts.list_renderable_fonts = true,
            "--settings" => opts.settings = true,
            "--settings-selftest" => opts.settings_selftest = true,
            "-p" | "--profile" => match args.get(i + 1) {
                Some(v) => {
                    opts.profile = Some(v.to_string_lossy().into_owned());
                    i += 1;
                }
                None => return Outcome::Fail(format!("{text} needs a profile name\n")),
            },
            "--font" => match args.get(i + 1) {
                Some(v) => {
                    opts.font = Some(v.to_string_lossy().into_owned());
                    i += 1;
                }
                None => return Outcome::Fail("--font needs a family name\n".into()),
            },
            "--prose-font" => match args.get(i + 1) {
                Some(v) => {
                    opts.prose_font = Some(v.to_string_lossy().into_owned());
                    i += 1;
                }
                None => return Outcome::Fail("--prose-font needs a family name\n".into()),
            },
            "--workdir" => match args.get(i + 1) {
                Some(v) => {
                    opts.workdir = Some(PathBuf::from(v));
                    i += 1;
                }
                None => return Outcome::Fail("--workdir needs a directory\n".into()),
            },
            "--ssh" => match args.get(i + 1) {
                Some(v) => {
                    let spec = v.to_string_lossy().into_owned();
                    if let Err(e) = crate::ssh::SshRequest::parse(&spec) {
                        return Outcome::Fail(format!("--ssh: {e}\n"));
                    }
                    opts.ssh = Some(spec);
                    i += 1;
                }
                None => return Outcome::Fail("--ssh needs a destination\n".into()),
            },
            "--program" => match args.get(i + 1) {
                Some(v) => {
                    opts.program = Some(v.clone());
                    i += 1;
                }
                None => return Outcome::Fail("--program needs a program\n".into()),
            },
            "-e" => {
                // Everything after `-e` belongs to the child, options and
                // all. A bare trailing `-e` leaves the command unset.
                let rest = &args[i + 1..];
                if let Some((program, tail)) = rest.split_first() {
                    opts.command = Some(Command {
                        program: program.clone(),
                        args: tail.to_vec(),
                    });
                }
                return Outcome::Run(opts);
            }
            other => {
                return Outcome::Fail(format!("unknown option: {other}\n{}", help(program_name)))
            }
        }
        i += 1;
    }

    Outcome::Run(opts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Options {
        match parse("robco-term", args.iter().copied()) {
            Outcome::Run(o) => o,
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn empty_command_line_is_all_defaults() {
        assert_eq!(run(&[]), Options::default());
    }

    #[test]
    fn contract_flags_parse() {
        let o = run(&[
            "--default-settings",
            "--profile",
            "Deep Blue",
            "--workdir",
            "/tmp",
            "--fullscreen",
            "--verbose",
        ]);
        assert!(o.default_settings);
        assert_eq!(o.profile.as_deref(), Some("Deep Blue"));
        assert_eq!(o.workdir, Some(PathBuf::from("/tmp")));
        assert!(o.fullscreen);
        assert!(o.verbose);
    }

    #[test]
    fn frame_stats_flag_parses_and_defaults_off() {
        assert!(!run(&[]).frame_stats);
        assert!(run(&["--frame-stats"]).frame_stats);
    }

    #[test]
    fn dump_settings_flag_parses_and_defaults_off() {
        assert!(!run(&[]).dump_settings);
        assert!(run(&["--dump-settings"]).dump_settings);
    }

    #[test]
    fn list_renderable_fonts_flag_parses_and_defaults_off() {
        assert!(!run(&[]).list_renderable_fonts);
        assert!(run(&["--list-renderable-fonts"]).list_renderable_fonts);
        // The two are separate answers: asking for the settings does not
        // ask for the scan.
        assert!(!run(&["--dump-settings"]).list_renderable_fonts);
        assert!(!run(&["--list-renderable-fonts"]).dump_settings);
    }

    #[test]
    fn settings_flags_parse_and_default_off() {
        assert!(!run(&[]).settings);
        assert!(!run(&[]).settings_selftest);
        assert!(run(&["--settings"]).settings);
        assert!(run(&["--settings-selftest"]).settings_selftest);
    }

    #[test]
    fn dash_e_swallows_the_settings_flags_too() {
        // The same rule as every other flag of ours: after `-e` these belong
        // to the child, and a command line ending in one still opens a
        // terminal running that command.
        let o = run(&["-e", "sh", "-c", "echo hi", "--settings"]);
        assert!(!o.settings);
        assert_eq!(
            o.command.expect("command").args.last(),
            Some(&OsString::from("--settings"))
        );
        assert!(!run(&["-e", "sh", "--settings-selftest"]).settings_selftest);
    }

    #[test]
    fn ssh_flag_takes_a_destination_and_refuses_a_bad_one() {
        assert_eq!(run(&[]).ssh, None);
        assert_eq!(
            run(&["--ssh", "overseer@vault:2222"]).ssh.as_deref(),
            Some("overseer@vault:2222")
        );
        assert!(matches!(
            parse("robco-term", ["--ssh"]),
            Outcome::Fail(text) if text.contains("destination")
        ));
        assert!(matches!(
            parse("robco-term", ["--ssh", "vault:notaport"]),
            Outcome::Fail(text) if text.contains("not a port")
        ));
    }

    #[test]
    fn short_profile_flag_is_the_same_flag() {
        assert_eq!(run(&["-p", "Plasma"]).profile.as_deref(), Some("Plasma"));
    }

    #[test]
    fn dash_e_catches_everything_after_it() {
        // The whole point: `--fullscreen` here is the child's argument,
        // not ours, so the window must not come up fullscreen.
        let o = run(&["-e", "sh", "-c", "echo hi", "--fullscreen"]);
        let cmd = o.command.expect("command");
        assert_eq!(cmd.program, OsString::from("sh"));
        assert_eq!(
            cmd.args,
            vec![
                OsString::from("-c"),
                OsString::from("echo hi"),
                OsString::from("--fullscreen")
            ]
        );
        assert!(!o.fullscreen);
    }

    #[test]
    fn our_flags_before_dash_e_still_count() {
        let o = run(&["--fullscreen", "-e", "top"]);
        assert!(o.fullscreen);
        assert_eq!(o.command.unwrap().program, OsString::from("top"));
    }

    #[test]
    fn dash_e_with_no_arguments_of_its_own_is_an_empty_arg_list() {
        let cmd = run(&["-e", "htop"]).command.expect("command");
        assert!(cmd.args.is_empty());
    }

    #[test]
    fn bare_trailing_dash_e_leaves_no_command() {
        assert!(run(&["-e"]).command.is_none());
    }

    #[test]
    fn help_and_version_print_and_stop() {
        assert!(matches!(parse("robco-term", ["--help"]), Outcome::Print(_)));
        assert!(matches!(parse("robco-term", ["-v"]), Outcome::Print(_)));
    }

    #[test]
    fn unknown_option_fails_loudly() {
        assert!(matches!(parse("robco-term", ["--nope"]), Outcome::Fail(_)));
    }

    #[test]
    fn missing_option_value_fails() {
        assert!(matches!(
            parse("robco-term", ["--profile"]),
            Outcome::Fail(_)
        ));
        assert!(matches!(
            parse("robco-term", ["--workdir"]),
            Outcome::Fail(_)
        ));
    }
}
