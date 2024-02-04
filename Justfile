executable_suffix := if os_family() == "windows" { ".exe" } else { "" }
target := `rustc +nightly -vV | sed -n 's|host: ||p'`

@_default:
    just --list

# Builds get-it-going using nightly to be as small as possible
build:
    RUSTFLAGS="-Zlocation-detail=none" cargo +nightly build \
      -Z build-std=std,panic_abort \
      --target {{target}} \
      --release

# Builds get-it-going and creates a renamed executable with a config file
ship name: build
    cp target/release/get-it-going{{executable_suffix}} "{{name}}{{executable_suffix}}"
    cp config.example.toml "{{name}}.toml"
