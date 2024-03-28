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

# Build a universal binary for get-it-going
[macos]
build-universal:
    RUSTFLAGS="-Zlocation-detail=none" cargo +nightly build \
        -Z build-std=std,panic_abort \
        --target x86_64-apple-darwin \
        --release
    RUSTFLAGS="-Zlocation-detail=none" cargo +nightly build \
        -Z build-std=std,panic_abort \
        --target aarch64-apple-darwin \
        --release
    @mkdir -p target/universal
    lipo -create -output target/universal/get-it-going \
        target/x86_64-apple-darwin/release/get-it-going \
        target/aarch64-apple-darwin/release/get-it-going

# Builds get-it-going and creates a renamed executable with a config file
ship name: build
    cp target/release/get-it-going{{executable_suffix}} "{{name}}{{executable_suffix}}"
    cp config.example.toml "{{name}}.toml"
