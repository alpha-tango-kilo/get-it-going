use std::{
    env, io,
    io::Write,
    path::PathBuf,
    process::{Command, ExitCode},
};

use anyhow::{anyhow, Context};
use env_logger::Env;
use log::{debug, error, info, LevelFilter};

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
    let _config_file = find_config(name)?
        .ok_or_else(|| anyhow!("unable to find config file"))?;
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
