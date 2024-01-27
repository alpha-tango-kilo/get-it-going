use std::{
    borrow::Cow,
    env,
    ffi::OsStr,
    fmt, fs, io,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

use anyhow::{anyhow, bail, Context};
use env_logger::{fmt::Color, Env};
use log::{debug, error, info, Level, LevelFilter};
use serde::{
    de::{Error, MapAccess, Visitor},
    Deserialize, Deserializer,
};
use shlex::Shlex;

fn main() -> ExitCode {
    let name = get_name();
    let name_clone = name.clone();
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
                "[{name_clone} {}]: {}",
                style.value(record.level()),
                record.args()
            )
        })
        .init();

    if let Err(why) = _main(&name) {
        error!("unable to launch {name}: {why}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn _main(name: &str) -> anyhow::Result<()> {
    // Step 1: read config
    let config_file = find_config(name)?
        .ok_or_else(|| anyhow!("unable to find config file"))?;
    let config = fs::read_to_string(&config_file)
        .with_context(|| format!("couldn't read {}", config_file.display()))?;
    let config = toml::from_str::<AppConfig>(&config)?;

    // Step 2: work out if we're good to go
    let _root = if !config.required_files.is_empty() {
        if config.search_parents {
            search_parents(env::current_dir()?, &config.required_files)
                .ok_or_else(|| {
                    anyhow!(
                        "couldn't find required files in current or parent \
                         directories"
                    )
                })?
        } else {
            let current_dir = env::current_dir()?;
            files_exist_in(&current_dir, &config.required_files)
                .then_some(current_dir)
                .ok_or_else(|| {
                    anyhow!("couldn't find required files in current directory")
                })?
        }
    } else {
        env::current_dir()?
    };

    // Step 3: run before_run task/script
    let mut command = match &config.before_run {
        BeforeRun::Command(cmd_str) => {
            let mut iter = Shlex::new(cmd_str);
            let mut command = Command::new(iter.next().unwrap());
            command.args(iter);
            command
        },
        BeforeRun::ScriptPath(path) => Command::new(path),
    };
    let status = command.status().context("failed to run before_run")?;
    if !status.success() {
        // TODO: include exit code?
        bail!("before_run returned a non-zero status");
    }

    // Step 4: build and spawn process
    let program: Cow<Path> = match &config.run {
        Run::SubcommandOf(this) => Path::new(this).into(),
        Run::PrependFolder(folder) => folder.join(name).into(),
        Run::Executable(this) => this.into(),
    };
    let mut command = Command::new(program.as_os_str());
    if matches!(&config.run, Run::SubcommandOf(_)) {
        command.arg(name);
    }
    command.args(env::args_os().skip(1));
    log_command(name, &command);
    command.spawn().context("failed to run")?;
    Ok(())
}

fn get_name() -> Box<str> {
    match env::var("GIG_OVERRIDE") {
        Ok(name) => name.into_boxed_str(),
        Err(_) => {
            let executable = env::current_exe().unwrap();
            executable.file_stem().unwrap().to_string_lossy().into()
        },
    }
}

fn find_config(name: &str) -> Result<Option<PathBuf>, io::Error> {
    let config_name = format!("{name}.toml");
    // TODO: are these dirs any good?
    for folder in [env::current_dir()?, dirs::config_dir().unwrap()] {
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

fn search_parents(dir: impl AsRef<Path>, files: &[PathBuf]) -> Option<PathBuf> {
    let mut dir = dir.as_ref();
    if files_exist_in(dir, files) {
        return Some(dir.to_owned());
    }
    while let Some(parent) = dir.parent() {
        dir = parent;
        if files_exist_in(dir, files) {
            return Some(dir.to_owned());
        }
    }
    None
}

fn log_command(name: &str, command: &Command) {
    let program = command.get_program().to_str().expect(
        "program should be UTF-8 when it was made from UTF-8 originally",
    );
    let args = command
        .get_args()
        .map(OsStr::to_string_lossy)
        .collect::<Vec<_>>();
    let args = args.join(" ");
    info!("spawning {name} by running: {program} {args}");
}

#[derive(Debug, Deserialize)]
struct AppConfig {
    required_files: Vec<PathBuf>,
    #[serde(default)]
    search_parents: bool,
    before_run: BeforeRun,
    run: Run,
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
