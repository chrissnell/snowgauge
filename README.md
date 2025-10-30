# SnowGauge Rust Port

Rust implementation of the SnowGauge gRPC service for reading snow depth measurements from ultrasonic sensors.

## Build

```bash
cargo build --release
```

## Run

### With Serial Port
```bash
cargo run -- --port /dev/ttyS0 --listen-addr 0.0.0.0:7669
```

### With Simulator
```bash
cargo run -- --simulator --simulator-base-distance 1000.0 --listen-addr 0.0.0.0:7669 --log
```

## Command Line Options

- `--port`: Serial port name (default: /dev/ttyS0)
- `--debug`: Enable debug logging
- `--listen-addr`: gRPC server address (default: 0.0.0.0:7669)
- `--log`: Log distance measurements to stdout
- `--simulator`: Enable simulator mode
- `--simulator-base-distance`: Starting distance in mm for simulator (default: 1000.0)

All options can also be set via environment variables:
- `PORT`
- `DEBUG`
- `LISTEN_ADDR`
- `LOG_DISTANCE`
- `SIMULATOR`
- `SIMULATOR_BASE_DISTANCE`