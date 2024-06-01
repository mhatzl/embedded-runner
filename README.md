# embedded-runner

Cargo runner for embedded projects using [OpenOCD](https://openocd.org/).

## Usage

1. Install the crate using `cargo install embedded-runner`.

2. Set `embedded-runner run` as cargo runner at `.cargo/config.toml`

   ```
   [target(<your target configuration>)]
   runner = "embedded-runner run"
   ```

3. Create a runner configuration

   By default, `embedded-runner` will look for a runner configuration at `.embedded/runner.toml`.
   A custom path may be set by the `--runner-cfg` argument before the `run` command.

   If you use [*mantra*](https://github.com/mhatzl/mantra) to trace requirements, you may add the `[mantra]` section
   to automatically get trace and coverage data from executed tests.
   Be sure to enable the `defmt` feature for [`mantra-rust-macros`](https://github.com/mhatzl/mantra/tree/main/langs/rust/mantra-rust-macros) to get the required coverage logs.

   **The configuration allows the following settings:**

   ```toml
   # Optional: Load section in the gdb script.
   # 
   # The load section gets resolved using the Tera templating language.
   # Variables `binary_path`, `binary_filepath`, and `binary_filepath_noextension` are passed as context.
   #
   # e.g. "load {{ binary_filepath }}"
   load = "load"

   # Optional: Path to a custom OpenOCD configuration
   openocd-cfg = ".embedded/openocd.cfg"

   # Optional: Path to write OpenOCD logs to
   openocd-log = ".embedded/openocd.log"

   # Optional: RTT port to use
   rtt-port = 19021

   # Enables *mantra*
   [mantra]

   # Optional: The URL to connect to the *mantra* database
   db-url = "sqlite://.embedded/mantra.db?mode=rwc"

   # Optional: Includes traces outside the working directory
   extern_traces = ["../../crate-dep-test"]


   # Optional: Settings to automatically extract requirements
   [mantra.extract]

   # Path to look for requirements
   root = "../reqs.md"

   link = "local"

   # Kind of origin to extract requirements from.
   # Currently only "GitHub" is supported
   origin = "GitHub"


   # Optional: Define a command to run before the runner executes the binary.
   # A 'post-runner' may also be set that is run after executing the binary.
   [pre-runner]

   # Name of the command
   name = "powershell"

   # Arguments passed to the command.
   # The binary path is automatically added as last argument 
   args = ["echo"]
   ```

4. Create and run your `defmt-test` tests

   Consult the [`defmt-test` documentation](https://crates.io/crates/defmt-test) on how to create and manage tests using the `defmt` framework.

# License

MIT Licensed
