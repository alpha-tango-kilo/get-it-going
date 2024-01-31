use std::{
    borrow::Cow,
    env,
    ffi::OsStr,
    fmt, fs, io,
    io::Write,
    path::{Path, PathBuf},
    process,
    process::{Command, ExitCode, ExitStatus},
};

use anyhow::{anyhow, bail, Context};
use env_logger::{fmt::Color, Env};
use log::{debug, error, info, warn, Level, LevelFilter};
use once_cell::sync::Lazy;
use serde::{
    de::{Error, MapAccess, Visitor},
    Deserialize, Deserializer,
};
use shlex::Shlex;

static NAME: Lazy<Box<str>> = Lazy::new(|| match env::var("GIG_OVERRIDE") {
    Ok(name) => name.into_boxed_str(),
    Err(_) => {
        let executable = env::current_exe().unwrap();
        executable.file_stem().unwrap().to_string_lossy().into()
    },
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

    if let Err(why) = _main() {
        error!("unable to launch {}: {why}", NAME.as_ref());
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn _main() -> anyhow::Result<()> {
    // Step 1: read config
    let config = AppConfig::find_and_load()?;

    // Step 2: work out if we're good to go, and where to run from
    let root = match config.get_root() {
        Some(root) => root,
        // If we're not good to go, do we have a fallback to run instead?
        None => match config.generate_fallback() {
            Some(command) => {
                warn!("unable to locate required files, running fallback");
                command.spawn().context("failed to run")?;
                return Ok(());
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
    command.spawn().context("failed to run")?;
    Ok(())
}

fn find_config(name: &str) -> Result<Option<PathBuf>, io::Error> {
    let config_name = format!("{name}.toml");
    // TODO: are these dirs any good?
    for folder in [
        Cow::<Path>::Borrowed(&CWD),
        dirs::config_dir().unwrap().into(),
    ] {
        let config_file = folder.join(&config_name);
        debug!("checking if {} exists", config_file.display());
        if config_file.exists() {
            info!("found {}", config_file.display());
            return Ok(Some(config_file));
        }
    }
    Ok(None)
}

fn files_exist_in(dir: impl AsRef<Path>, files: &[PathBuf]) -> bool {
    let dir = dir.as_ref();
    files.iter().all(|file_name| dir.join(file_name).exists())
}

fn search_parents(files: &[PathBuf]) -> Option<Cow<'static, Path>> {
    let mut dir: &Path = &CWD;
    if files_exist_in(dir, files) {
        return Some(dir.into());
    }
    while let Some(parent) = dir.parent() {
        dir = parent;
        if files_exist_in(dir, files) {
            return Some(dir.to_owned().into());
        }
    }
    None
}

#[derive(Debug, Deserialize)]
struct AppConfig {
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
        let config_file = find_config(&NAME)?
            .ok_or_else(|| anyhow!("unable to find config file"))?;
        let config = fs::read_to_string(&config_file).with_context(|| {
            format!("couldn't read {}", config_file.display())
        })?;
        Ok(toml::from_str::<AppConfig>(&config)?)
    }

    fn get_root(&self) -> Option<Cow<Path>> {
        if !self.required_files.is_empty() {
            if self.search_parents {
                search_parents(&self.required_files)
            } else {
                files_exist_in(&*CWD, &self.required_files)
                    .then_some(Cow::<Path>::Borrowed(&*CWD))
            }
        } else {
            Some(Cow::<Path>::Borrowed(&*CWD))
        }
    }

    fn generate_before_run(&self, root: impl AsRef<Path>) -> LoggedCommand {
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

    fn generate_run(&self, root: impl AsRef<Path>) -> LoggedCommand {
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
            let mut command = Command::new(&fallback.path);
            command.args(env::args_os().skip(1));
            LoggedCommand(command)
        })
    }
}

#[derive(Debug)]
enum BeforeRun {
    Command(String),
    // TODO: is it even practical to support this?
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
    path: PathBuf,
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

#[derive(Debug)]
struct LoggedCommand(Command);

impl LoggedCommand {
    fn log(&self) {
        let program = self.0.get_program().to_str().expect(
            "program should be UTF-8 when it was made from UTF-8 originally",
        );
        let args = self
            .0
            .get_args()
            .map(OsStr::to_string_lossy)
            .collect::<Vec<_>>();
        let args = args.join(" ");
        let cwd = self
            .0
            .get_current_dir()
            .filter(|cwd| *cwd != *CWD)
            .map_or(String::new(), |cwd| format!(" in {}", cwd.display()));
        info!(
            "spawning {} by running: `{program} {args}`{cwd}",
            NAME.as_ref(),
        );
    }

    fn status(mut self) -> io::Result<ExitStatus> {
        self.log();
        self.0.status()
    }

    fn spawn(mut self) -> io::Result<process::Child> {
        self.log();
        self.0.spawn()
    }
}
