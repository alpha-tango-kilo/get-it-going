use std::{
    env, fmt, fs, io,
    io::Write,
    path::PathBuf,
    process::{Command, ExitCode},
};

use anyhow::{anyhow, Context};
use env_logger::Env;
use log::{debug, error, info, LevelFilter};
use serde::{
    de::{Error, MapAccess, Visitor},
    Deserialize, Deserializer,
};

fn main() -> ExitCode {
    let name = get_name();
    let name_clone = name.clone();
    env_logger::builder()
        .filter_level(LevelFilter::Warn)
        .parse_env(Env::new().filter("GIG_LOG"))
        .format(move |buf, record| {
            writeln!(
                buf,
                "[{name_clone} {}]: {}",
                record.level(),
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
    let config_file = find_config(name)?
        .ok_or_else(|| anyhow!("unable to find config file"))?;
    let config = fs::read_to_string(&config_file)
        .with_context(|| format!("couldn't read {}", config_file.display()))?;
    let _config = toml::from_str::<AppConfig>(&config)?;
    // TODO: load config
    let program = "echo"; // TODO: use program from config
    info!("spawning {program}");
    Command::new(program)
        .args(env::args_os().skip(1))
        .spawn()
        .context("failed to run")?;
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
                    map.next_entry::<&'de str, String>()?.ok_or_else(|| {
                        A::Error::custom("empty before_run table")
                    })?;
                match key {
                    "command" => Ok(BeforeRun::Command(value)),
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
                    .next_entry::<&'de str, String>()?
                    .ok_or_else(|| A::Error::custom("empty run table"))?;
                match key {
                    "subcommand_of" => Ok(Run::SubcommandOf(value)),
                    "path" => {
                        let path = PathBuf::from(value);
                        if path.is_dir() {
                            Ok(Run::PrependFolder(path))
                        } else if path.is_file() {
                            Ok(Run::Executable(path))
                        } else {
                            Err(A::Error::custom(
                                "invalid path (not folder or file)",
                            ))
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
