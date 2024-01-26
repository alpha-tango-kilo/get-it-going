executable_suffix := if os_family() == "windows" { ".exe" } else { "" }

@default:
    just --list

# Builds get-it-going and creates a renamed executable with a config file
ship name:
    cargo build --release
    cp target/release/get-it-going{{executable_suffix}} "{{name}}{{executable_suffix}}"
    cp config.example.toml "{{name}}.toml"
