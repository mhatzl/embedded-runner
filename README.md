# embedded-runner

Cargo runner for embedded projects using [OpenOCD](https://openocd.org/).

## Usage

1. Install the crate using `cargo install embedded-runner`.

2. Set `embedded-runner run` as cargo runner at `.cargo/config.toml`

   ```
   [target(<your target configuration>)]
   runner = "embedded-runner run"
   ```

# License

MIT Licensed
