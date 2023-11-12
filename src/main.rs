use std::{env, io, io::Write, path::PathBuf, process::ExitCode};

use anyhow::anyhow;
use env_logger::Env;
use log::{debug, error, info, LevelFilter};

fn main() -> ExitCode {
    let name = get_name();
    env_logger::builder()
        .filter_level(LevelFilter::Warn)
        .parse_env(Env::new().filter("GIG_LOG"))
        .format(move |buf, record| {
            writeln!(buf, "[{name} {}]: {}", record.level(), record.args())
        })
        .init();

    if let Err(why) = _main(name) {
        error!("unable to launch {name}: {why}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn _main(name: &str) -> anyhow::Result<()> {
    let _config_file = find_config(name)?
        .ok_or_else(|| anyhow!("unable to find config file"))?;
    // TODO: load config
    Ok(())
}

// Small memory leak to get the name as a static string so it can be used in
// logs
fn get_name() -> &'static str {
    match env::var("GIG_OVERRIDE") {
        Ok(name) => Box::leak(name.into_boxed_str()),
        Err(_) => {
            let executable = env::current_exe().unwrap();
            let name = executable.file_stem().unwrap().to_string_lossy();
            let name = name.into();
            Box::leak(name)
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
