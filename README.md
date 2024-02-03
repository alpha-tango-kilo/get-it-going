# Get it going!

User friendly bootstrapping of tools that require per-project installs.
Intended to make life easier for tooling developers/devops/sysadmins who want their end-users to not have to think so hard about using tools that require per-project installs/versions.
GIG streamlines the flow for less commandline savvy users, only requiring that they try and run the tool in a valid context (i.e. not just in their home, but in a project repo)

## How does it work?

For sake of example, let's say you have a Python tool `wrench` that needs to be run in a project's virtual environment

GIG is designed to be installed system-wide in the user's path under the same name as the target program.
Thus, you compile GIG and rename the executable `wrench`, and distribute/install it.
Then either in your project folder, or in the system-wide configurations folder (location TBD), you create a `wrench.toml` configuration file, that looks something like this:

```toml
required_files = [
    "requirements.txt",
]
search_parents = true

[before_run]
command = "python -m venv venv && venv/bin/pip install -r requirements.txt"

[run]
path = "venv/bin/"
```

Now, when a user invokes `wrench` in a folder, GIG checks for the existence of the `requirements.txt` file, creates a venv and installs those requirements, and then invokes `venv/bin/wrench` (passing along any arguments from the original invocation)

## How do I use it?

1. Clone the repo
2. Run `just ship <name>` (if you don't have [just](https://github.com/casey/just) installed, just run the commands under the `ship:` heading manually)
3. Edit the created `<name>.toml` file to your needs
4. Distribute/Install the executable on users' machines (along with the configuration if opting for system-wide configuration)

## How heavy is the executable?

I'm making a concerted effort to keep the final GIG binary as small as possible, given it's just a shim, and may be installed multiple times (under different names) on a single system.
At the time of writing (before a stable release), the final executable size is 476 KB on Windows

## Roadmap / Future ideas

- [ ] Option to have `before_run` only be executed once, instead of on every invocation

- [x] Add a `[fallback]` section to customise behaviour when `required_files` aren't found

- [~] OS-specific values for command/path fields

- [ ] GUI version, for GUI tools? :o

- [ ] Proper installer for MacOS/Windows? (.msi or .pkg or whatever)

- [ ] Would symlinks work? Save duplicating binaries on the target device
