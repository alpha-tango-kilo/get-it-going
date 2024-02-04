#![deny(clippy::undocumented_unsafe_blocks)]
#![deny(unsafe_op_in_unsafe_fn)]

use std::{
    borrow::Cow,
    env,
    ffi::OsStr,
    fmt, fs,
    io::Write,
    ops::ControlFlow,
    path::{Path, PathBuf},
    process::{Command, ExitCode, ExitStatus},
};

use anyhow::{bail, Context};
use env_logger::{fmt::Color, Env};
use log::{debug, error, info, warn, Level, LevelFilter};
use once_cell::sync::Lazy;
use serde::{
    de::{Error, MapAccess, Visitor},
    Deserialize, Deserializer,
};
use shlex::Shlex;

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
compile_error!("unsupported OS: only Windows, MacOS, and Linux currently");

static NAME: Lazy<Box<str>> = Lazy::new(|| match env::var("GIG_OVERRIDE") {
    Ok(name) => name.into_boxed_str(),
    Err(_) => {
        let executable = env::current_exe().expect("can't access own path");
        executable.file_stem().unwrap().to_string_lossy().into()
    },
});

static SYSTEM_WIDE_CONFIG_DIRECTORY: Lazy<PathBuf> = Lazy::new(|| {
    #[cfg(windows)]
    let system_config_dir = Path::new("C:\\Program Files\\Common Files");
    #[cfg(target_os = "macos")]
    let system_config_dir = Path::new("/Library/Application Support");
    #[cfg(target_os = "linux")]
    let system_config_dir = Path::new("/etc");
    system_config_dir.join("get-it-going")
});

static CWD: Lazy<PathBuf> = Lazy::new(|| {
    env::current_dir()
        .expect("get-it-going must have access to current working directory")
});

fn main() -> ExitCode {
    env_logger::builder()
        .filter_level(LevelFilter::Warn)
        .parse_env(Env::new().filter("GIG_LOG"))
        .format(move |buf, record| {
            let mut style = buf.style();
            match record.level() {
                Level::Error => {
                    style.set_color(Color::Red);
                },
                Level::Warn => {
                    style.set_color(Color::Yellow);
                },
                Level::Info => {},
                Level::Debug | Level::Trace => {
                    style.set_dimmed(true);
                },
            }
            writeln!(
                buf,
                "[{} {}]: {}",
                NAME.as_ref(),
                style.value(record.level()),
                record.args()
            )
        })
        .init();

    match _main() {
        Ok(status) => {
            // Some scuff to get i32 exit codes into u8 without wrapping to
            // non-zero to zero
            let orig_code = status.code();
            let exit_code = orig_code
                .unwrap_or(!status.success() as i32)
                .unsigned_abs() as u8;
            debug!(
                "exited with status {orig_code:?}, converted to {exit_code}",
            );
            ExitCode::from(exit_code)
        },
        Err(why) => {
            error!("unable to launch {}: {why}", NAME.as_ref());
            ExitCode::FAILURE
        },
    }
}

fn _main() -> anyhow::Result<ExitStatus> {
    // Step 1: read config
    let config = AppConfig::find_and_load()?;

    // Step 2: work out if we're good to go, and where to run from
    let root = match config.get_root() {
        Some(root) => root,
        // If we're not good to go, do we have a fallback to run instead?
        None => match config.generate_fallback() {
            Some(command) => {
                info!("unable to locate required files, running fallback");
                let status = command.status()?;
                return Ok(status);
            },
            None => bail!("couldn't find required files"),
        },
    };

    // Step 3: run before_run task/script
    let command = config.generate_before_run(&root);
    let status = command.status().context("failed to run before_run")?;
    if !status.success() {
        bail!("before_run returned a non-zero status");
    }

    // Step 4: build and spawn process
    let command = config.generate_run(&root);
    // TODO: better error message
    let status = command.status()?;
    Ok(status)
}

#[derive(Debug, Deserialize)]
struct AppConfig {
    #[serde(default)]
    required_files: Vec<PathBuf>,
    #[serde(default)]
    search_parents: bool,
    before_run: BeforeRun,
    run: Run,
    #[serde(default)]
    fallback: Option<Fallback>,
}

impl AppConfig {
    fn find_and_load() -> anyhow::Result<Self> {
        let config_name = format!("{}.toml", &*NAME);
        let config_file_flow = [&*CWD, &*SYSTEM_WIDE_CONFIG_DIRECTORY]
            .iter()
            .try_for_each(|&dir| {
                let config_file = dir.join(&config_name);
                debug!("checking if {} exists", config_file.display());
                if config_file.exists() {
                    info!("found {}", config_file.display());
                    return ControlFlow::Break(config_file);
                }
                ControlFlow::Continue(())
            });
        let config_file = match config_file_flow {
            ControlFlow::Break(path) => path,
            ControlFlow::Continue(()) => bail!("unable to find config file"),
        };

        let config = fs::read_to_string(&config_file).with_context(|| {
            format!("couldn't read {}", config_file.display())
        })?;
        let config = toml::from_str::<AppConfig>(&config)?;
        config.lint();
        Ok(config)
    }

    fn get_root(&self) -> Option<Cow<Path>> {
        let files_exist_in = |dir: &Path, files: &[PathBuf]| {
            files.iter().all(|file_name| dir.join(file_name).exists())
        };

        if !self.required_files.is_empty() {
            if self.search_parents {
                let mut dir: &Path = &CWD;
                if files_exist_in(dir, &self.required_files) {
                    return Some(dir.into());
                }
                // Can't use while-let with break values, so we overcome
                loop {
                    match dir.parent() {
                        Some(dir)
                            if files_exist_in(dir, &self.required_files) =>
                        {
                            break Some(dir.to_owned().into());
                        },
                        Some(new_dir) => dir = new_dir,
                        None => break None,
                    }
                }
            } else {
                files_exist_in(&CWD, &self.required_files)
                    .then_some(Cow::<Path>::Borrowed(&*CWD))
            }
        } else {
            Some(Cow::<Path>::Borrowed(&*CWD))
        }
    }

    fn generate_before_run(&self, root: &Path) -> LoggedCommand {
        let mut command = match &self.before_run {
            BeforeRun::Command(cmd_str) => {
                let mut iter = Shlex::new(cmd_str);
                let mut command = Command::new(iter.next().unwrap());
                command.args(iter);
                command
            },
            BeforeRun::ScriptPath(path) => Command::new(path),
        };
        command.current_dir(root);
        LoggedCommand(command)
    }

    fn generate_run(&self, root: &Path) -> LoggedCommand {
        let program: Cow<Path> = match &self.run {
            Run::SubcommandOf(this) => Path::new(this).into(),
            Run::PrependFolder(folder) => {
                let exe_name: Cow<str> = if cfg!(windows) {
                    format!("{}.exe", NAME.as_ref()).into()
                } else {
                    NAME.as_ref().into()
                };
                folder.join(Path::new(exe_name.as_ref())).into()
            },
            Run::Executable(this) => this.into(),
        };

        let mut command = Command::new(program.as_os_str());
        if matches!(&self.run, Run::SubcommandOf(_)) {
            command.arg(NAME.as_ref());
        }
        command.args(env::args_os().skip(1));
        command.current_dir(root);
        LoggedCommand(command)
    }

    fn generate_fallback(&self) -> Option<LoggedCommand> {
        self.fallback.as_ref().map(|fallback| {
            let command = match &fallback.path {
                Some(path) => {
                    let mut command = Command::new(path);
                    command.args(env::args_os().skip(1));
                    command
                },
                None => {
                    // Re-run command without GIG in $PATH
                    let gig_path = env::current_exe().unwrap();
                    let gig_dir = gig_path
                        .parent()
                        .unwrap()
                        .as_os_str()
                        .as_encoded_bytes();

                    let path = env::var_os("PATH").expect("$PATH unset");
                    let path_bytes = path.as_encoded_bytes();
                    let path_parts = path_bytes
                        .split(|&byte| byte == b':')
                        .filter(|&slice| slice != gig_dir)
                        .map(|slice|
                            // SAFETY: we are calling
                            // OsStr::from_encoded_bytes_unchecked on bytes
                            // made by OsStr::as_encoded_bytes, only having
                            // split on valid UTF-8 characters.
                            // Also, I'm basically doing the example code from
                            // the Rust docs of
                            // OsStr::from_encoded_bytes_unchecked lol
                            unsafe { OsStr::from_encoded_bytes_unchecked(slice) }
                        )
                        .collect::<Vec<_>>();
                    let new_path = path_parts.join(OsStr::new(":"));
                    debug!(
                        "$PATH before:\n{}\n$PATH after:\n{}",
                        path.to_string_lossy(),
                        new_path.to_string_lossy(),
                    );

                    let mut command = Command::new(NAME.as_ref());
                    command.env("PATH", new_path).args(env::args_os().skip(1));
                    command
                },
            };
            LoggedCommand(command)
        })
    }

    fn lint(&self) {
        if self.required_files.is_empty() && self.search_parents {
            warn!(
                "search_parents has no effect if there are no required files"
            );
        }
        if self.required_files.is_empty() && self.fallback.is_some() {
            warn!("fallback has no effect if there are no required files");
        }
    }
}

#[derive(Debug)]
enum BeforeRun {
    Command(String),
    ScriptPath(PathBuf),
}

impl<'de> Deserialize<'de> for BeforeRun {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct BeforeRunVisitor;

        impl<'de> Visitor<'de> for BeforeRunVisitor {
            type Value = BeforeRun;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("before_run table")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let (key, value) =
                    map.next_entry::<String, String>()?.ok_or_else(|| {
                        A::Error::custom("empty before_run table")
                    })?;
                match key.as_str() {
                    "command" => {
                        if !value.is_empty() {
                            Ok(BeforeRun::Command(value))
                        } else {
                            Err(A::Error::custom("command can't be empty"))
                        }
                    },
                    "script_path" => {
                        let path = PathBuf::from(value);
                        if path.is_file() {
                            Ok(BeforeRun::ScriptPath(path))
                        } else {
                            Err(A::Error::custom("invalid path (not a file)"))
                        }
                    },
                    unknown => Err(A::Error::custom(format_args!(
                        "unrecognised key \"{unknown}\", expected \"command\" \
                         or \"script_path\""
                    ))),
                }
            }
        }

        deserializer.deserialize_map(BeforeRunVisitor)
    }
}

#[derive(Debug)]
enum Run {
    SubcommandOf(String),
    PrependFolder(PathBuf),
    Executable(PathBuf),
}

impl<'de> Deserialize<'de> for Run {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RunVisitor;

        impl<'de> Visitor<'de> for RunVisitor {
            type Value = Run;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("run table")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let (key, value) = map
                    .next_entry::<String, String>()?
                    .ok_or_else(|| A::Error::custom("empty run table"))?;
                match key.as_str() {
                    "subcommand_of" => Ok(Run::SubcommandOf(value)),
                    "path" => {
                        if value.ends_with('/') {
                            Ok(Run::PrependFolder(value.into()))
                        } else {
                            Ok(Run::Executable(value.into()))
                        }
                    },
                    unknown => Err(A::Error::custom(format_args!(
                        "unrecognised key \"{unknown}\", expected \
                         \"subcommand_of\" or \"path\""
                    ))),
                }
            }
        }

        deserializer.deserialize_map(RunVisitor)
    }
}

#[derive(Debug, Deserialize)]
struct Fallback {
    #[serde(default)]
    path: Option<PathBuf>,
}

#[derive(Debug)]
struct LoggedCommand(Command);

impl LoggedCommand {
    fn log(&self) {
        info!("spawning {} by running: {self}", NAME.as_ref());
    }

    fn status(mut self) -> anyhow::Result<ExitStatus> {
        self.log();
        self.0
            .status()
            .with_context(|| format!("failed to invoke {self}"))
    }
}

impl fmt::Display for LoggedCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let program = self.0.get_program().to_str().expect(
            "program should be UTF-8 when it was made from UTF-8 originally",
        );
        let args = self
            .0
            .get_args()
            .map(OsStr::to_string_lossy)
            .collect::<Vec<_>>()
            .join(" ");
        let cwd = self
            .0
            .get_current_dir()
            .filter(|cwd| *cwd != *CWD)
            .map_or(String::new(), |cwd| format!(" in {}", cwd.display()));
        write!(f, "`{program} {args}`{cwd}")
    }
}

#[cfg(test)]
mod unit_tests {
    use crate::AppConfig;

    #[test]
    fn deserialise_example() {
        let app_config =
            toml::from_str::<AppConfig>(include_str!("../config.example.toml"))
                .expect("should deserialise");
        dbg!(app_config);
    }
}
