# embedded-runner

Cargo runner for embedded projects using [GDB](https://www.sourceware.org/gdb/) and [OpenOCD](https://openocd.org/).

Ensure the GDB executable is either `arm-none-eabi-gdb` or set per environmental variable `GDB`.
The OpenOCD executable `openocd` must be available on path.

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

   Be sure to enable the `defmt` feature for [`mantra-rust-macros`](https://github.com/mhatzl/mantra/tree/main/langs/rust/mantra-rust-macros) to get the requirement coverage logs when using [*mantra*](https://github.com/mhatzl/mantra).

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

   # Optional: Connection to a GDB server to use instead of OpenOCD
   gdb-connection = ""

   # Optional: Path to write GDB logs to
   gdb-logfile = "<output directory>/gdb.log"

   # Optional: RTT port to use on the host
   rtt-port = 19021

   # Optional: Define a command to run before the runner executes the binary.
   # A 'post-runner' may also be set that is run after executing the binary.
   #
   # On windows, `pre-runner-windows` is available that takes precedence over `pre-runner`.
   # Same with `post-runner-windows`.
   [pre-runner]

   # Name of the command
   name = "powershell"

   # Arguments passed to the command.
   # The binary path is automatically added as last argument 
   args = ["echo"]
   ```

4. Optional: Add your OpenOCD configuration

   This configuration file is needed if no GDB connection is set in the `runner.toml` file.

   By default, `embedded-runner` will look for `.embedded/openocd.cfg`,
   but you may change this in the runner configuration.

5. Create and run your `defmt-test` tests

   Consult the [`defmt-test` documentation](https://crates.io/crates/defmt-test) on how to create and manage tests using the `defmt` framework.

6. Optional: Collect test results from multiple test runs

   Run `embedded-runner collect <output filepath>` to combine all test run results into one file.
   The content will be JSON adhering to the [mantra `CoverageSchema`](https://github.com/mhatzl/mantra).

# License

MIT Licensed
